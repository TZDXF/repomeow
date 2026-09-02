//! 从 CC Switch(`~/.cc-switch`)读取供应商,筛选出 OpenAI chat 兼容的项供设置页导入。
//!
//! CC Switch 3.x 以 SQLite(`cc-switch.db`)为唯一事实源,旧版为 `config.json`;
//! 两者统一读出 (app_type, settings_config) 后按应用类型解析,仅保留 OpenAI chat
//! 兼容的供应商:
//! - codex:TOML `wire_api = "chat"`(responses 协议未实现)
//! - opencode:`npm = "@ai-sdk/openai-compatible"`
//! - openclaw / pi:`api = "openai-completions"`(openclaw 缺省 api 视为 openai-completions)
//! - hermes:`base_url` + `api_key` 顶层字段(OpenAI 兼容端点)
//! - grokbuild:TOML `api_backend = "chat_completions"`
//!
//! claude(Anthropic 协议)与 gemini(Google 协议)不属于 OpenAI chat 格式,不导入。

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::agent::llm::types::API_OPENAI_COMPLETIONS;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::time_util::now_ts_nanos;

use super::catalog::AiModelDef;

/// 一个可导入的 CC Switch 供应商(OpenAI chat 兼容)。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CcSwitchProvider {
    /// CC Switch 内的供应商 id(前端去重后作为厂商 id 候选)。
    pub id: String,
    pub name: String,
    /// 来源应用:codex / opencode / openclaw / pi / hermes / grokbuild。
    pub app: String,
    pub base_url: String,
    /// 可能为空(如密钥走环境变量),导入后由用户补齐。
    pub api_key: String,
    #[serde(default)]
    pub models: Vec<AiModelDef>,
    /// 在 CC Switch 中是否为该应用当前启用项。
    pub current: bool,
}

/// 扫描结果;`found = false` 表示本机未安装/未配置过 CC Switch。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CcSwitchScan {
    pub found: bool,
    pub providers: Vec<CcSwitchProvider>,
}

/// 从本机 `~/.cc-switch/` 扫描可导入的 OpenAI chat 兼容供应商。
pub fn scan_cc_switch_providers(app: &AppHandle) -> AppResult<CcSwitchScan> {
    let home = app
        .path()
        .home_dir()
        .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))?;
    scan_at(&home.join(".cc-switch"))
}

fn scan_at(dir: &Path) -> AppResult<CcSwitchScan> {
    let db_path = dir.join("cc-switch.db");
    let legacy_path = dir.join("config.json");
    let raw = if db_path.is_file() {
        read_providers_db(&db_path)?
    } else if legacy_path.is_file() {
        read_legacy_config(&legacy_path)
    } else {
        return Ok(CcSwitchScan {
            found: false,
            providers: Vec::new(),
        });
    };
    let mut providers: Vec<CcSwitchProvider> = raw.iter().filter_map(convert).collect();
    // 稳定排序:当前启用项优先,其余按名称
    providers.sort_by(|a, b| b.current.cmp(&a.current).then_with(|| a.name.cmp(&b.name)));
    Ok(CcSwitchScan {
        found: true,
        providers,
    })
}

// ── 读取(SQlite 数据库 / 旧版 config.json) ─────────────────────────

/// 数据库行(只取解析所需的核心列,旧库缺新增列也能查)。
struct RawProvider {
    id: String,
    app: String,
    name: String,
    settings_config: Value,
    current: bool,
}

/// CC Switch 可能正在运行并持有数据库;复制(含 WAL 侧车文件)到临时目录再打开,
/// 避免锁冲突,也能读到 WAL 中已提交的最新数据。
fn read_providers_db(db_path: &Path) -> AppResult<Vec<RawProvider>> {
    let staging = std::env::temp_dir().join(format!("repomeow-cc-switch-{}", now_ts_nanos()));
    fs::create_dir_all(&staging)?;
    let result = read_providers_db_staged(db_path, &staging);
    let _ = fs::remove_dir_all(&staging);
    result
}

fn read_providers_db_staged(db_path: &Path, staging: &Path) -> AppResult<Vec<RawProvider>> {
    let copy = staging.join("cc-switch.db");
    fs::copy(db_path, &copy)?;
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{}", db_path.display(), suffix));
        if sidecar.is_file() {
            fs::copy(&sidecar, staging.join(format!("cc-switch.db{suffix}")))?;
        }
    }
    query_providers(&copy)
}

