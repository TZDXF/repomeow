use serde_json::Value;

use crate::agent::llm::types::{
    is_supported_api, API_ANTHROPIC_MESSAGES, API_GOOGLE_GENERATIVE_AI, API_OPENAI_COMPLETIONS,
    API_OPENAI_RESPONSES,
};

use crate::ai::catalog::AiModelDef;
use super::read::RawProvider;
use super::CcSwitchProvider;

// ── 按应用类型解析 settings_config ──────────────────────────────────

pub(super) fn text(value: Option<&Value>) -> String {
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

pub(super) fn convert(raw: &RawProvider) -> Option<CcSwitchProvider> {
    let (base_url, api_key, api, models) = match raw.app.as_str() {
        "codex" => parse_codex(&raw.settings_config)?,
        "opencode" => parse_opencode(&raw.settings_config)?,
        "claude" | "claude-desktop" => {
            let (base_url, api_key, models) = parse_claude(&raw.settings_config)?;
            (base_url, api_key, API_ANTHROPIC_MESSAGES.to_string(), models)
        }
        "gemini" => {
            let (base_url, api_key, models) = parse_gemini(&raw.settings_config)?;
            (base_url, api_key, API_GOOGLE_GENERATIVE_AI.to_string(), models)
        }
        "openclaw" => parse_openclaw(&raw.settings_config)?,
        "pi" => parse_pi(&raw.settings_config)?,
        "hermes" => {
            let (base_url, api_key, models) = parse_hermes(&raw.settings_config)?;
            (base_url, api_key, API_OPENAI_COMPLETIONS.to_string(), models)
        }
        "grokbuild" => {
            let (base_url, api_key, models) = parse_grokbuild(&raw.settings_config)?;
            (base_url, api_key, API_OPENAI_COMPLETIONS.to_string(), models)
        }
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
        api,
        models,
        current: raw.current,
    })
}

