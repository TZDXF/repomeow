//! 多厂商 AI 接入配置(`~/.repomeow/ai-config.json`)。
//!
//! 格式对齐 pi 的 models.json(provider 列表 + 模型元数据),字段为 camelCase;
//! `api` 预留多 API 类型,当前仅支持 `openai-completions`。文件缺失时用
//! 内置目录(`builtin_models.json`)播种;播种时若旧版 `settings.json` 的
//! `aiBaseUrl/aiApiKey/aiModel` 三键仍在,额外合成一个「自定义」厂商完成
//! 迁移。文件损坏时备份后重新播种,绝不阻塞调用方。
//!
//! 消费方式:
//! - commit/report/wiki/测试连接:经 [`legacy_ai_config`] 投影成单一
//!   `AiConfig`(defaultModel 指向的厂商/模型),`sdk::load_config` 内部切换;
//! - 项目问答(chat):经 [`resolve_chat_prefs`] 取 chat 偏好(缺省回退
//!   defaultModel),再经 [`resolve_model`] 得到填全元数据的 `Model`。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;

use crate::agent::llm::types::{
    InputKind, Model, ModelCost, ModelCostRates, ModelThinkingLevel, OpenAICompletionsCompat,
    API_OPENAI_COMPLETIONS,
};
use crate::app_data_dir;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::time_util::now_ts;

/// 配置文件名(位于 `~/.repomeow/` 下)。
pub const AI_CONFIG_FILE_NAME: &str = "ai-config.json";
const AI_CONFIG_VERSION: u32 = 1;
const BUILTIN_CONFIG: &str = include_str!("builtin_models.json");

// ── 配置结构(camelCase,与 pi models.json 同族) ─────────────────────

/// 顶层配置。`defaultModel` 是 commit/report/wiki/测试连接使用的默认模型;
/// `chat` 是问答面板的全局偏好。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfigFile {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub providers: BTreeMap<String, AiProvider>,
    #[serde(default)]
    pub default_model: Option<ModelRef>,
    #[serde(default)]
    pub chat: ChatPrefs,
}

fn default_version() -> u32 {
    AI_CONFIG_VERSION
}

fn default_api() -> String {
    API_OPENAI_COMPLETIONS.to_string()
}

fn default_thinking() -> String {
    "off".to_string()
}

/// 一个厂商(OpenAI 兼容端点)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiProvider {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_api")]
    pub api: String,
    #[serde(default)]
    pub models: Vec<AiModelDef>,
}

/// 厂商下的模型定义(元数据用于上下文占用、思考参数与成本计算)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiModelDef {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub reasoning: bool,
    /// 空列表按 `[Text]` 处理。
    #[serde(default)]
    pub input: Vec<InputKind>,
    #[serde(default)]
    pub context_window: i64,
    #[serde(default)]
    pub max_tokens: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<ModelCost>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level_map: Option<BTreeMap<String, Option<String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compat: Option<OpenAICompletionsCompat>,
}

/// 模型引用:厂商 id + 模型 id。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRef {
    pub provider_id: String,
    pub model_id: String,
}

/// 问答面板的全局偏好。`thinking` 取值:
/// `off/minimal/low/medium/high/xhigh/max`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatPrefs {
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default = "default_thinking")]
    pub thinking: String,
    #[serde(default)]
    pub permission: ChatPermission,
}

impl Default for ChatPrefs {
    fn default() -> Self {
        Self {
            provider_id: None,
            model_id: None,
            thinking: default_thinking(),
            permission: ChatPermission::default(),
        }
    }
}

/// 问答工具权限:
/// - `all`:全部工具直接执行;
/// - `ask`:全部工具可用,但五个有副作用工具(`update_wiki` /
///   `regenerate_wiki` / `add_custom_command` / `generate_report` /
///   `set_wiki_model`)执行前由应用弹出硬确认(见 `commands/chat.rs` 的
///   before_tool_call 门禁)。
///
/// 旧值 `readOnly` 反序列化为 Ask(全部工具 + 执行前确认),保证旧配置平滑升级。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChatPermission {
    #[default]
    All,
    #[serde(alias = "readOnly")]
    Ask,
}

// ── 读写(原子写;缺/坏 → 播种) ──────────────────────────────────────

pub fn config_path(app: &AppHandle) -> AppResult<PathBuf> {
    app_data_dir(app).map(|dir| dir.join(AI_CONFIG_FILE_NAME))
}

