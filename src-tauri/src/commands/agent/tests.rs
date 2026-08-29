use std::path::Path;

use agent_client_protocol::schema::v1::{
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectGroup,
    SessionConfigSelectOption, ToolCallLocation, ToolKind,
};

use super::callbacks::{
    permission_allowed, read_file_within, read_tool_activity_text, tool_activity_text,
};
use super::config::{category_str, config_option_info};
use super::registry::parse_command_line;
use super::AcpConfigChoice;
use crate::time_util::now_ts_nanos;

#[test]
fn config_option_info_maps_category_and_flattens_groups() {
    let option = SessionConfigOption::select(
        "model",
        "Model",
        "glm-4.6",
        vec![
            SessionConfigSelectOption::new("glm-4.6", "GLM-4.6"),
            SessionConfigSelectOption::new("glm-4.5", "GLM-4.5"),
        ],
    )
    .category(SessionConfigOptionCategory::Model);
    let info = config_option_info(&option);
    assert_eq!(info.id, "model");
    assert_eq!(info.category.as_deref(), Some("model"));
    assert_eq!(info.current.as_deref(), Some("glm-4.6"));
    assert_eq!(info.choices.len(), 2);
    assert_eq!(info.choices[1].id, "glm-4.5");
    assert_eq!(info.choices[1].name, "GLM-4.5");

    let grouped = SessionConfigOption::select(
        "effort",
        "Effort",
        "low",
        vec![SessionConfigSelectGroup::new(
            "g1",
            "常用",
            vec![SessionConfigSelectOption::new("low", "Low")],
        )],
    )
    .category(SessionConfigOptionCategory::ThoughtLevel);
    let info = config_option_info(&grouped);
    assert_eq!(info.category.as_deref(), Some("thought_level"));
    assert_eq!(
        info.choices,
        vec![AcpConfigChoice {
            id: "low".into(),
            name: "Low".into(),
        }]
    );

    assert_eq!(
        category_str(&Some(SessionConfigOptionCategory::Other("custom".into()))).as_deref(),
        Some("custom"),
    );
    assert_eq!(category_str(&None), None);
}

#[test]
fn parse_command_line_splits_and_quotes() {
    assert_eq!(
        parse_command_line("npx -y pkg --acp"),
        ["npx", "-y", "pkg", "--acp"]
    );
    assert_eq!(
        parse_command_line(r#""C:\Program Files\agent.exe" --acp "a b""#),
        ["C:\\Program Files\\agent.exe", "--acp", "a b"]
    );
    assert!(parse_command_line("   ").is_empty());
}

#[test]
fn tool_activity_summarizes_raw_input_inline() {
    let locations = vec![ToolCallLocation::new(r"D:\repo\src\main.rs")];
    let input = serde_json::json!({
        "arguments": {
            "file_path": "src/lib.rs",
            "limit": 40,
            "query": "must not be displayed"
        }
    });
    let text = tool_activity_text("read", &locations, Some(&input));
    assert_eq!(text, "read src/lib.rs must not be displayed");

    let other = serde_json::json!({"verbose": {"nested": true}});
    assert_eq!(
        tool_activity_text("custom", &[], Some(&other)),
        r#"custom {"verbose":{"nested":true}}"#
    );

    let fallback = tool_activity_text("read", &locations, None);
    assert_eq!(fallback, r"read D:\repo\src\main.rs");

    let callback = read_tool_activity_text(Path::new("src/lib.rs"), Some(20), Some(40));
    assert_eq!(callback, "read src/lib.rs:20+40");

    let long = serde_json::json!({"command": "x".repeat(300)});
    let text = tool_activity_text("bash", &[], Some(&long));
    assert!(text.len() < 140);
    assert!(text.ends_with('…'));
    assert!(!text.contains('\n'));
}

#[test]
fn writable_sessions_allow_code_changes_but_not_mode_switches() {
    assert!(!permission_allowed(Some(&ToolKind::Edit), false));
    assert!(!permission_allowed(Some(&ToolKind::Execute), false));
    assert!(permission_allowed(Some(&ToolKind::Edit), true));
    assert!(permission_allowed(Some(&ToolKind::Delete), true));
    assert!(permission_allowed(Some(&ToolKind::Move), true));
    assert!(permission_allowed(Some(&ToolKind::Execute), true));
    assert!(!permission_allowed(Some(&ToolKind::SwitchMode), true));
    assert!(permission_allowed(None, true));
}

#[test]
fn read_file_within_respects_root_and_range() {
    let dir = std::env::temp_dir().join(format!("repomeow-agent-test-{}", now_ts_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.txt");
    std::fs::write(&file, "l1\nl2\nl3\n").unwrap();

    assert_eq!(
        read_file_within(&dir, Path::new("a.txt"), Some(2), Some(1)).unwrap(),
        "l2"
    );
    assert_eq!(
        read_file_within(&dir, &file, None, None).unwrap(),
        "l1\nl2\nl3\n"
    );
    assert!(read_file_within(&dir, Path::new("../escape.txt"), None, None).is_err());
    std::fs::remove_dir_all(&dir).ok();
}
