//! 资源库数据模型:serde 结构化类型。
//!
//! 磁盘布局(`~/.repomeow/resource-library/`):
//! - `library.json` — 元数据(恒为明文,git 同步/状态查询无需口令)
//! - `skills.json` — 分组与技能元数据(恒为明文;加密上锁后仍可管理)
//! - `mcp.json` — 通用 MCP 服务器定义(唯一加密文件)
//! - `skills/<directory>/SKILL.md` — 技能正文(恒为明文)
//! - `.git/` — 整个资源库目录本身是本地 git 仓库
//!
//! 同步状态存仓库外 `~/.repomeow/resource-library-state.json`(见 store.rs),
//! 不入 git,避免同步后工作区立即变脏。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// library.json 的格式版本;未发布前可原地演进
pub const LIBRARY_VERSION: u32 = 1;

/// 加密文件容器所用的密钥校验值(nonce + 密文,base64),存于明文 meta:
/// 解锁时先校验口令正确性,把「口令错误」与「数据文件损坏」区分开
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyCheck {
    pub nonce: String,
    pub ciphertext: String,
}

/// 最近一次自动同步的结果记录(网络失败不阻断本地保存,记录于此)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SyncRecord {
    pub at: i64,
    pub ok: bool,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub diverged: bool,
}

/// library.json(明文):版本、加密状态与 KDF 参数。
/// 历史版本曾内嵌 lastSync,现移至仓库外 state 文件;旧字段读取时被忽略
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LibraryMeta {
    pub version: u32,
    pub encrypted: bool,
    /// Argon2id 盐(b64);仅 encrypted=true 时存在
    pub kdf_salt: Option<String>,
    /// 口令校验值;仅 encrypted=true 时存在
    pub key_check: Option<KeyCheck>,
}

impl Default for LibraryMeta {
    fn default() -> Self {
        Self {
            version: LIBRARY_VERSION,
            encrypted: false,
            kdf_salt: None,
            key_check: None,
        }
    }
}

/// 仓库外同步状态文件(`<库目录名>-state.json`,不入 git)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LibraryState {
    pub last_sync: Option<SyncRecord>,
}

// ── Skill(多分组)──────────────────────────────────────────────────────

/// 技能分组(技能经 Skill.group_ids 多对多关联;分组可空、可删除,
/// 删除分组时自动从所有技能解除关联)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillGroup {
    pub id: String,
    pub name: String,
    /// 分组颜色(#RRGGBB,可选;仅前端展示用)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// 排序权重,数值小者在前;`reorder` 全量重排
    pub sort: u32,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 技能元数据;正文独立存储于 `skills/<directory>/SKILL.md`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub id: String,
    /// 正文目录名(skills/ 下的安全目录名,仅字母数字与 - _ .)
    pub directory: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// 来自 skills.sh 的市场条目标识与展示来源；手动创建的技能保持为空。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marketplace: Option<MarketplaceSource>,
    /// 所属分组(多分组;可为空 = 无分组)
    #[serde(default)]
    pub group_ids: Vec<String>,
    /// 技能排序权重,数值小者在前;`reorder` 全量重排
    #[serde(default)]
    pub sort_order: u32,
    pub created_at: i64,
    pub updated_at: i64,
}

/// skills.json:扁平结构,同一技能只存一份
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SkillLibrary {
    pub groups: Vec<SkillGroup>,
    pub skills: Vec<Skill>,
}

/// skills.sh 市场中单项 Skill 的稳定来源标识，随本地 Skill 同步保存。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceSource {
    pub id: String,
    pub source: String,
    pub url: String,
}

/// 市场卡片的最小展示数据。目录页面没有稳定描述字段，导入时以 SKILL.md frontmatter 为准。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceSkill {
    pub id: String,
    pub name: String,
    pub source: String,
    pub installs: u64,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_skill_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MarketplaceList {
    pub skills: Vec<MarketplaceSkill>,
}

// ── MCP ────────────────────────────────────────────────────────────────

/// MCP transport 判别符
pub const TRANSPORT_STDIO: &str = "stdio";
pub const TRANSPORT_HTTP: &str = "http";
pub const TRANSPORT_SSE: &str = "sse";
pub const TRANSPORTS: &[&str] = &[TRANSPORT_STDIO, TRANSPORT_HTTP, TRANSPORT_SSE];

fn default_true() -> bool {
    true
}

/// 通用 MCP 服务器定义(创建/更新入参)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct McpServerInput {
    pub name: String,
    pub description: Option<String>,
    /// `stdio` | `http` | `sse`
    pub transport: String,
    /// stdio:可执行文件;http/sse:远程地址
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub url: Option<String>,
    pub headers: HashMap<String, String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Rust 默认值与 serde 缺省值一致(enabled=true)