fn query_providers(path: &Path) -> AppResult<Vec<RawProvider>> {
    let conn = Connection::open(path)?;
    let mut stmt = match conn
        .prepare("SELECT id, app_type, name, settings_config, is_current FROM providers")
    {
        Ok(stmt) => stmt,
        // 表不存在(空库/未知版本)按无供应商处理
        Err(rusqlite::Error::SqliteFailure(_, Some(message)))
            if message.contains("no such table") =>
        {
            return Ok(Vec::new());
        }
        Err(error) => return Err(error.into()),
    };
    let rows = stmt.query_map([], |row| {
        let config_text: String = row.get(3)?;
        Ok(RawProvider {
            id: row.get(0)?,
            app: row.get(1)?,
            name: row.get(2)?,
            settings_config: serde_json::from_str(&config_text).unwrap_or(Value::Null),
            current: row.get::<_, i64>(4).unwrap_or(0) != 0,
        })
    })?;
    let mut providers = Vec::new();
    for row in rows {
        providers.push(row?);
    }
    Ok(providers)
}

/// 旧版 CC Switch(<3.x)的 `config.json`:`{ apps: { <app>: { providers: {id: Provider}, current } } }`。
/// 解析失败按无供应商处理(不阻断,数据库才是新版事实源)。
fn read_legacy_config(path: &Path) -> Vec<RawProvider> {
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    let Some(apps) = value.get("apps").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut providers = Vec::new();
    for (app, manager) in apps {
        let current = manager.get("current").and_then(Value::as_str).unwrap_or("");
        let Some(entries) = manager.get("providers").and_then(Value::as_object) else {
            continue;
        };
        for (id, provider) in entries {
            let Some(settings_config) = provider.get("settingsConfig").cloned() else {
                continue;
            };
            providers.push(RawProvider {
                id: id.clone(),
                app: app.clone(),
                name: text(provider.get("name")),
                settings_config,
                current: current == id,
            });
        }
    }
    providers
}

// ── 按应用类型解析 settings_config ──────────────────────────────────

fn text(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string()
}

/// 归一化 API Key:空白与 `{env:VAR}` 环境变量引用都按空处理(导入后由用户补齐)。
fn api_key_text(value: Option<&Value>) -> String {
    let key = text(value);
    if key.starts_with("{env:") {
        String::new()
    } else {
        key
    }
}

fn convert(raw: &RawProvider) -> Option<CcSwitchProvider> {
    let (base_url, api_key, models) = match raw.app.as_str() {
        "codex" => parse_codex(&raw.settings_config)?,
        "opencode" => parse_opencode(&raw.settings_config)?,
        "openclaw" => parse_openclaw(&raw.settings_config)?,
        "pi" => parse_pi(&raw.settings_config)?,
        "hermes" => parse_hermes(&raw.settings_config)?,
        "grokbuild" => parse_grokbuild(&raw.settings_config)?,
        _ => return None,
    };
    if base_url.is_empty() {
        return None;
    }
    Some(CcSwitchProvider {
        id: raw.id.clone(),
        name: raw.name.clone(),
        app: raw.app.clone(),
        base_url,
        api_key,
        models,
        current: raw.current,
    })
}

/// 无完整元数据来源时的模型条目(上下文/输出窗口未知,导入后可在设置页补齐)。
fn bare_model(id: &str, name: String, context_window: i64, max_tokens: i64) -> AiModelDef {
    AiModelDef {
        id: id.to_string(),
        name,
        reasoning: true,
        input: Vec::new(),
        context_window,
        max_tokens,
        cost: None,
        thinking_level_map: None,
        compat: None,
    }
}

