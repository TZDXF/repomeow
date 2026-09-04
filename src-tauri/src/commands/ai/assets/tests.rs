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
    assert_eq!(root_mcp.servers_key, "mcpServers");
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
