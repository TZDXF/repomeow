use super::cc_export::*;
use super::*;

use serde_json::json;

use crate::commands::usage::count_o200k_tokens;

fn temp_project_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "repomeow-ai-assets-{tag}-{}-{}",
        std::process::id(),
        crate::time_util::now_ts_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn detects_fixed_instruction_files() {
    let dir = temp_project_dir("files");
    fs::write(dir.join("CLAUDE.md"), "# claude").unwrap();
    fs::write(dir.join("AGENTS.md"), "# agents").unwrap();
    fs::create_dir_all(dir.join(".github")).unwrap();
    fs::write(dir.join(".github/copilot-instructions.md"), "hint").unwrap();
    fs::create_dir_all(dir.join(".cursor/rules")).unwrap();
    fs::write(dir.join(".cursor/rules/a.mdc"), "rule a").unwrap();
    fs::write(dir.join(".cursor/rules/b.md"), "rule b").unwrap();
    fs::write(dir.join(".cursor/rules/c.txt"), "not a rule").unwrap();

    let assets = scan_assets(&dir.to_string_lossy()).unwrap();
    let paths: Vec<&str> = assets.files.iter().map(|f| f.path.as_str()).collect();
    assert!(paths.contains(&"CLAUDE.md"));
    assert!(paths.contains(&"AGENTS.md"));
    assert!(paths.contains(&".github/copilot-instructions.md"));
    assert!(paths.contains(&".cursor/rules/a.mdc"));
    assert!(paths.contains(&".cursor/rules/b.md"));
    assert!(!paths.contains(&".cursor/rules/c.txt"));
    let agents_md = assets.files.iter().find(|f| f.path == "AGENTS.md").unwrap();
    assert!(agents_md.agents.contains(&"codex".to_string()));

    // agent 状态:claude/codex/copilot/cursor 应报「已配置」
    let configured = |id: &str| {
        assets
            .agents
            .iter()
            .find(|a| a.id == id)
            .map(|a| !a.configs.is_empty())
            .unwrap_or(false)
    };
    assert!(configured("claude"));
    assert!(configured("codex"));
    assert!(configured("copilot"));
    assert!(configured("cursor"));
    assert!(!configured("gemini"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn reads_mcp_servers_and_tolerates_corrupt() {
    let dir = temp_project_dir("mcp");
    fs::write(
        dir.join(".mcp.json"),
        r#"{"mcpServers":{"b":{"command":"b"},"a":{"url":"https://a"}}}"#,
    )
    .unwrap();
    fs::create_dir_all(dir.join(".cursor")).unwrap();
    fs::write(dir.join(".cursor/mcp.json"), "not json").unwrap();

    let assets = scan_assets(&dir.to_string_lossy()).unwrap();
    assert_eq!(assets.mcp.len(), 2);
    let root_mcp = assets.mcp.iter().find(|m| m.path == ".mcp.json").unwrap();
    assert_eq!(root_mcp.dialect, "claude");
    assert_eq!(root_mcp.agents, vec!["claude".to_string()]);
    let names: Vec<&str> = root_mcp.servers.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["a", "b"]);
    assert_eq!(root_mcp.servers[0].config["url"], "https://a");
    let cursor_mcp = assets
        .mcp
        .iter()
        .find(|m| m.path == ".cursor/mcp.json")
        .unwrap();
    assert!(cursor_mcp.servers.is_empty());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn scans_project_skills_with_frontmatter() {
    let dir = temp_project_dir("skills");
    let skill = dir.join(".claude/skills/demo");
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: demo-skill\ndescription: \"做演示\"\n---\n\n# Demo\n",
    )
    .unwrap();
    // 无 SKILL.md 的目录不算技能
    fs::create_dir_all(dir.join(".claude/skills/no-md")).unwrap();

    let assets = scan_assets(&dir.to_string_lossy()).unwrap();
    assert_eq!(assets.skills.len(), 1);
    assert_eq!(assets.skills[0].dir, ".claude/skills/demo");
    assert_eq!(assets.skills[0].name, "demo-skill");
    assert_eq!(assets.skills[0].description, "做演示");
    assert_eq!(
        assets.skills[0].description_token_count,
        count_o200k_tokens("做演示")
    );
    assert_eq!(
        assets.skills[0].token_count,
        count_o200k_tokens("---\nname: demo-skill\ndescription: \"做演示\"\n---\n\n# Demo\n")
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn scans_multiple_skill_dirs_and_dedupes_by_name() {
    let dir = temp_project_dir("skills-dedup");
    // .agents/skills 独有技能
    let agents_only = dir.join(".agents/skills/review");
    fs::create_dir_all(&agents_only).unwrap();
    fs::write(agents_only.join("SKILL.md"), "---\nname: review\n---\n").unwrap();
    // .zcode/skills 独有技能
    let zcode_only = dir.join(".zcode/skills/zcode-skill");
    fs::create_dir_all(&zcode_only).unwrap();
    fs::write(zcode_only.join("SKILL.md"), "---\nname: zcode-skill\n---\n").unwrap();
    // 三个目录同名技能:.claude/skills 优先保留
    for rel in [
        ".claude/skills/shared",
        ".agents/skills/shared",
        ".zcode/skills/shared",
    ] {
        let shared = dir.join(rel);
        fs::create_dir_all(&shared).unwrap();
        fs::write(shared.join("SKILL.md"), "---\nname: shared\n---\n").unwrap();
    }

    let assets = scan_assets(&dir.to_string_lossy()).unwrap();
    let names: Vec<&str> = assets.skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["review", "shared", "zcode-skill"]);
    assert_eq!(assets.skills[0].dir, ".agents/skills/review");
    assert_eq!(assets.skills[1].dir, ".claude/skills/shared");
    assert_eq!(assets.skills[2].dir, ".zcode/skills/zcode-skill");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn frontmatter_requires_opening_fence() {
    assert_eq!(parse_skill_frontmatter("# hi\nname: x\n"), (None, None));
    let (name, desc) = parse_skill_frontmatter("---\nname: x\n---\nbody");
    assert_eq!(name, Some("x".to_string()));
    assert_eq!(desc, None);
}

#[test]
fn cc_skill_export_roundtrip() {
    let cc = temp_project_dir("cc-src");
    let project = temp_project_dir("cc-dst");
    let src = cc.join("skills/zip-skill");
    fs::create_dir_all(src.join("scripts")).unwrap();
    fs::write(src.join("SKILL.md"), "---\nname: zip\n---\n").unwrap();
    fs::write(src.join("scripts/run.py"), "print()").unwrap();

    set_project_cc_skill_at(&cc, &project, "zip-skill", true).unwrap();
    assert!(project.join(".claude/skills/zip-skill/SKILL.md").is_file());
    assert!(project
        .join(".claude/skills/zip-skill/scripts/run.py")
        .is_file());

    // 重复导出 = 整体替换(源里新增文件也带上)
    fs::write(src.join("extra.txt"), "x").unwrap();
    set_project_cc_skill_at(&cc, &project, "zip-skill", true).unwrap();
    assert!(project.join(".claude/skills/zip-skill/extra.txt").is_file());

    set_project_cc_skill_at(&cc, &project, "zip-skill", false).unwrap();
    assert!(!project.join(".claude/skills/zip-skill").exists());

    // 路径穿越与缺失源
    assert!(set_project_cc_skill_at(&cc, &project, "../evil", true).is_err());
    assert!(set_project_cc_skill_at(&cc, &project, "missing", true).is_err());
    // 取消不存在的目录幂等
    set_project_cc_skill_at(&cc, &project, "missing", false).unwrap();

    let _ = fs::remove_dir_all(&cc);
    let _ = fs::remove_dir_all(&project);
}

#[test]
fn mcp_upsert_merges_and_remove_is_idempotent() {
    let dir = temp_project_dir("mcp-upsert");
    let target = dir.join(".mcp.json");

    // 项目已有自有条目,合并不覆盖
    fs::write(
        &target,
        r#"{"mcpServers":{"own":{"command":"own"}},"other":1}"#,
    )
    .unwrap();
    upsert_mcp_server(
        &target,
        "mcpServers",
        "web",
        json!({"command": "npx", "args": ["mcp-web"]}),
    )
    .unwrap();
    let doc: Value = serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
    assert!(doc["mcpServers"]["own"].is_object());
    assert_eq!(doc["mcpServers"]["web"]["command"], json!("npx"));
    assert_eq!(doc["other"], json!(1));

    // 覆盖同名键
    upsert_mcp_server(&target, "mcpServers", "web", json!({"url": "https://x"})).unwrap();
    let doc: Value = serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
    assert_eq!(doc["mcpServers"]["web"]["url"], json!("https://x"));

    remove_mcp_server(&target, "mcpServers", "web").unwrap();
    let doc: Value = serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
    assert!(doc["mcpServers"]["web"].is_null());
    assert!(doc["mcpServers"]["own"].is_object());
    // 再删幂等;文件不存在也幂等
    remove_mcp_server(&target, "mcpServers", "web").unwrap();
    remove_mcp_server(&dir.join("nope/.mcp.json"), "mcpServers", "web").unwrap();

    // 损坏 JSON 报错且不改写
    fs::write(&target, "not json").unwrap();
    assert!(upsert_mcp_server(&target, "mcpServers", "a", json!({})).is_err());
    assert_eq!(fs::read_to_string(&target).unwrap(), "not json");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn create_and_delete_project_skill_roundtrip() {
    let dir = temp_project_dir("skill-crud");
    let created = create_project_skill(
        dir.to_string_lossy().to_string(),
        " my-skill ".to_string(),
        "做演示: \"quoted\"\n第二行".to_string(),
    )
    .unwrap();
    assert_eq!(created, ".claude/skills/my-skill");
    let md = dir.join(".claude/skills/my-skill/SKILL.md");
    let content = fs::read_to_string(&md).unwrap();
    assert!(content.starts_with("---\nname: my-skill\n"));
    assert!(content.contains("description: \"做演示: \\\"quoted\\\" 第二行\""));
    // 再建同名报错
    assert!(create_project_skill(dir.to_string_lossy().to_string(), "my-skill".into(), String::new()).is_err());
    // 删除后可重建
    delete_project_skill(dir.to_string_lossy().to_string(), created).unwrap();
    assert!(!dir.join(".claude/skills/my-skill").exists());
    delete_project_skill(dir.to_string_lossy().to_string(), ".claude/skills/my-skill".into()).unwrap();

    // 路径穿越与越界目录
    assert!(delete_project_skill(dir.to_string_lossy().to_string(), "../evil".into()).is_err());
    assert!(delete_project_skill(dir.to_string_lossy().to_string(), ".claude/skills/a/b".into()).is_err());
    assert!(delete_project_skill(dir.to_string_lossy().to_string(), "not-skills/x".into()).is_err());
    // 允许 .agents/.zcode 下的技能
    let alt = dir.join(".agents/skills/alt");
    fs::create_dir_all(&alt).unwrap();
    delete_project_skill(dir.to_string_lossy().to_string(), ".agents/skills/alt".into()).unwrap();
    assert!(!alt.exists());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn project_mcp_server_crud_rejects_unknown_config_path() {
    let dir = temp_project_dir("mcp-crud");
    let project = dir.to_string_lossy().to_string();

    set_project_mcp_server(
        project.clone(),
        ".mcp.json".into(),
        "web".into(),
        json!({"type": "stdio", "command": "npx", "args": ["-y", "srv"]}),
    )
    .unwrap();
    // 目标文件不存在时自动创建(仅含 mcpServers)
    let doc: Value = serde_json::from_str(&fs::read_to_string(dir.join(".mcp.json")).unwrap()).unwrap();
    assert_eq!(doc["mcpServers"]["web"]["command"], json!("npx"));

    // VS Code 键名为 servers 的文件同样可写
    fs::create_dir_all(dir.join(".vscode")).unwrap();
    fs::write(dir.join(".vscode/mcp.json"), r#"{"servers":{"keep":{}}}"#).unwrap();
    set_project_mcp_server(
        project.clone(),
        ".vscode/mcp.json".into(),
        "remote".into(),
        json!({"type": "http", "url": "https://mcp.example.com"}),
    )
    .unwrap();
    let doc: Value =
        serde_json::from_str(&fs::read_to_string(dir.join(".vscode/mcp.json")).unwrap()).unwrap();
    assert!(doc["servers"]["keep"].is_object());
    assert_eq!(doc["servers"]["remote"]["url"], json!("https://mcp.example.com"));

    remove_project_mcp_server(project.clone(), ".vscode/mcp.json".into(), "remote".into()).unwrap();
    let doc: Value =
        serde_json::from_str(&fs::read_to_string(dir.join(".vscode/mcp.json")).unwrap()).unwrap();
    assert!(doc["servers"]["remote"].is_null());

    // 只允许探测表内的配置文件;服务器名/非对象定义拒绝
    assert!(set_project_mcp_server(project.clone(), "evil.json".into(), "x".into(), json!({})).is_err());
    assert!(set_project_mcp_server(project.clone(), ".mcp.json".into(), "a/b".into(), json!({})).is_err());
    assert!(set_project_mcp_server(project.clone(), ".mcp.json".into(), "x".into(), json!("str")).is_err());
    assert!(remove_project_mcp_server(project, ".mcp.json".into(), String::new()).is_err());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn multi_dialect_scan_and_crud_preserves_foreign_content() {
    let dir = temp_project_dir("mcp-dialects");
    let project = dir.to_string_lossy().to_string();

    // gemini:settings.json 的 mcpServers,文件里其他设置键保留
    fs::create_dir_all(dir.join(".gemini")).unwrap();
    fs::write(
        dir.join(".gemini/settings.json"),
        r#"{"theme":"auto","mcpServers":{"keep":{"command":"keep-cmd"}}}"#,
    )
    .unwrap();
    set_project_mcp_server(
        project.clone(),
        ".gemini/settings.json".into(),
        "web".into(),
        json!({"command": "npx", "args": ["-y", "srv"]}),
    )
    .unwrap();
    let doc: Value =
        serde_json::from_str(&fs::read_to_string(dir.join(".gemini/settings.json")).unwrap())
            .unwrap();
    assert_eq!(doc["theme"], json!("auto"));
    assert!(doc["mcpServers"]["keep"].is_object());
    assert_eq!(doc["mcpServers"]["web"]["command"], json!("npx"));

    // opencode:opencode.json 的 mcp 键(command 为数组),其余配置键保留
    fs::write(
        dir.join("opencode.json"),
        r#"{"$schema":"https://opencode.ai/config.json","mcp":{"keep":{"type":"remote","url":"https://k"}}}"#,
    )
    .unwrap();
    set_project_mcp_server(
        project.clone(),
        "opencode.json".into(),
        "local".into(),
        json!({"type": "local", "command": ["npx", "-y", "srv"], "environment": {"A": "B"}}),
    )
    .unwrap();
    let doc: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("opencode.json")).unwrap()).unwrap();
    assert!(doc["$schema"].is_string());
    assert_eq!(doc["mcp"]["local"]["command"], json!(["npx", "-y", "srv"]));
    assert_eq!(doc["mcp"]["local"]["environment"]["A"], json!("B"));

    // codex:config.toml 的 mcp_servers 表;手写注释/既有键/既有条目格式保留
    fs::create_dir_all(dir.join(".codex")).unwrap();
    fs::write(
        dir.join(".codex/config.toml"),
        "# my codex config\nmodel = \"gpt-5\"\n\n[mcp_servers.keep]\ncommand = \"keep-cmd\"\n",
    )
    .unwrap();
    set_project_mcp_server(
        project.clone(),
        ".codex/config.toml".into(),
        "web".into(),
        json!({"command": "npx", "args": ["-y", "srv"], "env": {"K": "V"}}),
    )
    .unwrap();
    let text = fs::read_to_string(dir.join(".codex/config.toml")).unwrap();
    assert!(text.contains("# my codex config"));
    assert!(text.contains("model = \"gpt-5\""));
    assert!(text.contains("[mcp_servers.keep]"));
    assert!(text.contains("[mcp_servers.web.env]"));
    assert!(text.contains("K = \"V\""));
    remove_project_mcp_server(project.clone(), ".codex/config.toml".into(), "web".into()).unwrap();
    let text = fs::read_to_string(dir.join(".codex/config.toml")).unwrap();
    assert!(text.contains("[mcp_servers.keep]"));
    assert!(!text.contains("mcp_servers.web"));

    // zcode:.zcode/config.json 的嵌套 mcp.servers 键,其他配置键保留
    fs::create_dir_all(dir.join(".zcode")).unwrap();
    fs::write(
        dir.join(".zcode/config.json"),
        r#"{"plugins":{"x":true},"mcp":{"servers":{"keep":{"type":"stdio","command":"k","enable":false}}}}"#,
    )
    .unwrap();
    set_project_mcp_server(
        project.clone(),
        ".zcode/config.json".into(),
        "web".into(),
        json!({"type": "stdio", "command": "npx", "args": ["-y", "srv"]}),
    )
    .unwrap();
    let doc: Value =
        serde_json::from_str(&fs::read_to_string(dir.join(".zcode/config.json")).unwrap())
            .unwrap();
    assert!(doc["plugins"]["x"].is_boolean());
    assert_eq!(doc["mcp"]["servers"]["web"]["command"], json!("npx"));
    // 既有条目的 enable 停用标记保留(表单不托管该键)
    assert_eq!(doc["mcp"]["servers"]["keep"]["enable"], json!(false));
    remove_project_mcp_server(project.clone(), ".zcode/config.json".into(), "keep".into()).unwrap();
    let doc: Value =
        serde_json::from_str(&fs::read_to_string(dir.join(".zcode/config.json")).unwrap())
            .unwrap();
    assert!(doc["mcp"]["servers"]["keep"].is_null());
    assert!(doc["mcp"]["servers"]["web"].is_object());

    // 扫描:全部 7 个目标上报(含未创建文件);存在的文件带方言与归属 agent
    fs::write(dir.join(".mcp.json"), r#"{"mcpServers":{}}"#).unwrap();
    let assets = scan_assets(&project).unwrap();
    assert_eq!(assets.mcp_targets.len(), 7);
    let paths: Vec<&str> = assets.mcp_targets.iter().map(|t| t.path.as_str()).collect();
    assert!(paths.contains(&".codex/config.toml"));
    assert!(paths.contains(&".zcode/config.json"));
    assert_eq!(assets.mcp.len(), 5);
    let gem = assets
        .mcp
        .iter()
        .find(|m| m.path == ".gemini/settings.json")
        .unwrap();
    assert_eq!(gem.dialect, "gemini");
    assert_eq!(gem.agents, vec!["gemini".to_string()]);
    let codex = assets.mcp.iter().find(|m| m.path == ".codex/config.toml").unwrap();
    assert_eq!(codex.dialect, "codex");
    assert_eq!(codex.servers[0].name, "keep");
    assert_eq!(codex.servers[0].config["command"], json!("keep-cmd"));
    let oc = assets.mcp.iter().find(|m| m.path == "opencode.json").unwrap();
    assert_eq!(oc.dialect, "opencode");
    let zc = assets
        .mcp
        .iter()
        .find(|m| m.path == ".zcode/config.json")
        .unwrap();
    assert_eq!(zc.dialect, "claude");
    assert_eq!(zc.agents, vec!["zcode".to_string()]);
    assert_eq!(zc.servers[0].name, "web");

    // 损坏 TOML:扫描容忍(条目为空但文件仍列出),写侧报错不改写
    fs::write(dir.join(".codex/config.toml"), "not [toml").unwrap();
    let assets = scan_assets(&project).unwrap();
    let codex = assets.mcp.iter().find(|m| m.path == ".codex/config.toml").unwrap();
    assert!(codex.servers.is_empty());
    assert!(set_project_mcp_server(
        project.clone(),
        ".codex/config.toml".into(),
        "x".into(),
        json!({"command": "c"}),
    )
    .is_err());

    let _ = fs::remove_dir_all(&dir);
}
