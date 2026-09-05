//! 资源库存储层:文件布局、进程内互斥、原子写、加密感知读写。
//!
//! 互斥:应用经 single-instance 插件保证单进程,此处用进程内 `Mutex` 兜底
//! 并发命令(不引入锁文件,避免崩溃残留导致永久锁死)。git 网络操作另有
//! `mod.rs` 里的异步 `SYNC_LOCK` 串行化,避免并发 fetch/push 争抢 refs。
//!
//! 加密范围(**仅 mcp.json**):skills.json 与 skills/** 恒为明文,
//! 加密上锁后技能仍可正常管理,仅 MCP 定义读写需要解锁。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::de::DeserializeOwned;
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::APP_DATA_DIR_NAME;

#[cfg(test)]
use super::crypto::clear_key;
use super::crypto::{decrypt_bytes, encrypt_bytes, key_for};
use super::errors::{codes, RlError, RlResult};
use super::models::{LibraryMeta, LibraryState, McpServer, SkillLibrary};

pub const LIBRARY_DIR_NAME: &str = "resource-library";
pub const FILE_LIBRARY: &str = "library.json";
pub const FILE_SKILLS: &str = "skills.json";
pub const FILE_MCP: &str = "mcp.json";
pub const DIR_SKILLS: &str = "skills";
pub const SKILL_BODY_FILE: &str = "SKILL.md";
/// 同步状态文件名后缀:存于库目录**之外**(root 的父目录),不入 git,
/// 否则同步一结束工作区立即变脏
pub const STATE_FILE_SUFFIX: &str = "-state.json";

/// 技能目录名安全规则:1~64 字符,首字符字母数字或下划线,余下仅字母数字与 - _ .
/// (禁止路径分隔符/`.` 开头/`.` `..`,防目录穿越与隐藏目录)
pub fn is_safe_directory(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphanumeric() || first == '_')
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// 全模块互斥:串行化库数据读写(短临界区,不含网络操作)
static OP_LOCK: Mutex<()> = Mutex::new(());

pub fn lock_op() -> MutexGuard<'static, ()> {
    OP_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// 全局资源库:所有命令经 `Library::app` 拿到 `~/.repomeow/resource-library`
#[derive(Clone, Debug)]
pub struct Library {
    root: PathBuf,
}

impl Library {
    /// 应用级资源库根目录(`~/.repomeow/resource-library`)
    pub fn app(app: &AppHandle) -> RlResult<Self> {
        let home = app
            .path()
            .home_dir()
            .map_err(|e| RlError::coded("io_error", e.to_string()))?;
        Ok(Self::new(
            home.join(APP_DATA_DIR_NAME).join(LIBRARY_DIR_NAME),
        ))
    }

    /// 任意根目录(测试用)
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 目录是否存在且非空(import 冲突判定用)
    #[cfg(test)]
    pub fn has_content(&self) -> bool {
        fs::read_dir(&self.root)
            .map(|mut it| it.next().is_some())
            .unwrap_or(false)
    }

    /// 建目录并播种默认文件(library.json / skills.json / mcp.json / skills/)
    pub fn ensure(&self) -> RlResult<()> {
        fs::create_dir_all(&self.root)?;
        fs::create_dir_all(self.root.join(DIR_SKILLS))?;
        if !self.root.join(FILE_LIBRARY).exists() {
            self.write_meta(&LibraryMeta::default())?;
        }
        if !self.root.join(FILE_SKILLS).exists() {
            self.write_plain_json(FILE_SKILLS, &SkillLibrary::default())?;
        }
        if !self.root.join(FILE_MCP).exists() {
            self.write_plain_json(FILE_MCP, &Vec::<McpServer>::new())?;
        }
        Ok(())
    }

    // ── meta(library.json,恒为明文,git 跟踪)───────────────────────────

    pub fn meta(&self) -> RlResult<LibraryMeta> {
        let bytes = fs::read(self.root.join(FILE_LIBRARY))?;
        let text = std::str::from_utf8(&bytes).map_err(|e| RlError::corrupt(FILE_LIBRARY, e))?;
        serde_json::from_str(text).map_err(|e| RlError::corrupt(FILE_LIBRARY, e))
    }

