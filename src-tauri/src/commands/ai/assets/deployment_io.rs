//! 项目配置 IO:拒绝链接/重解析点、目录外路径与损坏配置;所有写入先暂存。
use super::super::resource_library::{RlError, RlResult};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

pub(super) fn problem(message: impl Into<String>) -> RlError {
    RlError::coded("project_ai_conflict", message)
}

fn is_link(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_type().is_symlink() || meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

pub(super) fn safe_path(root: &Path, relative: &str) -> RlResult<PathBuf> {
    if relative.is_empty() {
        return Err(problem("empty path"));
    }
    let mut target = root.to_path_buf();
    for component in Path::new(relative).components() {
        let Component::Normal(part) = component else {
            return Err(problem(relative));
        };
        if part.to_string_lossy().contains(':') {
            return Err(problem(relative));
        }
        target.push(part);
        match fs::symlink_metadata(&target) {
            Ok(meta) if is_link(&meta) => {
                return Err(problem(format!("link: {}", target.display())))
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    if !target.starts_with(root) || target == root {
        return Err(problem(relative));
    }
    Ok(target)
}

pub(super) fn hash(bytes: &[u8]) -> String {
    // git 的稳定内容哈希,不把 MCP 密钥写入部署记录。
    git2::Oid::hash_object(git2::ObjectType::Blob, bytes)
        .expect("blob hash")
        .to_string()
}

pub(super) fn json_hash(value: &Value) -> String {
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let sorted: BTreeMap<_, _> =
                    map.iter().map(|(k, v)| (k.clone(), canonical(v))).collect();
                Value::Object(sorted.into_iter().collect())
            }
            Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
            value => value.clone(),
        }
    }
    hash(canonical(value).to_string().as_bytes())
}

pub(super) struct SkillFile {
    pub bytes: Vec<u8>,
    pub permissions: fs::Permissions,
}
pub(super) type SkillTree = BTreeMap<String, SkillFile>;

pub(super) fn read_tree(root: &Path) -> RlResult<SkillTree> {
    fn walk(
        root: &Path,
        dir: &Path,
        depth: usize,
        size: &mut u64,
        out: &mut SkillTree,
    ) -> RlResult<()> {
        if depth > 32 {
            return Err(problem("skill directory too deep"));
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let meta = fs::symlink_metadata(&path)?;
            if is_link(&meta) {
                return Err(problem(format!("link: {}", path.display())));
            }
            if meta.is_dir() {
                walk(root, &path, depth + 1, size, out)?;
            } else if meta.is_file() {
                *size += meta.len();
                if *size > 64 * 1024 * 1024 || out.len() >= 10_000 {
                    return Err(problem("skill exceeds 64 MiB / 10000 files"));
                }
                let rel = crate::path_util::to_forward_slash(
                    path.strip_prefix(root)
                        .map_err(|e| problem(e.to_string()))?,
                );
                out.insert(
                    rel,
                    SkillFile {
                        bytes: fs::read(path)?,
                        permissions: meta.permissions(),
                    },
                );
            } else {
                return Err(problem("unsupported file type"));
            }
        }
        Ok(())
    }
    if is_link(&fs::symlink_metadata(root)?) {
        return Err(problem("linked skill directory"));
    }
    let mut tree = BTreeMap::new();
    walk(root, root, 0, &mut 0, &mut tree)?;
    Ok(tree)
}

pub(super) fn tree_hash(tree: &SkillTree) -> String {
    let mut bytes = Vec::new();
    for (name, file) in tree {
        bytes.extend_from_slice(&(name.len() as u64).to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(&(file.bytes.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&file.bytes);
    }
    hash(&bytes)
}

pub(super) fn atomic_write(path: &Path, bytes: &[u8]) -> RlResult<()> {
    let parent = path.parent().ok_or_else(|| problem("missing parent"))?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".repomeow-{}.tmp",
        crate::time_util::now_ts_nanos()
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)?;
    use std::io::Write;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(Into::into)
}

/// 暂存目录与目标均位于已校验的父目录;旧目录先保留,提升失败时回滚。
pub(super) fn replace_tree(root: &Path, relative: &str, tree: Option<&SkillTree>) -> RlResult<()> {
    let target = safe_path(root, relative)?;
    let parent = target.parent().ok_or_else(|| problem(relative))?;
    fs::create_dir_all(parent)?;
    let nonce = crate::time_util::now_ts_nanos();
    let stage = parent.join(format!(".repomeow-stage-{nonce}"));
    let backup = parent.join(format!(".repomeow-backup-{nonce}"));
    if stage.exists() || backup.exists() {
        return Err(problem("temporary directory collision"));
    }
    if let Some(tree) = tree {
        fs::create_dir(&stage)?;
        let prepare: RlResult<()> = (|| {
            for (rel, file) in tree {
                let path = safe_path(&stage, rel)?;
                fs::create_dir_all(path.parent().unwrap())?;
                fs::write(&path, &file.bytes)?;
                fs::set_permissions(path, file.permissions.clone())?;
            }
            Ok(())
        })();
        if let Err(e) = prepare {
            let _ = remove_tree(root, &stage);
            return Err(e);
        }
    }
    let existed = target.exists();
    if existed {
        if let Err(e) = fs::rename(&target, &backup) {
            if tree.is_some() {
                let _ = remove_tree(root, &stage);
            }
            return Err(e.into());
        }
    }
    if tree.is_some() {
        if let Err(e) = fs::rename(&stage, &target) {
            if existed {
                fs::rename(&backup, &target)?;
            }
            let _ = remove_tree(root, &stage);
            return Err(e.into());
        }
    }
    // 旧文件已经移出 Agent 的加载路径;清理失败保留备份,不谎报配置未生效。
    if existed {
        if let Err(e) = remove_tree(root, &backup) {
            eprintln!("AI resource backup retained: {e}");
        }
    }
    Ok(())
}

fn remove_tree(root: &Path, path: &Path) -> RlResult<()> {
    let rel = path
        .strip_prefix(root)
        .map_err(|e| problem(e.to_string()))?;
    let checked = safe_path(root, &crate::path_util::to_forward_slash(rel))?;
    read_tree(&checked)?; // 递归删除前拒绝目录内链接。
    super::super::resource_library::store::remove_dir_tolerating_readonly(&checked)
}