pub fn load_ai_config_file(app: &AppHandle) -> AiConfigFile {
    match app_data_dir(app) {
        Ok(dir) => load_ai_config_file_at(&dir),
        Err(error) => {
            eprintln!("[ai-catalog] 解析应用数据目录失败,使用内置目录: {error}");
            builtin_config()
        }
    }
}

pub fn save_ai_config_file(app: &AppHandle, config: &AiConfigFile) -> AppResult<()> {
    let dir = app_data_dir(app)?;
    save_ai_config_file_at(&dir, config)
}

/// 读取配置;缺失或损坏时播种(含旧 settings.json 键迁移)并尽力落盘。
pub fn load_ai_config_file_at(data_dir: &Path) -> AiConfigFile {
    let path = data_dir.join(AI_CONFIG_FILE_NAME);
    let loaded = match fs::read_to_string(&path) {
        Ok(raw) => match serde_json::from_str::<AiConfigFile>(&raw) {
            Ok(mut config) => {
                normalize(&mut config);
                config
            }
            Err(error) => {
                eprintln!("[ai-catalog] {AI_CONFIG_FILE_NAME} 解析失败({error}),备份后重新播种");
                backup_corrupt(&path);
                seed_at(data_dir)
            }
        },
        Err(_) => seed_at(data_dir),
    };
    loaded
}

pub fn save_ai_config_file_at(data_dir: &Path, config: &AiConfigFile) -> AppResult<()> {
    let mut config = config.clone();
    normalize(&mut config);
    let path = data_dir.join(AI_CONFIG_FILE_NAME);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            AppError::coded(ErrorCode::IoError, format!("{}: {e}", parent.display()))
        })?;
    }
    let body = serde_json::to_string_pretty(&config)
        .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))?;
    let tmp = data_dir.join(format!("{AI_CONFIG_FILE_NAME}.tmp"));
    fs::write(&tmp, body)
        .map_err(|e| AppError::coded(ErrorCode::IoError, format!("{}: {e}", tmp.display())))?;
    fs::rename(&tmp, &path)
        .map_err(|e| AppError::coded(ErrorCode::IoError, format!("{}: {e}", path.display())))?;
    Ok(())
}

/// 播种:内置目录 + 旧 settings.json 三键迁移(三键齐备时合成「自定义」厂商)。
fn seed_at(data_dir: &Path) -> AiConfigFile {
    let mut config = builtin_config();
    let legacy = read_legacy_settings(data_dir);
    let base_url = legacy
        .get("aiBaseUrl")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let api_key = legacy
        .get("aiApiKey")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let model = legacy
        .get("aiModel")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if !base_url.is_empty() && !api_key.is_empty() && !model.is_empty() {
        config.providers.insert(
            LEGACY_PROVIDER_ID.to_string(),
            AiProvider {
                name: "自定义".to_string(),
                base_url: base_url.trim_end_matches('/').to_string(),
                api_key: api_key.to_string(),
                api: API_OPENAI_COMPLETIONS.to_string(),
                models: vec![AiModelDef {
                    id: model.to_string(),
                    name: String::new(),
                    reasoning: false,
                    input: vec![InputKind::Text],
                    context_window: 0,
                    max_tokens: 0,
                    cost: None,
                    thinking_level_map: None,
                    compat: None,
                }],
            },
        );
        config.default_model = Some(ModelRef {
            provider_id: LEGACY_PROVIDER_ID.to_string(),
            model_id: model.to_string(),
        });
        config.chat = ChatPrefs {
            provider_id: Some(LEGACY_PROVIDER_ID.to_string()),
            model_id: Some(model.to_string()),
            thinking: default_thinking(),
            permission: ChatPermission::default(),
        };
    }
    normalize(&mut config);
    if let Err(error) = save_ai_config_file_at(data_dir, &config) {
        eprintln!("[ai-catalog] 播种 {AI_CONFIG_FILE_NAME} 失败: {error}");
    }
    config
}

fn backup_corrupt(path: &Path) {
    let backup = path.with_extension(format!("json.corrupt-{}", now_ts()));
    if let Err(error) = fs::rename(path, &backup) {
        eprintln!("[ai-catalog] 备份损坏配置失败: {error}");
    }
}