/// pi / openclaw 的 models 数组与本配置同族,直接反序列化为 AiModelDef(无 id 的项丢弃)。
fn parse_model_array(config: &Value) -> Vec<AiModelDef> {
    config
        .get("models")
        .and_then(Value::as_array)
        .map(|models| {
            models
                .iter()
                .filter_map(|model| {
                    let mut model = model.clone();
                    // openclaw 的 cost 只有 input/output;补齐缺省字段,避免整条模型因反序列化失败被丢弃
                    if let Some(cost) = model.get_mut("cost").and_then(Value::as_object_mut) {
                        cost.entry("cacheRead").or_insert(Value::from(0.0));
                        cost.entry("cacheWrite").or_insert(Value::from(0.0));
                    }
                    let mut def: AiModelDef = serde_json::from_value(model).ok()?;
                    def.id = def.id.trim().to_string();
                    if def.id.is_empty() {
                        return None;
                    }
                    if def.name.trim().is_empty() {
                        def.name = def.id.clone();
                    }
                    Some(def)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// codex:`{ auth: { OPENAI_API_KEY }, config: "<config.toml 文本>" }`;
/// 仅当激活 model_provider 的 `wire_api = "chat"` 时兼容 OpenAI chat。
fn parse_codex(config: &Value) -> Option<(String, String, Vec<AiModelDef>)> {
    let config_text = config.get("config").and_then(Value::as_str)?;
    let doc = config_text.parse::<toml::Value>().ok()?;
    let providers = doc.get("model_providers").and_then(toml::Value::as_table);
    let active_id = doc.get("model_provider").and_then(toml::Value::as_str);
    let active = active_id.and_then(|id| providers.and_then(|tables| tables.get(id)));
    // base_url:激活的 model_providers 条目优先,回退顶层 base_url(对齐 cc-switch 提取顺序)
    let base_url = active
        .and_then(|table| table.get("base_url"))
        .and_then(toml::Value::as_str)
        .or_else(|| doc.get("base_url").and_then(toml::Value::as_str))
        .map(str::trim)
        .map(|url| url.trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty())?;
    // wire_api:只看激活条目;缺省按 codex 惯例视为 responses,不写 "chat" 的不导入
    let wire_api = active
        .and_then(|table| table.get("wire_api"))
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .unwrap_or("responses");
    if wire_api != "chat" {
        return None;
    }
    // 密钥:auth.OPENAI_API_KEY → 激活条目/顶层 experimental_bearer_token
    let bearer = active
        .and_then(|table| table.get("experimental_bearer_token"))
        .and_then(toml::Value::as_str)
        .or_else(|| {
            doc.get("experimental_bearer_token")
                .and_then(toml::Value::as_str)
        });
    let api_key = config
        .get("auth")
        .and_then(|auth| auth.get("OPENAI_API_KEY"))
        .and_then(Value::as_str)
        .or(bearer)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let models = doc
        .get("model")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| vec![bare_model(id, id.to_string(), 0, 0)])
        .unwrap_or_default();
    Some((base_url, api_key, models))
}

/// opencode:`{ npm, options: { baseURL, apiKey }, models: { id: { name, limit } } }`;
/// 仅 `@ai-sdk/openai-compatible` 包是 OpenAI chat 协议。
fn parse_opencode(config: &Value) -> Option<(String, String, Vec<AiModelDef>)> {
    let npm = text(config.get("npm"));
    if npm != "@ai-sdk/openai-compatible" {
        return None;
    }
    let options = config.get("options");
    let base_url = text(options.and_then(|o| o.get("baseURL")))
        .trim_end_matches('/')
        .to_string();
    let api_key = api_key_text(options.and_then(|o| o.get("apiKey")));
    let models = config
        .get("models")
        .and_then(Value::as_object)
        .map(|entries| {
            entries
                .iter()
                .map(|(id, model)| {
                    let limit = model.get("limit");
                    let context = limit
                        .and_then(|l| l.get("context"))
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    let output = limit
                        .and_then(|l| l.get("output"))
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    let name = text(model.get("name"));
                    bare_model(
                        id,
                        if name.is_empty() { id.clone() } else { name },
                        context,
                        output,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    Some((base_url, api_key, models))
}

/// openclaw:`{ baseUrl, apiKey, api?, models: […] }`(pi 同族格式);
/// 写了 api 且不是 openai-completions 的不导入。
fn parse_openclaw(config: &Value) -> Option<(String, String, Vec<AiModelDef>)> {
    let api = text(config.get("api"));
    if !api.is_empty() && api != API_OPENAI_COMPLETIONS {
        return None;
    }
    let base_url = text(config.get("baseUrl"))
        .trim_end_matches('/')
        .to_string();
    let api_key = text(config.get("apiKey"));
    Some((base_url, api_key, parse_model_array(config)))
}

/// pi:models.json 供应商节点 `{ baseUrl, apiKey, api, models: […] }`;
/// 仅 `api = "openai-completions"` 兼容。
fn parse_pi(config: &Value) -> Option<(String, String, Vec<AiModelDef>)> {
    if text(config.get("api")) != API_OPENAI_COMPLETIONS {
        return None;
    }
    let base_url = text(config.get("baseUrl"));
    let base_url = if base_url.is_empty() {
        // cc-switch 允许 baseUrl 写在某个模型条目上
        config
            .get("models")
            .and_then(Value::as_array)
            .and_then(|models| models.iter().find_map(|model| model.get("baseUrl")))
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string()
    } else {
        base_url
    };
    let base_url = base_url.trim_end_matches('/').to_string();
    let api_key = text(config.get("apiKey"));
    Some((base_url, api_key, parse_model_array(config)))
}

/// hermes:`{ base_url, api_key, model?, models: { id: { context_length } } }`。
fn parse_hermes(config: &Value) -> Option<(String, String, Vec<AiModelDef>)> {
    let base_url = text(config.get("base_url"))
        .trim_end_matches('/')
        .to_string();
    let api_key = text(config.get("api_key"));
    let mut models: Vec<AiModelDef> = config
        .get("models")
        .and_then(Value::as_object)
        .map(|entries| {
            entries
                .iter()
                .map(|(id, model)| {
                    let context = model
                        .get("context_length")
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    bare_model(id, id.clone(), context, 0)
                })
                .collect()
        })
        .unwrap_or_default();
    let active = text(config.get("model"));
    if !active.is_empty() && !models.iter().any(|model| model.id == active) {
        models.push(bare_model(&active, active.clone(), 0, 0));
    }
    Some((base_url, api_key, models))
}

/// grokbuild:`{ config: "<toml>" }`,TOML 形如
/// `[models] default = "<profile>"` + `[model."<profile>"] { model, base_url, api_key, api_backend, context_window }`;
/// 仅 `api_backend = "chat_completions"` 兼容。
fn parse_grokbuild(config: &Value) -> Option<(String, String, Vec<AiModelDef>)> {
    let config_text = config.get("config").and_then(Value::as_str)?;
    let doc = config_text.parse::<toml::Value>().ok()?;
    let profile = doc
        .get("models")
        .and_then(toml::Value::as_table)
        .and_then(|models| models.get("default"))
        .and_then(toml::Value::as_str)?
        .trim();
    let selected = doc
        .get("model")
        .and_then(toml::Value::as_table)
        .and_then(|models| models.get(profile))
        .and_then(toml::Value::as_table)?;
    let backend = selected
        .get("api_backend")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .unwrap_or("responses");
    if backend != "chat_completions" {
        return None;
    }
    let base_url = selected
        .get("base_url")
        .and_then(toml::Value::as_str)
        .map(|url| url.trim().trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty())?;
    // env_key 指向的环境变量在 GUI 进程里通常读不到,宁可留空也不代入错误的密钥
    let api_key = selected
        .get("api_key")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let context = selected
        .get("context_window")
        .and_then(toml::Value::as_integer)
        .filter(|value| *value > 0)
        .unwrap_or(0);
    let model_id = selected
        .get("model")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let models = if model_id.is_empty() {
        Vec::new()
    } else {
        vec![bare_model(model_id, model_id.to_string(), context, 0)]
    };
    Some((base_url, api_key, models))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn raw(app: &str, settings_config: Value) -> RawProvider {
        RawProvider {
            id: "p1".to_string(),
            app: app.to_string(),
            name: "测试".to_string(),
            settings_config,
            current: false,
        }
    }

    fn convert_app(app: &str, settings_config: Value) -> Option<CcSwitchProvider> {
        convert(&raw(app, settings_config))
    }

    #[test]
    fn codex_chat_wire_api() {
        let provider = convert_app(
            "codex",
            json!({
                "auth": { "OPENAI_API_KEY": "sk-test" },
                "config": "model_provider = \"custom\"\nmodel = \"gpt-4o\"\n\n[model_providers.custom]\nbase_url = \"https://api.example.com/v1/\"\nwire_api = \"chat\"\n",
            }),
        )
        .expect("chat wire_api 应可导入");
        assert_eq!(provider.base_url, "https://api.example.com/v1");
        assert_eq!(provider.api_key, "sk-test");
        assert_eq!(provider.models.len(), 1);
        assert_eq!(provider.models[0].id, "gpt-4o");
    }

    #[test]
    fn codex_responses_wire_api_skipped() {
        assert!(convert_app(
            "codex",
            json!({
                "auth": { "OPENAI_API_KEY": "sk-test" },
                "config": "[model_providers.custom]\nbase_url = \"https://api.example.com/v1\"\nwire_api = \"responses\"\n",
            }),
        )
        .is_none());
        // 未写 wire_api 按 responses 处理,不导入
        assert!(convert_app(
            "codex",
            json!({
                "config": "[model_providers.custom]\nbase_url = \"https://api.example.com/v1\"\n",
            }),
        )
        .is_none());
    }

    #[test]
    fn codex_bearer_token_fallback() {
        let provider = convert_app(
            "codex",
            json!({
                "config": "model_provider = \"custom\"\n\n[model_providers.custom]\nbase_url = \"https://api.example.com/v1\"\nwire_api = \"chat\"\nexperimental_bearer_token = \"tok-1\"\n",
            }),
        )
        .expect("bearer token 兜底");
        assert_eq!(provider.api_key, "tok-1");
    }

    #[test]
    fn opencode_requires_openai_compatible_npm() {
        let settings = || {
            json!({
                "npm": "@ai-sdk/openai-compatible",
                "options": { "baseURL": "https://api.example.com/v1", "apiKey": "sk-x" },
                "models": { "gpt-4o": { "name": "GPT-4o", "limit": { "context": 128000, "output": 16384 } } },
            })
        };
        let provider = convert_app("opencode", settings()).expect("openai-compatible 应可导入");
        assert_eq!(provider.base_url, "https://api.example.com/v1");
        assert_eq!(provider.models[0].context_window, 128000);
        assert_eq!(provider.models[0].max_tokens, 16384);

        let mut other = settings();
        other["npm"] = json!("@ai-sdk/anthropic");
        assert!(convert_app("opencode", other).is_none());
    }

    #[test]
    fn opencode_env_ref_key_becomes_empty() {
        let provider = convert_app(
            "opencode",
            json!({
                "npm": "@ai-sdk/openai-compatible",
                "options": { "baseURL": "https://api.example.com/v1", "apiKey": "{env:MY_KEY}" },
            }),
        )
        .expect("可导入");
        assert_eq!(provider.api_key, "");
    }

    #[test]
    fn openclaw_api_filter_and_models() {
        let provider = convert_app(
            "openclaw",
            json!({
                "baseUrl": "https://api.example.com/v1",
                "apiKey": "sk-x",
                "api": "openai-completions",
                "models": [{ "id": "m1", "name": "M1", "contextWindow": 64000, "reasoning": false }],
            }),
        )
        .expect("openai-completions 应可导入");
        assert_eq!(provider.models[0].id, "m1");
        assert_eq!(provider.models[0].context_window, 64000);
        assert!(!provider.models[0].reasoning);

        // 缺省 api 视为 openai-completions
        assert!(convert_app(
            "openclaw",
            json!({ "baseUrl": "https://api.example.com/v1", "apiKey": "sk-x" }),
        )
        .is_some());
        // cost 只有 input/output 的 openclaw 模型也能导入
        let provider = convert_app(
            "openclaw",
            json!({
                "baseUrl": "https://api.example.com/v1",
                "apiKey": "sk-x",
                "models": [{ "id": "m2", "cost": { "input": 1.0, "output": 2.0 } }],
            }),
        )
        .expect("部分 cost 字段可导入");
        assert_eq!(provider.models.len(), 1);
        let cost = provider.models[0].cost.as_ref().expect("cost 保留");
        assert_eq!(cost.rates.input, 1.0);
        assert_eq!(cost.rates.cache_read, 0.0);
        // anthropic 协议不导入
        assert!(convert_app(
            "openclaw",
            json!({ "baseUrl": "https://api.example.com", "apiKey": "sk-x", "api": "anthropic-messages" }),
        )
        .is_none());
    }

    #[test]
    fn pi_requires_openai_completions() {
        let provider = convert_app(
            "pi",
            json!({
                "baseUrl": "https://api.example.com/v1",
                "apiKey": "sk-x",
                "api": "openai-completions",
                "models": [{ "id": "m1", "contextWindow": 1000, "maxTokens": 100, "cost": { "input": 1.0, "output": 2.0, "cacheRead": 0.1, "cacheWrite": 0.0 } }],
            }),
        )
        .expect("openai-completions 应可导入");
        assert_eq!(provider.models[0].max_tokens, 100);
        assert!(provider.models[0].cost.is_some());

        assert!(convert_app(
            "pi",
            json!({ "baseUrl": "https://api.example.com", "apiKey": "sk-x", "api": "openai-responses" }),
        )
        .is_none());
        // baseUrl 可落在模型条目上
        let provider = convert_app(
            "pi",
            json!({
                "apiKey": "sk-x",
                "api": "openai-completions",
                "models": [{ "id": "m1", "baseUrl": "https://api.example.com/v1" }],
            }),
        )
        .expect("模型级 baseUrl 兜底");
        assert_eq!(provider.base_url, "https://api.example.com/v1");
    }

    #[test]
    fn hermes_snake_case_fields() {
        let provider = convert_app(
            "hermes",
            json!({
                "base_url": "https://openrouter.ai/api/v1/",
                "api_key": "sk-or-x",
                "model": "anthropic/claude-opus-4-8",
                "models": { "anthropic/claude-opus-4-8": { "context_length": 200000 } },
            }),
        )
        .expect("hermes 应可导入");
        assert_eq!(provider.base_url, "https://openrouter.ai/api/v1");
        assert_eq!(provider.models.len(), 1);
        assert_eq!(provider.models[0].context_window, 200000);
    }

    #[test]
    fn grokbuild_chat_completions_only() {
        let toml = |backend: &str| {
            json!({
                "config": format!("[models]\ndefault = \"p1\"\n\n[model.\"p1\"]\nmodel = \"grok-4.5\"\nbase_url = \"https://api.x.ai/v1\"\nname = \"xAI\"\napi_key = \"xai-x\"\napi_backend = \"{backend}\"\ncontext_window = 500000\n"),
            })
        };
        let provider = convert_app("grokbuild", toml("chat_completions")).expect("chat 应可导入");
        assert_eq!(provider.base_url, "https://api.x.ai/v1");
        assert_eq!(provider.models[0].id, "grok-4.5");
        assert_eq!(provider.models[0].context_window, 500000);
        assert!(convert_app("grokbuild", toml("responses")).is_none());
    }

    #[test]
    fn anthropic_and_gemini_apps_skipped() {
        assert!(convert_app(
            "claude",
            json!({ "env": { "ANTHROPIC_BASE_URL": "https://api.example.com", "ANTHROPIC_AUTH_TOKEN": "sk-x" } }),
        )
        .is_none());
        assert!(convert_app(
            "gemini",
            json!({ "env": { "GOOGLE_GEMINI_BASE_URL": "https://api.example.com", "GEMINI_API_KEY": "sk-x" } }),
        )
        .is_none());
        // base_url 缺失不导入
        assert!(convert_app("hermes", json!({ "api_key": "sk-x" })).is_none());
    }

    #[test]
    fn legacy_config_json_shape() {
        let dir = std::env::temp_dir().join(format!("repomeow-ccs-legacy-{}", now_ts_nanos()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("config.json"),
            json!({
                "apps": {
                    "codex": {
                        "current": "a",
                        "providers": {
                            "a": {
                                "name": "A",
                                "settingsConfig": {
                                    "auth": { "OPENAI_API_KEY": "sk-a" },
                                    "config": "model_provider = \"custom\"\n\n[model_providers.custom]\nbase_url = \"https://a.example.com/v1\"\nwire_api = \"chat\"\n",
                                },
                            },
                        },
                    },
                    "claude": {
                        "current": "",
                        "providers": {
                            "b": {
                                "name": "B",
                                "settingsConfig": { "env": { "ANTHROPIC_BASE_URL": "https://b.example.com" } },
                            },
                        },
                    },
                },
            })
            .to_string(),
        )
        .unwrap();
        let scan = scan_at(&dir).expect("扫描成功");
        let _ = fs::remove_dir_all(&dir);
        assert!(scan.found);
        assert_eq!(scan.providers.len(), 1);
        assert_eq!(scan.providers[0].id, "a");
        assert!(scan.providers[0].current);
    }

    #[test]
    fn missing_dir_reports_not_found() {
        let dir = std::env::temp_dir().join(format!("repomeow-ccs-none-{}", now_ts_nanos()));
        let scan = scan_at(&dir).expect("扫描成功");
        assert!(!scan.found);
        assert!(scan.providers.is_empty());
    }
}