impl Default for McpServerInput {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: None,
            transport: String::new(),
            command: None,
            args: Vec::new(),
            env: HashMap::new(),
            url: None,
            headers: HashMap::new(),
            enabled: true,
        }
    }
}

/// 已持久化的 MCP 服务器(输入字段 + 库内标识与时间戳)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServer {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub transport: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub url: Option<String>,
    pub headers: HashMap<String, String>,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

// ── 查询结果 ───────────────────────────────────────────────────────────

/// 库信息(设置页/首次引导用;加密未解锁时 MCP 计数取 0)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryInfo {
    pub root: String,
    pub version: u32,
    pub encrypted: bool,
    /// 本次进程内是否已 unlock(口令仅内存)
    pub unlocked: bool,
    pub git_initialized: bool,
    pub git_dirty: bool,
    pub remote_url: Option<String>,
    pub branch: Option<String>,
    pub skill_count: u32,
    pub group_count: u32,
    pub mcp_count: u32,
    pub last_sync: Option<SyncRecord>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptionStatus {
    pub enabled: bool,
    pub unlocked: bool,
}

/// 单条被跳过的导入来源(SKILL.md 目录):name 取 frontmatter name,
/// 缺失时取目录名;reason 为稳定字符串码,由前端 i18n 映射文案
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillImportSkip {
    pub name: String,
    /// `conflict`(与现有技能重名)| `invalid`(缺 frontmatter name / 读取失败)
    pub reason: String,
}

/// 一次批量导入(压缩包 / 文件夹 / URL)的结果;部分成功语义:
/// 可导入的照常入库,重名或缺 name 的条目跳过并记入 skipped
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillImportOutcome {
    pub imported: Vec<Skill>,
    pub skipped: Vec<SkillImportSkip>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillBody {
    pub id: String,
    pub content: String,
}

/// 一次同步尝试的结果(自动同步与显式 `rl_sync_once` 共用;
/// 网络失败不外抛,记录在 error_code / error_message)
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncOutcome {
    pub ok: bool,
    pub fetched: bool,
    pub pulled: bool,
    pub pushed: bool,
    /// 若本次顺带做了快照提交,为提交短 hash
    pub committed: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub diverged: bool,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

/// 同步状态(rl_sync_status;配置了 remote 时先 fetch)
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub initialized: bool,
    pub remote: Option<String>,
    pub branch: Option<String>,
    pub dirty: bool,
    pub ahead: u32,
    pub behind: u32,
    pub diverged: bool,
    /// 远端分支已删除(fetch 找不到对应 ref)
    pub remote_gone: bool,
    pub last_sync: Option<SyncRecord>,
}

#[cfg(test)]
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub backup: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_defaults_to_version_1_unencrypted() {
        let meta = LibraryMeta::default();
        assert_eq!(meta.version, LIBRARY_VERSION);
        assert!(!meta.encrypted);
        assert!(meta.kdf_salt.is_none());
        assert!(meta.key_check.is_none());
    }

    #[test]
    fn old_library_json_with_last_sync_field_still_parses() {
        let json = r#"{"version":1,"encrypted":false,"lastSync":{"at":1,"ok":true}}"#;
        let meta: LibraryMeta = serde_json::from_str(json).unwrap();
        assert!(!meta.encrypted);
    }

    #[test]
    fn mcp_enabled_defaults_true_on_missing_field() {
        let def: McpServerInput =
            serde_json::from_str(r#"{"name":"x","transport":"stdio"}"#).unwrap();
        assert!(def.enabled);
        assert!(def.args.is_empty());
        assert!(def.env.is_empty());
    }

    #[test]
    fn sync_record_defaults_are_all_zero() {
        let r = SyncRecord::default();
        assert!(!r.ok && !r.diverged);
        assert_eq!(r.ahead + r.behind, 0);
    }

    #[test]
    fn skill_library_flat_roundtrip() {
        let lib = SkillLibrary {
            groups: vec![SkillGroup {
                id: "g1".into(),
                name: "通用".into(),
                color: None,
                sort: 0,
                created_at: 1,
                updated_at: 1,
            }],
            skills: vec![Skill {
                id: "s1".into(),
                directory: "s1".into(),
                name: "审查代码".into(),
                description: String::new(),
                marketplace: None,
                group_ids: vec!["g1".into()],
                sort_order: 2,
                created_at: 1,
                updated_at: 1,
            }],
        };
        let json = serde_json::to_string(&lib).unwrap();
        let back: SkillLibrary = serde_json::from_str(&json).unwrap();
        assert_eq!(back.skills[0].group_ids, vec!["g1"]);
        assert_eq!(back.skills[0].sort_order, 2);
        assert_eq!(back.groups.len(), 1);
    }
}