/// 内置目录(静态资源,解析失败回退空配置)。
pub fn builtin_config() -> AiConfigFile {
    serde_json::from_str::<AiConfigFile>(BUILTIN_CONFIG).unwrap_or_default()
}

/// 旧 settings.json 的原始键(迁移用)。
fn read_legacy_settings(data_dir: &Path) -> serde_json::Value {
    fs::read_to_string(data_dir.join("settings.json"))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

// ── 归一化 ───────────────────────────────────────────────────────────

/// 迁移旧单配置的厂商 id。
pub const LEGACY_PROVIDER_ID: &str = "custom";

/// 去空白、剔除空厂商/空模型、去重,并清掉悬空的 default/chat 引用。
pub fn normalize(config: &mut AiConfigFile) {
    config.version = AI_CONFIG_VERSION;
    let mut providers = BTreeMap::new();
    for (id, mut provider) in std::mem::take(&mut config.providers) {
        let id = id.trim().to_string();
        if id.is_empty() {
            continue;
        }
        provider.name = provider.name.trim().to_string();
        provider.base_url = provider.base_url.trim().trim_end_matches('/').to_string();
        provider.api_key = provider.api_key.trim().to_string();
        if provider.api.trim() != API_OPENAI_COMPLETIONS {
            provider.api = API_OPENAI_COMPLETIONS.to_string();
        }
        let mut models = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for mut model in provider.models {
            model.id = model.id.trim().to_string();
            if model.id.is_empty() || !seen.insert(model.id.clone()) {
                continue;
            }
            model.name = model.name.trim().to_string();
            if model.input.is_empty() {
                model.input = vec![InputKind::Text];
            }
            models.push(model);
        }
        provider.models = models;
        providers.insert(id, provider);
    }
    config.providers = providers;

    config.default_model = config
        .default_model
        .take()
        .filter(|reference| model_exists(config, &reference.provider_id, &reference.model_id));
    let chat_model_valid = config
        .chat
        .provider_id
        .as_deref()
        .zip(config.chat.model_id.as_deref())
        .is_some_and(|(provider_id, model_id)| model_exists(config, provider_id, model_id));
    if !chat_model_valid {
        config.chat.provider_id = None;
        config.chat.model_id = None;
    }
    if !matches!(
        config.chat.thinking.as_str(),
        "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
    ) {
        config.chat.thinking = default_thinking();
    }
}

fn model_exists(config: &AiConfigFile, provider_id: &str, model_id: &str) -> bool {
    config
        .providers
        .get(provider_id)
        .is_some_and(|provider| provider.models.iter().any(|model| model.id == model_id))
}

// ── 解析 ─────────────────────────────────────────────────────────────

/// 解析 defaultModel 指向的完整模型与厂商密钥。
pub fn resolve_default_model(config: &AiConfigFile) -> AppResult<(Model, String)> {
    let reference = config
        .default_model
        .as_ref()
        .ok_or_else(|| AppError::coded(ErrorCode::AiNotConfigured, ""))?;
    let model = resolve_model(config, &reference.provider_id, &reference.model_id)?;
    let api_key = config
        .providers
        .get(&reference.provider_id)
        .map(|provider| provider.api_key.trim().to_string())
        .unwrap_or_default();
    if api_key.is_empty() {
        return Err(AppError::coded(ErrorCode::AiNotConfigured, ""));
    }
    Ok((model, api_key))
}

/// 解析 chat 偏好指向的模型:chat 未设置/失效时回退 defaultModel;
/// 都无效时返回 None(等价于「AI 尚未配置」)。
pub fn resolve_chat_prefs(config: &AiConfigFile) -> Option<(ModelRef, ChatPrefs)> {
    let chat = config.chat.clone();
    let reference = match (&chat.provider_id, &chat.model_id) {
        (Some(provider_id), Some(model_id)) if model_exists(config, provider_id, model_id) => {
            ModelRef {
                provider_id: provider_id.clone(),
                model_id: model_id.clone(),
            }
        }
        _ => config.default_model.clone()?,
    };
    Some((reference, chat))
}

/// 厂商 + 模型定义 → 填全元数据的 `Model`(替代原 `Model::from_settings`)。
pub fn resolve_model(config: &AiConfigFile, provider_id: &str, model_id: &str) -> AppResult<Model> {
    let provider = config.providers.get(provider_id).ok_or_else(|| {
        AppError::coded(
            ErrorCode::AiNotConfigured,
            format!("provider not found: {provider_id}"),
        )
    })?;
    if provider.api != API_OPENAI_COMPLETIONS {
        return Err(AppError::coded(
            ErrorCode::AiRequestFailed,
            format!(
                "暂不支持 AI API 类型「{}」,请使用 OpenAI 兼容接口",
                provider.api
            ),
        ));
    }
    let definition = provider
        .models
        .iter()
        .find(|model| model.id == model_id)
        .ok_or_else(|| {
            AppError::coded(
                ErrorCode::AiNotConfigured,
                format!("model not found: {provider_id}/{model_id}"),
            )
        })?;
    Ok(Model {
        id: definition.id.clone(),
        name: if definition.name.is_empty() {
            definition.id.clone()
        } else {
            definition.name.clone()
        },
        api: API_OPENAI_COMPLETIONS.to_string(),
        provider: provider_id.to_string(),
        base_url: provider.base_url.clone(),
        reasoning: definition.reasoning,
        thinking_level_map: definition.thinking_level_map.as_ref().map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        }),
        input: if definition.input.is_empty() {
            vec![InputKind::Text]
        } else {
            definition.input.clone()
        },
        cost: definition.cost.clone().unwrap_or(ModelCost {
            rates: ModelCostRates {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
            },
            tiers: None,
        }),
        context_window: definition.context_window,
        max_tokens: definition.max_tokens,
        sampling_params: None,
        headers: None,
        compat: definition.compat.clone(),
    })
}