    pub fn write_meta(&self, meta: &LibraryMeta) -> RlResult<()> {
        let mut text =
            serde_json::to_string_pretty(meta).map_err(|e| RlError::corrupt(FILE_LIBRARY, e))?;
        text.push('\n');
        self.atomic_write(&self.root.join(FILE_LIBRARY), text.as_bytes())
    }

    // ── 同步状态(仓库外,不入 git)──────────────────────────────────────

    /// 状态文件路径:`<库目录父目录>/<库目录名>-state.json`
    /// (应用侧即 `~/.repomeow/resource-library-state.json`)
    pub fn state_path(&self) -> PathBuf {
        let name = self
            .root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(LIBRARY_DIR_NAME);
        self.root
            .parent()
            .unwrap_or(&self.root)
            .join(format!("{name}{STATE_FILE_SUFFIX}"))
    }

    /// 读取同步状态;文件缺失/损坏时容错回默认(状态仅为提示性)
    pub fn read_state(&self) -> LibraryState {
        let path = self.state_path();
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                eprintln!("[resource-library] 状态文件解析失败({e}),回退默认");
                LibraryState::default()
            }),
            Err(_) => LibraryState::default(),
        }
    }

    /// 记录最近一次同步结果(写仓库外状态文件,不触碰 git 跟踪内容)
    pub fn record_sync(&self, record: &super::models::SyncRecord) -> RlResult<()> {
        let state = LibraryState {
            last_sync: Some(record.clone()),
        };
        let mut text =
            serde_json::to_string_pretty(&state).map_err(|e| RlError::corrupt("state.json", e))?;
        text.push('\n');
        self.atomic_write(&self.state_path(), text.as_bytes())
    }

    // ── 明文 JSON(skills.json;加密上锁后仍可读写)──────────────────────

    pub fn read_plain_json<T: DeserializeOwned>(&self, rel: &str) -> RlResult<T> {
        let bytes = fs::read(self.root.join(rel))?;
        let text = std::str::from_utf8(&bytes).map_err(|e| RlError::corrupt(rel, e))?;
        serde_json::from_str(text).map_err(|e| RlError::corrupt(rel, e))
    }

    pub fn write_plain_json<T: Serialize>(&self, rel: &str, value: &T) -> RlResult<()> {
        let mut text = serde_json::to_string_pretty(value).map_err(|e| RlError::corrupt(rel, e))?;
        text.push('\n');
        self.atomic_write(&self.root.join(rel), text.as_bytes())
    }

    // ── 加密 JSON(仅 mcp.json)─────────────────────────────────────────

    /// 读取 mcp.json;加密未解锁时返回 `resource_library_locked`
    pub fn read_mcp_json<T: DeserializeOwned>(&self) -> RlResult<T> {
        let bytes = fs::read(self.root.join(FILE_MCP))?;
        let key = if self.meta()?.encrypted {
            key_for(&self.root)
        } else {
            None
        };
        let plain = decrypt_bytes(key.as_deref(), &bytes)?;
        let text = std::str::from_utf8(&plain).map_err(|e| RlError::corrupt(FILE_MCP, e))?;
        serde_json::from_str(text).map_err(|e| RlError::corrupt(FILE_MCP, e))
    }

    /// 写 mcp.json;加密开启时写入容器字节(未解锁报 locked)
    pub fn write_mcp_json<T: Serialize>(&self, value: &T) -> RlResult<()> {
        let mut text =
            serde_json::to_string_pretty(value).map_err(|e| RlError::corrupt(FILE_MCP, e))?;
        text.push('\n');
        let out = if self.meta()?.encrypted {
            let key = key_for(&self.root).ok_or_else(|| RlError::coded(codes::LOCKED, ""))?;
            encrypt_bytes(&key, text.as_bytes())?
        } else {
            text.into_bytes()
        };
        self.atomic_write(&self.root.join(FILE_MCP), &out)
    }

    // ── 技能正文(skills/<directory>/SKILL.md,恒明文)───────────────────

    /// 正文路径;directory 必须是安全目录名(调用方校验后传入)
    pub fn body_path(&self, directory: &str) -> RlResult<PathBuf> {
        if !is_safe_directory(directory) {
            return Err(RlError::coded(
                codes::DIRECTORY_INVALID,
                directory.to_string(),
            ));
        }
        Ok(self
            .root
            .join(DIR_SKILLS)
            .join(directory)
            .join(SKILL_BODY_FILE))
    }

    pub fn read_body(&self, directory: &str) -> RlResult<Vec<u8>> {
        let path = self.body_path(directory)?;
        match fs::read(&path) {
            Ok(data) => Ok(data),
            // 未写正文 = 空内容(技能创建时可不带正文)
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn write_body(&self, directory: &str, content: &str) -> RlResult<()> {
        let path = self.body_path(directory)?;
        self.atomic_write(&path, content.as_bytes())
    }

    /// 技能目录重命名(directory 变化时迁移正文)
    pub fn rename_skill_dir(&self, from: &str, to: &str) -> RlResult<()> {
        let from_path = self.root.join(DIR_SKILLS).join(from);
        let to_path = self.root.join(DIR_SKILLS).join(to);
        if !from_path.exists() {
            return Ok(());
        }
        fs::create_dir_all(to_path.parent().expect("skills 目录恒有父级"))?;
        fs::rename(&from_path, &to_path)?;
        Ok(())
    }

    pub fn remove_skill_dir(&self, directory: &str) -> RlResult<()> {
        let dir = self.root.join(DIR_SKILLS).join(directory);
        remove_dir_tolerating_readonly(&dir)
    }

    // ── 加密批量变换(仅 mcp.json)──────────────────────────────────────

    /// 对 mcp.json 做字节变换(读取→变换→原子写),用于批量加密/解密;缺失容忍
    pub fn transform_encrypted_file(&self, f: impl Fn(&[u8]) -> RlResult<Vec<u8>>) -> RlResult<()> {
        let path = self.root.join(FILE_MCP);
        if !path.exists() {
            return Ok(());
        }
        let data = fs::read(&path)?;
        let out = f(&data)?;
        self.atomic_write(&path, &out)
    }

    // ── 备份 / 整库清除 ────────────────────────────────────────────────

    /// 整库备份目录(父目录下):`<库目录名>.backup-<ts>`
    #[cfg(test)]
    pub fn backup_dir(&self, ts: i64) -> PathBuf {
        let name = self
            .root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(LIBRARY_DIR_NAME);
        self.root
            .parent()
            .unwrap_or(&self.root)
            .join(format!("{name}.backup-{ts}"))
    }

    #[cfg(test)]
    pub fn clear_all(&self) -> RlResult<()> {
        clear_key(&self.root);
        if self.root.exists() {
            remove_dir_tolerating_readonly(&self.root)?;
        }
        fs::create_dir_all(&self.root)?;
        Ok(())
    }

    // ── 原子写(backup/ReplaceFile 语义)───────────────────────────────

    fn atomic_write(&self, target: &Path, bytes: &[u8]) -> RlResult<()> {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let ext = target
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!("{e}."))
            .unwrap_or_default();
        let tmp = target.with_extension(format!("{ext}repomeow-tmp"));
        fs::write(&tmp, bytes)?;
        // Windows 上 fs::rename 对已存在/只读/占用目标会失败(REPLACE_EXISTING 语义不稳):
        // 先备份旧文件再替换,替换失败回滚旧文件,成功删除备份
        #[cfg(windows)]
        {
            let backup = target.with_extension(format!("{ext}repomeow-bak"));
            let _ = fs::remove_file(&backup);
            if target.exists() {
                if let Err(e) = fs::rename(target, &backup) {
                    let _ = fs::remove_file(&tmp);
                    return Err(e.into());
                }
            }
            if let Err(e) = fs::rename(&tmp, target) {
                let _ = fs::rename(&backup, target);
                return Err(e.into());
            }
            let _ = fs::remove_file(&backup);
            Ok(())
        }
        #[cfg(not(windows))]
        {
            fs::rename(&tmp, target).map_err(Into::into)
        }
    }
}

