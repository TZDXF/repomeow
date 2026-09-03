use super::*;


/// 内置目录 + 给 deepseek 配上密钥的测试配置。
fn test_config() -> crate::ai::catalog::AiConfigFile {
    let mut config = crate::ai::catalog::builtin_config();
    config
        .providers
        .get_mut("deepseek")
        .unwrap()
        .api_key = "sk-test".to_string();
    config
}

#[test]
fn builtin_model_status_reports_missing_model_and_falls_back_to_default() {
    let config = test_config();

    // 显式配置的模型不存在 → 报错且原因含复合引用
    let (reference, status) = builtin_model_status(&config, Some("deepseek/no-such-model"));
    assert_eq!(reference.as_deref(), Some("deepseek/no-such-model"));
    let reason = status.unwrap_err();
    assert!(reason.contains("deepseek/no-such-model"), "{reason}");

    // 未显式配置且无默认模型 → 报「未配置默认模型」
    let (reference, status) = builtin_model_status(&config, None);
    assert!(reference.is_none());
    assert_eq!(status.unwrap_err(), "未配置默认模型");

    // 默认模型有效 → 通过,生效引用为默认模型
    let mut with_default = config.clone();
    with_default.default_model = Some(crate::ai::catalog::ModelRef {
        provider_id: "deepseek".to_string(),
        model_id: "deepseek-v4-pro".to_string(),
    });
    let (reference, status) = builtin_model_status(&with_default, None);
    assert_eq!(reference.as_deref(), Some("deepseek/deepseek-v4-pro"));
    assert!(status.is_ok());

    // 显式配置有效 → 通过
    let (_, status) = builtin_model_status(&config, Some("deepseek/deepseek-v4-pro"));
    assert!(status.is_ok());

    // 模型存在但厂商密钥为空 → 报错(openai 在内置目录里无密钥)
    let (_, status) = builtin_model_status(&config, Some("openai/gpt-5.3-chat-latest"));
    assert!(status.unwrap_err().contains("API Key"));
}
