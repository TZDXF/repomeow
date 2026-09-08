use super::*;

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
fn scans_multiple_skill_dirs_without_conflating_same_names() {
    let dir = temp_project_dir("skills-dedup");
    // .agents/skills 独有技能
    let agents_only = dir.join(".agents/skills/review");
    fs::create_dir_all(&agents_only).unwrap();
    fs::write(agents_only.join("SKILL.md"), "---\nname: review\n---\n").unwrap();
    // .zcode/skills 独有技能
    let zcode_only = dir.join(".zcode/skills/zcode-skill");
    fs::create_dir_all(&zcode_only).unwrap();
    fs::write(zcode_only.join("SKILL.md"), "---\nname: zcode-skill\n---\n").unwrap();
    // 三个目录同名技能:保留各自实例
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
    assert_eq!(
        names,
        ["review", "shared", "shared", "shared", "zcode-skill"]
    );
    assert_eq!(assets.skills[0].dir, ".agents/skills/review");
    let dirs: Vec<_> = assets.skills.iter().map(|s| s.dir.as_str()).collect();
    assert!(dirs.contains(&".claude/skills/shared"));
    assert!(dirs.contains(&".agents/skills/shared"));
    assert!(dirs.contains(&".zcode/skills/shared"));
    assert!(dirs.contains(&".zcode/skills/zcode-skill"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn frontmatter_requires_opening_fence() {
    assert_eq!(parse_skill_frontmatter("# hi\nname: x\n"), (None, None));
    let (name, desc) = parse_skill_frontmatter("---\nname: x\n---\nbody");
    assert_eq!(name, Some("x".to_string()));
    assert_eq!(desc, None);
}