#[cfg(windows)]
fn clear_readonly_recursive(root: &Path) {
    fn visit(path: &Path) {
        let Ok(meta) = fs::metadata(path) else {
            return;
        };
        if meta.is_dir() {
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    visit(&entry.path());
                }
            }
        }
        let mut perms = meta.permissions();
        if perms.readonly() {
            perms.set_readonly(false);
            let _ = fs::set_permissions(path, perms);
        }
    }
    visit(root);
}

#[cfg(not(windows))]
fn clear_readonly_recursive(_root: &Path) {}

/// 删除目录;Windows 下 git 对象文件只读,先清只读位再删(照抄 wiki 语义)
pub fn remove_dir_tolerating_readonly(dir: &Path) -> RlResult<()> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => {
            clear_readonly_recursive(dir);
            match fs::remove_dir_all(dir) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::ai::resource_library::crypto::{derive_key, new_salt};
    use crate::time_util::now_ts_nanos;

    fn temp_lib(name: &str) -> (Library, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "repomeow-rl-store-{name}-{}-{}",
            std::process::id(),
            now_ts_nanos()
        ));
        let lib = Library::new(root.clone());
        lib.ensure().unwrap();
        (lib, root)
    }

    #[test]
    fn ensure_seeds_plaintext_defaults() {
        let (lib, root) = temp_lib("seed");
        let meta = lib.meta().unwrap();
        assert_eq!(meta.version, 1);
        assert!(!meta.encrypted);
        let skills: SkillLibrary = lib.read_plain_json(FILE_SKILLS).unwrap();
        assert!(skills.groups.is_empty() && skills.skills.is_empty());
        let mcp: Vec<McpServer> = lib.read_mcp_json().unwrap();
        assert!(mcp.is_empty());
        assert!(root.join(DIR_SKILLS).is_dir());
    }

    #[test]
    fn safe_directory_rules() {
        assert!(is_safe_directory("sk_ab12"));
        assert!(is_safe_directory("my-skill.v2"));
        assert!(is_safe_directory("_x"));
        assert!(!is_safe_directory(""));
        assert!(!is_safe_directory("."));
        assert!(!is_safe_directory(".."));
        assert!(!is_safe_directory(".hidden"));
        assert!(!is_safe_directory("a/b"));
        assert!(!is_safe_directory("a\\b"));
        assert!(!is_safe_directory("空格"));
        assert!(!is_safe_directory(&"x".repeat(65)));
    }

    #[test]
    fn state_file_lives_outside_repo_and_records_sync() {
        let (lib, root) = temp_lib("state");
        let state_path = lib.state_path();
        assert_eq!(state_path.parent().unwrap(), root.parent().unwrap());
        assert_ne!(state_path, root.join("library.json"));
        // 初始无文件 → 默认
        assert!(lib.read_state().last_sync.is_none());
        let record = super::super::models::SyncRecord {
            at: 123,
            ok: true,
            error_code: None,
            error_message: None,
            ahead: 1,
            behind: 0,
            diverged: false,
        };
        lib.record_sync(&record).unwrap();
        assert!(state_path.exists());
        assert_eq!(lib.read_state().last_sync.unwrap().at, 123);
        // 损坏容错
        fs::write(&state_path, b"{broken").unwrap();
        assert!(lib.read_state().last_sync.is_none());
    }

    #[test]
    fn atomic_write_replaces_existing_file_repeatedly() {
        let (lib, root) = temp_lib("atomic");
        for i in 0..5 {
            let data: SkillLibrary = serde_json::from_str(&format!(
                r#"{{"groups":[],"skills":[{{"id":"s{i}","directory":"s{i}","name":"第{i}次","groupIds":[],"sortOrder":0,"createdAt":1,"updatedAt":1}}]}}"#
            ))
            .unwrap();
            lib.write_plain_json(FILE_SKILLS, &data).unwrap();
        }
        let back: SkillLibrary = lib.read_plain_json(FILE_SKILLS).unwrap();
        assert_eq!(back.skills[0].name, "第4次");
        // 无 tmp/bak 残留
        for entry in fs::read_dir(&root).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            assert!(!name.contains("repomeow-tmp") && !name.contains("repomeow-bak"));
        }
    }

    #[test]
    fn encrypted_only_mcp_json_and_skills_stay_plain() {
        let (lib, root) = temp_lib("enc");
        let salt = new_salt();
        let key = derive_key("p@ss", &salt).unwrap();
        // 批量加密只作用于 mcp.json
        lib.transform_encrypted_file(|bytes| encrypt_bytes(&key, bytes))
            .unwrap();
        assert!(super::super::crypto::is_container(
            &fs::read(root.join(FILE_MCP)).unwrap()
        ));
        assert!(!super::super::crypto::is_container(
            &fs::read(root.join(FILE_SKILLS)).unwrap()
        ));
        // library.json 恒为明文
        assert!(!super::super::crypto::is_container(
            &fs::read(root.join(FILE_LIBRARY)).unwrap()
        ));
    }

    #[test]
    fn body_roundtrip_via_directory_plaintext() {
        let (lib, root) = temp_lib("body");
        lib.write_body("sk1", "# 你好\n技能正文").unwrap();
        assert_eq!(
            String::from_utf8(lib.read_body("sk1").unwrap()).unwrap(),
            "# 你好\n技能正文"
        );
        assert_eq!(
            fs::read_to_string(root.join("skills/sk1/SKILL.md")).unwrap(),
            "# 你好\n技能正文"
        );
        // 不存在的正文 = 空
        assert_eq!(lib.read_body("nope").unwrap(), b"");
        // 非法目录名拒绝
        assert!(lib.write_body("../evil", "x").is_err());
    }

    #[test]
    fn mcp_json_write_encrypts_while_locked_skills_remain_editable() {
        let (lib, root) = temp_lib("mcpenc");
        let salt = new_salt();
        let key = derive_key("p@ss", &salt).unwrap();
        let mut meta = lib.meta().unwrap();
        meta.encrypted = true;
        lib.write_meta(&meta).unwrap();
        super::super::crypto::store_key(&root, key);
        lib.write_mcp_json(&Vec::<McpServer>::new()).unwrap();
        super::super::crypto::clear_key(&root);
        // mcp.json 为容器,skills.json 明文
        assert!(super::super::crypto::is_container(
            &fs::read(root.join(FILE_MCP)).unwrap()
        ));
        assert!(!super::super::crypto::is_container(
            &fs::read(root.join(FILE_SKILLS)).unwrap()
        ));
        // 上锁:技能读写照常,MCP 读写报 locked
        let err = lib.read_mcp_json::<Vec<McpServer>>().unwrap_err();
        assert_eq!(err.code(), codes::LOCKED);
        lib.write_plain_json(FILE_SKILLS, &SkillLibrary::default())
            .unwrap();
        lib.write_body("s1", "正文").unwrap();
        assert!(lib.read_body("s1").unwrap() == "正文".as_bytes());
        // 无 tmp/bak 残留
        for entry in fs::read_dir(&root).unwrap().flatten() {
            assert!(!entry.file_name().to_string_lossy().contains("repomeow-tmp"));
        }
    }

    #[test]
    fn clear_all_removes_content_and_key() {
        let (lib, root) = temp_lib("clear");
        lib.write_body("sk1", "x").unwrap();
        super::super::crypto::store_key(&root, derive_key("p", &new_salt()).unwrap());
        lib.clear_all().unwrap();
        assert!(root.exists() && !root.join(DIR_SKILLS).join("sk1").exists());
        assert!(!super::super::crypto::is_unlocked(&root));
    }
}
