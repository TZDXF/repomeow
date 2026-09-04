use std::fs;

use crate::time_util::now_ts_nanos;
use super::parse::*;
use super::read::*;
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
    let anthropic = convert_app(
        "openclaw",
        json!({ "baseUrl": "https://api.example.com", "apiKey": "sk-x", "api": "anthropic-messages" }),
    )
    .expect("已实现的 anthropic 协议应保留");
    assert_eq!(anthropic.api, "anthropic-messages");
}

#[test]
fn pi_accepts_supported_api_types() {
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

    let responses = convert_app(
        "pi",
        json!({ "baseUrl": "https://api.example.com", "apiKey": "sk-x", "api": "openai-responses" }),
    )
    .expect("已实现的 responses 协议应保留");
    assert_eq!(responses.api, "openai-responses");
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
