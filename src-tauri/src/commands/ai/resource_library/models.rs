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
    /// 分组描述(可选;空串 = 未填写,仅前端展示用)
    #[serde(default)]
    pub description: String,
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
#[serde(rename_all = "camelCase", default)]
pub struct MarketplaceSource {
    pub id: String,
    pub source: String,
    pub url: String,
    /// 技能在 GitHub 仓库内的目录（仓库相对路径，`/` 分隔）；空串 = 仓库根
    pub repo_dir: String,
    /// 安装/最近一次更新时该目录的 GitHub commit sha；空 = 未知（安装时 GitHub 不可用或旧数据）
    pub installed_sha: Option<String>,
    /// 最近一次从市场安装/更新的时间戳
    pub installed_at: Option<i64>,
}

impl Default for MarketplaceSource {
    fn default() -> Self {
        Self {
            id: String::new(),
            source: String::new(),
            url: String::new(),
            repo_dir: String::new(),
            installed_sha: None,
            installed_at: None,
        }
    }
}

/// 市场下载结果:SKILL.md 所在目录下的全部文件,`path` 为相对该目录的
/// 安全相对路径(`/` 分隔);`repo_dir` 是该目录在 GitHub 仓库内的路径,根级为空串
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceFile {
    pub path: String,
    pub contents: String,
}

#[derive(Debug, Clone)]
pub struct MarketplaceDownload {
    pub files: Vec<MarketplaceFile>,
    pub skill_md: String,
    pub repo_dir: String,
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

/// 单个已安装市场技能的更新检查结果;`update_available` 为 None 表示无法判断
/// (GitHub 查询失败 / 旧数据且内容比对未完成),error_code 为稳定字符串码
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceUpdateStatus {
    pub skill_id: String,
    pub marketplace_id: String,
    pub update_available: Option<bool>,
    pub error_code: Option<String>,
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

/// 单条被跳过的 MCP 导入条目;reason 为稳定码(conflict/invalid)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpImportSkip {
    pub name: String,
    /// `conflict`(与现有服务器或批内条目重名)| `invalid`(校验失败)
    pub reason: String,
}

/// 一次批量导入(JSON 粘贴)的结果;部分成功语义:可导入的照常入库,
/// 重名或校验失败的条目跳过并记入 skipped
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpImportOutcome {
    pub imported: Vec<McpServer>,
    pub skipped: Vec<McpImportSkip>,
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

// ── Skill 预览:token 统计与安全扫描 ───────────────────────────────────

/// 技能目录内单文件的 token 统计
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillTokenFile {
    /// 相对技能目录的路径(`/` 分隔);`SKILL.md` 为正文
    pub path: String,
    /// None = 二进制/非 UTF-8 文件,不参与 token 统计
    pub tokens: Option<i64>,
    pub bytes: u64,
}

/// 技能 token 统计(rl_skill_tokens;与项目 AI 资产同口径的 o200k 估算)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillTokenReport {
    pub id: String,
    /// 技能描述(skills.json / frontmatter 同步值)的 token 占用
    pub description_tokens: i64,
    /// 全部文本文件 token 合计(二进制文件不计入)
    pub total_tokens: i64,
    /// SKILL.md 在首位,其余按路径排序;含二进制文件(tokens 为 null)
    pub files: Vec<SkillTokenFile>,
    /// 技能内容指纹(描述 + 全部文件路径与字节的 FNV-1a);前端扫描结果
    /// 缓存据此判断是否仍有效
    pub hash: String,
}

/// 本地技能目录报告(skill_dir_overview;项目内非托管技能预览,不经过资源库)。
/// 名称/描述取自 SKILL.md frontmatter,名称缺失时回退目录名
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDirReport {
    pub name: String,
    pub description: String,
    pub description_tokens: i64,
    pub total_tokens: i64,
    pub files: Vec<SkillTokenFile>,
    /// 与 SkillTokenReport 同口径的内容指纹(描述 + 全部文件路径与字节)
    pub hash: String,
}

/// 单个技能文件的内容(rl_skill_file_read)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillFileContent {
    pub path: String,
    /// None = 二进制/非 UTF-8 文件或超出预览大小上限,无法以文本预览
    pub content: Option<String>,
}

/// 单条安全发现。静态发现:标题/说明为空,前端按 rule_id 走 i18n;
/// AI 发现:标题/说明直接用模型输出(已按界面语言要求生成)。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillScanFinding {
    /// `critical` | `high` | `medium` | `low`
    pub severity: String,
    /// 固定类别 id,前端 i18n 映射;未知类别为 `other`
    pub category: String,
    /// 静态规则 id(仅静态发现)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
    pub title: String,
    pub detail: String,
    /// 位置(`文件路径:行号` 或自由描述)
    pub location: String,
    /// `static` = 本地规则命中;`llm` = AI 语义分析
    pub source: String,
    /// 命中行内容(仅静态发现)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

/// 技能安全扫描报告(rl_skill_scan;两层管线合并后的最终结果)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillScanReport {
    pub skill_id: String,
    /// 0-100,含可执行脚本 ×1.3
    pub score: i32,
    /// `low` | `medium` | `high` | `critical`
    pub level: String,
    /// 静态发现(去掉 AI 判定的误报)+ AI 发现
    pub findings: Vec<SkillScanFinding>,
    /// 静态命中总数(含被 AI 过滤的误报)
    pub static_count: usize,
    /// 被 AI 判定为误报而过滤的静态命中数
    pub suppressed_count: usize,
    pub files_scanned: usize,
    /// `ok` | `skipped`(AI 未配置)| `canceled` | `failed`
    pub llm_status: String,
    pub llm_error_code: Option<String>,
    pub llm_error_message: Option<String>,
    pub llm_summary: Option<String>,
    pub scanned_at: i64,
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
                description: "日常使用".into(),
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