/// defaultModel 投影成单一 `AiConfig`(commit/report/wiki/测试连接继续用)。
pub fn legacy_ai_config(config: &AiConfigFile) -> crate::ai::sdk::AiConfig {
    let empty = crate::ai::sdk::AiConfig::default();
    let Some(reference) = &config.default_model else {
        return empty;
    };
    let Some(provider) = config.providers.get(&reference.provider_id) else {
        return empty;
    };
    crate::ai::sdk::AiConfig {
        ai_base_url: provider.base_url.clone(),
        ai_api_key: provider.api_key.clone(),
        ai_model: reference.model_id.clone(),
    }
}

/// chat 偏好的 thinking 字符串 → `ModelThinkingLevel`(未知值按 off)。
pub fn parse_thinking_level(thinking: &str) -> ModelThinkingLevel {
    match thinking {
        "minimal" => ModelThinkingLevel::Minimal,
        "low" => ModelThinkingLevel::Low,
        "medium" => ModelThinkingLevel::Medium,
        "high" => ModelThinkingLevel::High,
        "xhigh" => ModelThinkingLevel::Xhigh,
        "max" => ModelThinkingLevel::Max,
        _ => ModelThinkingLevel::Off,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("repomeow-catalog-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn builtin_catalog_resolves_full_metadata() {
        let config = builtin_config();
        let model = resolve_model(&config, "deepseek", "deepseek-v4-pro").unwrap();
        assert_eq!(model.provider, "deepseek");
        assert_eq!(model.base_url, "https://api.deepseek.com/v1");
        assert!(model.reasoning);
        assert_eq!(model.context_window, 1_000_000);
        assert_eq!(model.max_tokens, 384_000);
        assert!(model.cost.rates.input > 0.0);
        assert_eq!(model.input, vec![InputKind::Text]);

        let fallback = builtin_config();
        let unnamed = resolve_model(&fallback, "ollama", "missing-model");
        assert!(unnamed.is_err());
    }

    #[test]
    fn default_model_resolves_full_model_and_api_key() {
        let mut config = builtin_config();
        config.default_model = Some(ModelRef {
            provider_id: "deepseek".to_string(),
            model_id: "deepseek-v4-pro".to_string(),
        });
        let reference = config.default_model.as_ref().unwrap();
        let expected_base_url = config.providers[&reference.provider_id].base_url.clone();
        config
            .providers
            .get_mut(&reference.provider_id)
            .unwrap()
            .api_key = " sk-test ".to_string();
        let (model, api_key) = resolve_default_model(&config).unwrap();
        assert_eq!(model.id, reference.model_id);
        assert_eq!(model.base_url, expected_base_url);
        assert_eq!(api_key, "sk-test");
        // 未配置 defaultModel 时明确报 AiNotConfigured
        let mut unset = builtin_config();
        unset.default_model = None;
        assert!(resolve_default_model(&unset).is_err());
    }

    #[test]
    fn normalize_drops_dangling_refs_and_keeps_defaults() {
        let mut config = builtin_config();
        config.default_model = Some(ModelRef {
            provider_id: "deepseek".to_string(),
            model_id: "no-such-model".to_string(),
        });
        config.chat.provider_id = Some("ghost".to_string());
        config.chat.model_id = Some("ghost-model".to_string());
        config.chat.thinking = "ultra".to_string();
        normalize(&mut config);
        assert!(config.default_model.is_none());
        assert!(config.chat.provider_id.is_none());
        assert!(config.chat.model_id.is_none());
        assert_eq!(config.chat.thinking, "off");
        assert_eq!(config.chat.permission, ChatPermission::All);
        assert!(config
            .providers
            .values()
            .all(|provider| provider.api == API_OPENAI_COMPLETIONS));
    }

    #[test]
    fn seed_migrates_legacy_settings_keys() {
        let dir = temp_dir("migrate");
        fs::write(
            dir.join("settings.json"),
            r#"{"aiBaseUrl":"https://api.example.com/v1/","aiApiKey":"sk-test","aiModel":"my-model"}"#,
        )
        .unwrap();
        let config = load_ai_config_file_at(&dir);
        assert!(config.providers.contains_key(LEGACY_PROVIDER_ID));
        let reference = config.default_model.as_ref().unwrap();
        assert_eq!(reference.provider_id, LEGACY_PROVIDER_ID);
        assert_eq!(reference.model_id, "my-model");
        assert_eq!(config.chat.model_id.as_deref(), Some("my-model"));
        let projected = legacy_ai_config(&config);
        assert_eq!(projected.ai_base_url, "https://api.example.com/v1");
        assert_eq!(projected.ai_model, "my-model");
        // 落盘后幂等:再次读取不重复播种也不丢数据。
        let again = load_ai_config_file_at(&dir);
        assert_eq!(again, config);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_is_backed_up_and_reseeded() {
        let dir = temp_dir("corrupt");
        let path = dir.join(AI_CONFIG_FILE_NAME);
        fs::write(&path, "{ not json").unwrap();
        let config = load_ai_config_file_at(&dir);
        assert!(!config.providers.is_empty());
        assert!(path.exists(), "重新播种应生成新配置");
        let backups = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .filter(|name| name.contains(".corrupt-"))
            .count();
        assert_eq!(backups, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_thinking_level_covers_all_levels() {
        assert_eq!(parse_thinking_level("off"), ModelThinkingLevel::Off);
        assert_eq!(parse_thinking_level("high"), ModelThinkingLevel::High);
        assert_eq!(parse_thinking_level("xhigh"), ModelThinkingLevel::Xhigh);
        assert_eq!(parse_thinking_level("bogus"), ModelThinkingLevel::Off);
    }

    #[test]
    fn chat_permission_accepts_legacy_readonly_and_serializes_as_ask() {
        // 默认仍为 All。
        assert_eq!(ChatPermission::default(), ChatPermission::All);
        // 当前取值 all / ask。
        assert_eq!(
            serde_json::from_str::<ChatPermission>("\"all\"").unwrap(),
            ChatPermission::All
        );
        assert_eq!(
            serde_json::from_str::<ChatPermission>("\"ask\"").unwrap(),
            ChatPermission::Ask
        );
        // 旧值 readOnly 兼容反序列化为 Ask(全部工具 + 执行前确认)。
        assert_eq!(
            serde_json::from_str::<ChatPermission>("\"readOnly\"").unwrap(),
            ChatPermission::Ask
        );
        // ChatPrefs 缺省 permission → All;旧 readOnly → Ask。
        let prefs: ChatPrefs = serde_json::from_str("{}").unwrap();
        assert_eq!(prefs.permission, ChatPermission::All);
        let prefs: ChatPrefs = serde_json::from_str(r#"{"permission":"readOnly"}"#).unwrap();
        assert_eq!(prefs.permission, ChatPermission::Ask);
        // 序列化只输出 all / ask,不再输出 readOnly。
        assert_eq!(
            serde_json::to_string(&ChatPermission::Ask).unwrap(),
            "\"ask\""
        );
        assert_eq!(
            serde_json::to_string(&ChatPermission::All).unwrap(),
            "\"all\""
        );
    }
}