/// 无完整元数据来源时的模型条目(上下文/输出窗口未知,导入后可在设置页补齐)。
fn bare_model(id: &str, name: String, context_window: i64, max_tokens: i64) -> AiModelDef {
    AiModelDef {
        id: id.to_string(),
        api: None,
        base_url: None,
        headers: None,
        sampling_params: None,
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

/// codex:`{ auth: { OPENAI_API_KEY }, config: "<config.toml 文本>", modelCatalog? }`;
/// 按激活 model_provider 的 `wire_api` 判定协议:`chat` → OpenAI Chat,缺省/`responses` → OpenAI Responses。
fn parse_codex(config: &Value) -> Option<(String, String, String, Vec<AiModelDef>)> {
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
    // wire_api:只看激活条目;缺省按 codex 惯例视为 responses,其余取值不导入
    let wire_api = active
        .and_then(|table| table.get("wire_api"))
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .unwrap_or("responses");
    let api = match wire_api {
        "chat" => API_OPENAI_COMPLETIONS.to_string(),
        "responses" => API_OPENAI_RESPONSES.to_string(),
        _ => return None,
    };
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
    // 模型:settings_config.modelCatalog.models[](cc-switch 扩展)优先,回退 TOML 顶层 model
    let mut models = parse_codex_catalog(config);
    if models.is_empty() {
        if let Some(id) = doc
            .get("model")
            .and_then(toml::Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            models.push(bare_model(id, id.to_string(), 0, 0));
        }
    }
    Some((base_url, api_key, api, models))
}

/// settings_config.modelCatalog.models[]:`{ model, displayName?, contextWindow? }`(cc-switch 扩展字段)。
fn parse_codex_catalog(config: &Value) -> Vec<AiModelDef> {
    config
        .get("modelCatalog")
        .and_then(|catalog| catalog.get("models"))
        .and_then(Value::as_array)
        .map(|models| {
            models
                .iter()
                .filter_map(|model| {
                    let id = text(model.get("model"));
                    if id.is_empty() {
                        return None;
                    }
                    let context = model
                        .get("contextWindow")
                        .and_then(Value::as_i64)
                        .filter(|value| *value > 0)
                        .unwrap_or(0);
                    let name = text(model.get("displayName"));
                    Some(bare_model(
                        &id,
                        if name.is_empty() { id.clone() } else { name },
                        context,
                        0,
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// opencode:`{ npm, options: { baseURL, apiKey }, models: { id: { name, limit } } }`;
/// npm 是 AI SDK 供应商包,据此判定 wire adapter,未知包不导入。
fn parse_opencode(config: &Value) -> Option<(String, String, String, Vec<AiModelDef>)> {
    let api = opencode_api(&text(config.get("npm")))?;
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
    Some((base_url, api_key, api, models))
}

/// opencode 的 npm SDK 包 → wire adapter;`@ai-sdk/openai` 在 AI SDK v5 默认走 Responses 协议。
fn opencode_api(npm: &str) -> Option<String> {
    match npm {
        "@ai-sdk/openai-compatible" => Some(API_OPENAI_COMPLETIONS.to_string()),
        "@ai-sdk/openai" => Some(API_OPENAI_RESPONSES.to_string()),
        "@ai-sdk/anthropic" => Some(API_ANTHROPIC_MESSAGES.to_string()),
        "@ai-sdk/google" => Some(API_GOOGLE_GENERATIVE_AI.to_string()),
        _ => None,
    }
}

/// Claude Code 的 `[1m]` 模型后缀是 1M 上下文 beta 标记,不是模型 id 的一部分。
fn strip_context_suffix(id: &str) -> &str {
    id.strip_suffix("[1m]")
        .or_else(|| id.strip_suffix("[1M]"))
        .unwrap_or(id)
}

/// claude / claude-desktop:`{ env: { ANTHROPIC_BASE_URL, ANTHROPIC_AUTH_TOKEN, ANTHROPIC_MODEL, … } }`。
/// 模型取 ANTHROPIC_MODEL 与 SONNET/OPUS/HAIKU 分档映射去重;仅官方登录(无 base_url)的项不导入。
fn parse_claude(config: &Value) -> Option<(String, String, Vec<AiModelDef>)> {
    let env = config.get("env");
    let base_url = text(env.and_then(|e| e.get("ANTHROPIC_BASE_URL")))
        .trim_end_matches('/')
        .to_string();
    if base_url.is_empty() {
        return None;
    }
    let mut api_key = api_key_text(env.and_then(|e| e.get("ANTHROPIC_AUTH_TOKEN")));
    if api_key.is_empty() {
        api_key = api_key_text(env.and_then(|e| e.get("ANTHROPIC_API_KEY")));
    }
    let mut models: Vec<AiModelDef> = Vec::new();
    for key in [
        "ANTHROPIC_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    ] {
        let id = strip_context_suffix(&text(env.and_then(|e| e.get(key)))).to_string();
        if id.is_empty() || models.iter().any(|model| model.id == id) {
            continue;
        }
        models.push(bare_model(&id, id.clone(), 0, 0));
    }
    Some((base_url, api_key, models))
}

/// gemini:`{ env: { GOOGLE_GEMINI_BASE_URL, GEMINI_API_KEY, GEMINI_MODEL } }`;
/// 仅官方登录(无 base_url)的项不导入。
fn parse_gemini(config: &Value) -> Option<(String, String, Vec<AiModelDef>)> {
    let env = config.get("env");
    let base_url = text(env.and_then(|e| e.get("GOOGLE_GEMINI_BASE_URL")))
        .trim_end_matches('/')
        .to_string();
    if base_url.is_empty() {
        return None;
    }
    let mut api_key = api_key_text(env.and_then(|e| e.get("GEMINI_API_KEY")));
    if api_key.is_empty() {
        api_key = api_key_text(env.and_then(|e| e.get("GOOGLE_API_KEY")));
    }
    let mut model_id = text(env.and_then(|e| e.get("GEMINI_MODEL")));
    if model_id.is_empty() {
        model_id = text(env.and_then(|e| e.get("GOOGLE_GEMINI_MODEL")));
    }
    let models = if model_id.is_empty() {
        Vec::new()
    } else {
        vec![bare_model(&model_id, model_id.clone(), 0, 0)]
    };
    Some((base_url, api_key, models))
}

/// openclaw:`{ baseUrl, apiKey, api?, models: […] }`(pi 同族格式);
/// api 缺省按 openai-completions,写了四种已实现 adapter 之外的不导入。
fn parse_openclaw(config: &Value) -> Option<(String, String, String, Vec<AiModelDef>)> {
    let api = text(config.get("api"));
    let api = if api.is_empty() {
        API_OPENAI_COMPLETIONS.to_string()
    } else if is_supported_api(&api) {
        api
    } else {
        return None;
    };
    let base_url = text(config.get("baseUrl"))
        .trim_end_matches('/')
        .to_string();
    let api_key = text(config.get("apiKey"));
    Some((base_url, api_key, api, parse_model_array(config)))
}

/// pi:models.json 供应商节点 `{ baseUrl, apiKey, api, models: […] }`;
/// api 必须是四种已实现 adapter 之一,缺省(空)不导入。
fn parse_pi(config: &Value) -> Option<(String, String, String, Vec<AiModelDef>)> {
    let api = text(config.get("api"));
    if !is_supported_api(&api) {
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
    Some((base_url, api_key, api, parse_model_array(config)))
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

