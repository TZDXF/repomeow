use super::*;
use serde_json::json;

struct Fixture {
    base: PathBuf,
    library: Library,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "repomeow-deploy-{}-{}",
            std::process::id(),
            crate::time_util::now_ts_nanos()
        ));
        let root = base.join("project");
        fs::create_dir_all(&root).unwrap();
        let library = Library::new(base.join("data/resource-library"));
        library.ensure().unwrap();
        Self {
            root: fs::canonicalize(root).unwrap(),
            base,
            library,
        }
    }
    fn skill(&self, id: &str, directory: &str, body: &str) {
        let mut skills: SkillLibrary = self.library.read_plain_json(FILE_SKILLS).unwrap();
        skills.skills.retain(|s| s.id != id);
        skills.skills.push(serde_json::from_value(json!({ "id": id, "directory": directory, "name": directory, "description": "", "groupIds": [], "createdAt": 0, "updatedAt": 0 })).unwrap());
        self.library.write_plain_json(FILE_SKILLS, &skills).unwrap();
        self.library.write_body(directory, body).unwrap();
    }
    fn mcp(&self) {
        self.library.write_mcp_json(&vec![json!({ "id": "m1", "name": "context", "description": null, "transport": "stdio", "command": "node", "args": ["server.js"], "env": {"KEY":"secret"}, "url": null, "headers": {}, "enabled": true, "createdAt":0,"updatedAt":0 })]).unwrap();
    }
    fn apply(&self, kind: &str, agent: &str, ids: &[&str]) -> ApplyResult {
        apply(
            &self.library,
            &self.root,
            kind,
            agent,
            &ids.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // 测试只删除明确创建的临时根,解析后验证仍处于系统 TEMP 下。
        if let (Ok(base), Ok(temp)) = (
            fs::canonicalize(&self.base),
            fs::canonicalize(std::env::temp_dir()),
        ) {
            if base.starts_with(&temp) && base != temp {
                let _ = fs::remove_dir_all(base);
            }
        }
    }
}

#[test]
fn skills_copy_all_files_deduplicate_and_update_manually() {
    let f = Fixture::new();
    f.skill("s1", "review", "v1");
    fs::create_dir_all(f.library.root().join("skills/review/references")).unwrap();
    fs::write(
        f.library
            .root()
            .join("skills/review/references/example.bin"),
        [0, 255, 2],
    )
    .unwrap();
    let result = f.apply("skills", "claude", &["s1", "s1"]);
    assert_eq!(result.applied, 1);
    assert!(result.failures.is_empty());
    assert_eq!(
        fs::read(f.root.join(".claude/skills/review/references/example.bin")).unwrap(),
        [0, 255, 2]
    );
    assert_eq!(f.apply("skills", "claude", &["s1"]).applied, 0);
    f.skill("s1", "review", "v2");
    assert_eq!(
        snapshot(&f.library, &f.root, "skills").unwrap().deployments[0].status,
        "update"
    );
    assert_eq!(
        fs::read_to_string(f.root.join(".claude/skills/review/SKILL.md")).unwrap(),
        "v1"
    );
    assert_eq!(f.apply("skills", "claude", &["s1"]).applied, 1);
    assert_eq!(f.apply("skills", "claude", &[]).applied, 1);
    assert!(!f.root.join(".claude/skills/review").exists());
    assert!(f.library.root().join("skills/review/SKILL.md").exists());
}

#[test]
fn refuses_unmanaged_and_modified_resources() {
    let f = Fixture::new();
    f.skill("s1", "review", "library");
    fs::create_dir_all(f.root.join(".claude/skills/review")).unwrap();
    fs::write(f.root.join(".claude/skills/review/SKILL.md"), "user").unwrap();
    assert_eq!(f.apply("skills", "claude", &["s1"]).failures.len(), 1);
    assert_eq!(
        fs::read_to_string(f.root.join(".claude/skills/review/SKILL.md")).unwrap(),
        "user"
    );
    assert_eq!(f.apply("skills", "codex", &["s1"]).applied, 1);
    fs::write(f.root.join(".agents/skills/review/SKILL.md"), "local").unwrap();
    assert_eq!(f.apply("skills", "codex", &[]).failures.len(), 1);
    assert_eq!(
        snapshot(&f.library, &f.root, "skills").unwrap().deployments[0].status,
        "modified"
    );
}

#[test]
fn source_deletion_keeps_deployment_until_explicit_removal() {
    let f = Fixture::new();
    f.skill("s1", "review", "v1");
    f.apply("skills", "claude", &["s1"]);
    f.library
        .write_plain_json(FILE_SKILLS, &SkillLibrary::default())
        .unwrap();
    assert_eq!(
        snapshot(&f.library, &f.root, "skills").unwrap().deployments[0].status,
        "sourceMissing"
    );
    assert_eq!(f.apply("skills", "claude", &["s1"]).applied, 0);
    assert_eq!(f.apply("skills", "claude", &[]).applied, 1);
}

#[test]
fn mcp_preserves_other_settings_and_does_not_persist_secrets_in_manifest() {
    let f = Fixture::new();
    f.mcp();
    fs::create_dir_all(f.root.join(".gemini")).unwrap();
    fs::write(
        f.root.join(".gemini/settings.json"),
        r#"{"theme":"dark","mcpServers":{"existing":{"command":"other"}}}"#,
    )
    .unwrap();
    assert_eq!(f.apply("mcp", "gemini", &["m1"]).applied, 1);
    let file = f.root.join(".gemini/settings.json");
    let before: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    assert_eq!(before["theme"], "dark");
    assert_eq!(before["mcpServers"]["context"]["env"]["KEY"], "secret");
    assert!(!fs::read_to_string(manifest_file(&f.library, &f.root))
        .unwrap()
        .contains("secret"));
    assert_eq!(f.apply("mcp", "gemini", &[]).applied, 1);
    let after: Value = serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
    assert_eq!(
        after["mcpServers"]["existing"],
        before["mcpServers"]["existing"]
    );
    assert!(after["mcpServers"].get("context").is_none());
}

#[test]
fn corrupt_mcp_is_not_overwritten_and_toml_comments_survive() {
    let f = Fixture::new();
    f.mcp();
    fs::write(f.root.join(".mcp.json"), "{broken").unwrap();
    assert_eq!(f.apply("mcp", "claude", &["m1"]).failures.len(), 1);
    assert_eq!(
        fs::read_to_string(f.root.join(".mcp.json")).unwrap(),
        "{broken"
    );
    fs::create_dir_all(f.root.join(".codex")).unwrap();
    fs::write(
        f.root.join(".codex/config.toml"),
        "# keep this\nmodel = \"test\"\n[mcp_servers.existing]\ncommand = \"other\"\n",
    )
    .unwrap();
    assert_eq!(f.apply("mcp", "codex", &["m1"]).applied, 1);
    assert_eq!(f.apply("mcp", "codex", &["m1"]).applied, 0);
    assert_eq!(f.apply("mcp", "codex", &[]).applied, 1);
    let text = fs::read_to_string(f.root.join(".codex/config.toml")).unwrap();
    assert!(text.contains("# keep this"));
    assert!(text.contains("mcp_servers.existing"));
}

#[test]
fn rejects_path_escape_and_stable_json_hash_ignores_key_order() {
    let f = Fixture::new();
    assert!(safe_path(&f.root, "../outside").is_err());
    assert!(safe_path(&f.root, "C:/outside").is_err());
    assert!(safe_path(&f.root, ".claude/file:stream").is_err());
    assert_eq!(
        json_hash(&serde_json::from_str::<Value>(r#"{"b":2,"a":{"x":1,"z":2}}"#).unwrap()),
        json_hash(&json!({"a":{"z":2,"x":1},"b":2}))
    );
}

#[test]
fn unsupported_sse_and_shared_reference_removal_are_safe() {
    let f = Fixture::new();
    f.mcp();
    let mut servers: Vec<McpServer> = f.library.read_mcp_json().unwrap();
    servers[0].transport = "sse".into();
    servers[0].url = Some("https://example.test/sse".into());
    assert!(definition(&servers[0], mcp_target("codex").unwrap()).is_err());
    assert_eq!(
        definition(&servers[0], mcp_target("gemini").unwrap()).unwrap()["url"],
        "https://example.test/sse"
    );
    f.skill("s1", "review", "v1");
    f.apply("skills", "claude", &["s1"]);
    // 直接覆盖内存记录构造共享路径,验证删除逻辑不破坏其他引用。
    let state_path = manifest_file(&f.library, &f.root);
    let mut state = read_manifest(&state_path, &f.root).unwrap();
    let mut peer = state.entries[0].clone();
    peer.agent_id = "other".into();
    state.entries.push(peer);
    assert!(apply_one(
        &f.library,
        &f.root,
        &state_path,
        &mut state,
        "skills",
        "claude",
        "s1",
        false,
        None
    )
    .unwrap());
    assert!(f.root.join(".claude/skills/review/SKILL.md").exists());
    assert_eq!(state.entries.len(), 1);
}

#[test]
fn all_agent_targets_roundtrip_and_preserve_unrelated_mcp_entries() {
    let f = Fixture::new();
    f.mcp();
    for agent in TARGETS {
        let target = mcp_target(agent.id).unwrap();
        if target.dialect != "codex" {
            let path = safe_path(&f.root, target.path).unwrap();
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, r#"{"keep":{"value":42}}"#).unwrap();
        }
        let added = f.apply("mcp", agent.id, &["m1"]);
        assert!(
            added.failures.is_empty(),
            "{}: {:?}",
            agent.id,
            added
                .failures
                .iter()
                .map(|e| &e.message)
                .collect::<Vec<_>>()
        );
        assert_eq!(added.applied, 1);
        let doc = McpDocument::read(&f.root, target).unwrap();
        let value = doc.entry(target, "context").unwrap().unwrap();
        if agent.id == "opencode" {
            assert_eq!(value["command"], json!(["node", "server.js"]));
            assert_eq!(value["environment"]["KEY"], "secret");
        } else {
            assert_eq!(value["command"], "node");
        }
        assert_eq!(f.apply("mcp", agent.id, &["m1"]).applied, 0);
        assert_eq!(f.apply("mcp", agent.id, &[]).applied, 1);
        if target.dialect != "codex" {
            let text = fs::read_to_string(f.root.join(target.path)).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&text).unwrap()["keep"]["value"],
                42
            );
        }
    }
}

#[test]
fn invalid_manifest_and_project_local_mcp_changes_fail_closed() {
    let f = Fixture::new();
    f.mcp();
    f.apply("mcp", "claude", &["m1"]);
    let before = snapshot(&f.library, &f.root, "mcp").unwrap().revision;
    let path = f.root.join(".mcp.json");
    fs::write(
        &path,
        r#"{"mcpServers":{"context":{"command":"user-command"}}}"#,
    )
    .unwrap();
    assert_eq!(f.apply("mcp", "claude", &[]).failures.len(), 1);
    assert_eq!(
        snapshot(&f.library, &f.root, "mcp").unwrap().deployments[0].status,
        "modified"
    );
    f.skill("s1", "review", "v1");
    f.apply("skills", "claude", &["s1"]);
    assert_ne!(
        snapshot(&f.library, &f.root, "mcp").unwrap().revision,
        before
    );
    let state_path = manifest_file(&f.library, &f.root);
    let mut state = read_manifest(&state_path, &f.root).unwrap();
    state.entries[0].path = "../outside".into();
    save_manifest(&state_path, &state).unwrap();
    assert!(read_manifest(&state_path, &f.root).is_err());
}

#[test]
fn locked_mcp_library_does_not_block_skills_or_remove_existing_mcp() {
    let f = Fixture::new();
    f.mcp();
    f.apply("mcp", "claude", &["m1"]);
    let mut meta = f.library.meta().unwrap();
    meta.encrypted = true;
    f.library.write_meta(&meta).unwrap();
    // 合法加密容器头 + nonce/tag 占位;无进程内密钥时应先返回 locked。
    let mut encrypted = b"RLENC1\x01".to_vec();
    encrypted.extend_from_slice(&[0; 40]);
    fs::write(f.library.root().join("mcp.json"), encrypted).unwrap();
    let state = snapshot(&f.library, &f.root, "mcp").unwrap();
    assert_eq!(
        state.source_error.as_deref(),
        Some("resource_library_locked")
    );
    assert_eq!(state.deployments[0].status, "sourceUnavailable");
    assert_eq!(f.apply("mcp", "claude", &["m1"]).applied, 0);
    f.skill("s1", "review", "v1");
    assert_eq!(f.apply("skills", "claude", &["s1"]).applied, 1);
    assert_eq!(f.apply("mcp", "claude", &[]).applied, 1);
}

#[test]
fn partial_batch_keeps_successful_resources_and_reports_each_failure() {
    let f = Fixture::new();
    f.skill("s1", "review", "v1");
    let result = f.apply("skills", "claude", &["s1", "unknown"]);
    assert_eq!(result.applied, 1);
    assert_eq!(result.failures.len(), 1);
    assert_eq!(result.failures[0].resource_id, "unknown");
    assert!(f.root.join(".claude/skills/review/SKILL.md").exists());
}

#[cfg(unix)]
#[test]
fn symlinks_cannot_escape_source_or_project() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    f.skill("s1", "review", "v1");
    let outside = f.base.join("outside");
    fs::create_dir_all(&outside).unwrap();
    symlink(&outside, f.root.join(".claude")).unwrap();
    assert_eq!(f.apply("skills", "claude", &["s1"]).failures.len(), 1);
    symlink(&outside, f.library.root().join("skills/review/link")).unwrap();
    assert_eq!(f.apply("skills", "codex", &["s1"]).failures.len(), 1);
}

#[test]
fn shortlist_add_assign_remove_flow() {
    let f = Fixture::new();
    f.skill("s1", "review", "v1");
    // 添加仅登记项目列表,不产生任何部署文件。
    let added = add(&f.library, &f.root, "skills", &["s1".to_string()]).unwrap();
    assert_eq!(added.applied, 1);
    let snap = snapshot(&f.library, &f.root, "skills").unwrap();
    assert_eq!(snap.shortlist, vec!["s1".to_string()]);
    assert!(snap.deployments.is_empty());
    assert!(!f.root.join(".claude/skills/review").exists());
    // 重复添加幂等;未知来源按条目报错。
    assert_eq!(
        add(&f.library, &f.root, "skills", &["s1".to_string()])
            .unwrap()
            .applied,
        0
    );
    let bad = add(&f.library, &f.root, "skills", &["ghost".to_string()]).unwrap();
    assert_eq!(bad.failures.len(), 1);
    assert_eq!(bad.failures[0].code, "resource_library_skill_not_found");
    // 配置到两个 Agent,shortlist 保持不变。
    let assigned = assign(
        &f.library,
        &f.root,
        "skills",
        "s1",
        &["claude".to_string(), "cursor".to_string()],
    )
    .unwrap();
    assert_eq!(assigned.applied, 2);
    assert!(f.root.join(".claude/skills/review/SKILL.md").exists());
    assert!(f.root.join(".cursor/skills/review/SKILL.md").exists());
    // 取消全部 Agent 后仍保留在列表中。
    let cleared = assign(&f.library, &f.root, "skills", "s1", &[]).unwrap();
    assert_eq!(cleared.applied, 2);
    assert!(!f.root.join(".claude/skills/review").exists());
    let snap = snapshot(&f.library, &f.root, "skills").unwrap();
    assert_eq!(snap.shortlist, vec!["s1".to_string()]);
    assert!(snap.deployments.is_empty());
    // 移除出列表;已部署资源移除时会先解除全部 Agent 配置。
    assign(&f.library, &f.root, "skills", "s1", &["claude".to_string()]).unwrap();
    remove_resource(&f.library, &f.root, "skills", "s1").unwrap();
    let snap = snapshot(&f.library, &f.root, "skills").unwrap();
    assert!(snap.shortlist.is_empty());
    assert!(snap.deployments.is_empty());
    assert!(!f.root.join(".claude/skills/review").exists());
}

#[test]
fn assign_auto_enlists_deployed_resource_and_rejects_unknown_agent() {
    let f = Fixture::new();
    f.skill("s1", "review", "v1");
    // 未先添加、直接配置:自动加入项目列表。
    assign(&f.library, &f.root, "skills", "s1", &["claude".to_string()]).unwrap();
    assert_eq!(
        snapshot(&f.library, &f.root, "skills").unwrap().shortlist,
        vec!["s1".to_string()]
    );
    assert!(assign(&f.library, &f.root, "skills", "s1", &["ghost".to_string()]).is_err());
}

#[test]
fn skill_choice_carries_marketplace_source() {
    let f = Fixture::new();
    f.skill("s1", "review", "v1");
    f.skill("s2", "local", "v1");
    let mut skills: SkillLibrary = f.library.read_plain_json(FILE_SKILLS).unwrap();
    let skill = skills.skills.iter_mut().find(|s| s.id == "s1").unwrap();
    skill.marketplace = Some(crate::commands::ai::resource_library::MarketplaceSource {
        source: "owner/repo".to_string(),
        ..Default::default()
    });
    f.library.write_plain_json(FILE_SKILLS, &skills).unwrap();
    let snap = snapshot(&f.library, &f.root, "skills").unwrap();
    let by_id = |id: &str| snap.resources.iter().find(|r| r.id == id).unwrap();
    assert_eq!(by_id("s1").source.as_deref(), Some("owner/repo"));
    assert_eq!(by_id("s2").source, None);
}
#[test]
fn import_unmanaged_skill_enlists_with_current_content() {
    let f = Fixture::new();
    // 项目里手写的技能(非托管):含 SKILL.md 之外的文件
    let dir = f.root.join(".claude/skills/manual");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        "---\nname: manual\ndescription: 手写\n---\nbody",
    )
    .unwrap();
    fs::write(dir.join("extra.txt"), "x").unwrap();
    let outcome = import_skill(&f.library, &f.root, ".claude/skills/manual").unwrap();
    assert!(!outcome.existed);
    // 收入资源库(含全部文件)
    let skills: SkillLibrary = f.library.read_plain_json(FILE_SKILLS).unwrap();
    let skill = skills
        .skills
        .iter()
        .find(|s| s.id == outcome.resource_id)
        .unwrap();
    assert!(f
        .library
        .root()
        .join(DIR_SKILLS)
        .join(&skill.directory)
        .join("extra.txt")
        .exists());
    // 认领为托管且指纹取现状 → configured
    let snap = snapshot(&f.library, &f.root, "skills").unwrap();
    assert_eq!(snap.shortlist, vec![outcome.resource_id.clone()]);
    assert_eq!(snap.deployments.len(), 1);
    assert_eq!(snap.deployments[0].status, "configured");
    assert_eq!(snap.deployments[0].entry.agent_id, "claude");
    // 另一个 Agent 目录下同名技能:复用库中已有条目,内容不同 → 可更新
    let dir2 = f.root.join(".cursor/skills/other");
    fs::create_dir_all(&dir2).unwrap();
    fs::write(dir2.join("SKILL.md"), "---\nname: manual\n---\nother body").unwrap();
    let outcome2 = import_skill(&f.library, &f.root, ".cursor/skills/other").unwrap();
    assert!(outcome2.existed);
    assert_eq!(outcome2.resource_id, outcome.resource_id);
    let snap = snapshot(&f.library, &f.root, "skills").unwrap();
    let cursor = snap
        .deployments
        .iter()
        .find(|d| d.entry.agent_id == "cursor")
        .unwrap();
    assert_eq!(cursor.status, "update");
    // 目录归属不明或穿越路径一律拒绝
    assert!(import_skill(&f.library, &f.root, ".claude/skills/..").is_err());
    assert!(import_skill(&f.library, &f.root, "docs/guide").is_err());
}

#[test]
fn import_unmanaged_mcp_enlists_with_current_value() {
    let f = Fixture::new();
    fs::write(
        f.root.join(".mcp.json"),
        serde_json::to_string_pretty(&json!({
            "mcpServers": {
                "context": {"type": "stdio", "command": "node", "args": ["server.js"], "env": {"KEY": "secret"}}
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let outcome = import_mcp(&f.library, &f.root, ".mcp.json", "context").unwrap();
    assert!(!outcome.existed);
    let list: Vec<McpServer> = f.library.read_mcp_json().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].command.as_deref(), Some("node"));
    assert_eq!(list[0].env.get("KEY").map(String::as_str), Some("secret"));
    let snap = snapshot(&f.library, &f.root, "mcp").unwrap();
    assert_eq!(snap.deployments.len(), 1);
    assert_eq!(snap.deployments[0].status, "configured");
    assert_eq!(snap.deployments[0].entry.agent_id, "claude");
    assert_eq!(snap.shortlist, vec![outcome.resource_id.clone()]);
    // 不存在的服务器/未知文件一律报错
    assert!(import_mcp(&f.library, &f.root, ".mcp.json", "ghost").is_err());
    assert!(import_mcp(&f.library, &f.root, "mcp.json", "context").is_err());
}

#[test]
fn parse_server_value_covers_project_dialects() {
    let claude = &MCP_TARGETS[0];
    let def = parse_server_value(
        claude,
        "s",
        &json!({"type": "sse", "url": "https://x", "headers": {"A": "b"}}),
    )
    .unwrap();
    assert_eq!(def.transport, "sse");
    assert_eq!(def.url.as_deref(), Some("https://x"));
    assert_eq!(def.headers.get("A").map(String::as_str), Some("b"));
    let opencode = MCP_TARGETS
        .iter()
        .find(|t| t.dialect == "opencode")
        .unwrap();
    let def = parse_server_value(
        opencode,
        "s",
        &json!({"type": "local", "command": ["npx", "srv", "--x"], "environment": {"K": "v"}}),
    )
    .unwrap();
    assert_eq!(def.command.as_deref(), Some("npx"));
    assert_eq!(def.args, vec!["srv".to_string(), "--x".to_string()]);
    assert_eq!(def.env.get("K").map(String::as_str), Some("v"));
    let codex = MCP_TARGETS.iter().find(|t| t.dialect == "codex").unwrap();
    let def = parse_server_value(
        codex,
        "s",
        &json!({"url": "https://x", "http_headers": {"A": "b"}}),
    )
    .unwrap();
    assert_eq!(def.transport, "http");
    assert_eq!(def.headers.get("A").map(String::as_str), Some("b"));
    let gemini = MCP_TARGETS.iter().find(|t| t.dialect == "gemini").unwrap();
    let def = parse_server_value(gemini, "s", &json!({"httpUrl": "https://x"})).unwrap();
    assert_eq!(def.transport, "http");
    let def = parse_server_value(gemini, "s", &json!({"url": "https://x"})).unwrap();
    assert_eq!(def.transport, "sse");
    // 既无 command 也无 url 的定义拒绝
    assert!(parse_server_value(claude, "s", &json!({})).is_err());
    assert!(parse_server_value(opencode, "s", &json!({"type": "local"})).is_err());
}

#[test]
fn claim_local_skill_configures_agents_without_library_import() {
    let f = Fixture::new();
    let dir = f.root.join(".zcode/skills/release-tagger");
    fs::create_dir_all(&dir).unwrap();
    let body = "---\nname: release-tagger\ndescription: 打 tag\n---\nv1\n";
    fs::write(dir.join("SKILL.md"), body).unwrap();

    // 认领:不入库,来源 Agent(zcode)按现状登记为已配置。
    let outcome = claim_local(
        &f.library,
        &f.root,
        "skills",
        ".zcode/skills/release-tagger",
        None,
    )
    .unwrap();
    assert_eq!(
        outcome.resource_id,
        "local:skills:.zcode/skills/release-tagger"
    );
    let data: SkillLibrary = f.library.read_plain_json(FILE_SKILLS).unwrap();
    assert!(data.skills.is_empty());
    // 幂等:重复认领返回同一 ID,不产生重复记录。
    let again = claim_local(
        &f.library,
        &f.root,
        "skills",
        ".zcode/skills/release-tagger",
        None,
    )
    .unwrap();
    assert_eq!(again.resource_id, outcome.resource_id);
    let snap = snapshot(&f.library, &f.root, "skills").unwrap();
    assert_eq!(snap.resources.len(), 1);
    assert_eq!(snap.resources[0].name, "release-tagger");
    assert_eq!(snap.deployments.len(), 1);
    assert_eq!(snap.deployments[0].entry.agent_id, "zcode");
    assert_eq!(snap.deployments[0].status, "configured");

    // 配置到 claude:复制内容;解除 claude:删除副本,来源不动。
    let r = assign(
        &f.library,
        &f.root,
        "skills",
        &outcome.resource_id,
        &["zcode".to_string(), "claude".to_string()],
    )
    .unwrap();
    assert!(r.failures.is_empty());
    assert_eq!(r.applied, 1);
    assert_eq!(
        fs::read_to_string(f.root.join(".claude/skills/release-tagger/SKILL.md")).unwrap(),
        body
    );

    // 来源编辑后:来源 Agent 显示 modified,其他 Agent 显示 update,重新 assign 应用更新。
    let body_v2 = body.replace("v1", "v2");
    fs::write(dir.join("SKILL.md"), &body_v2).unwrap();
    let snap = snapshot(&f.library, &f.root, "skills").unwrap();
    let status = |agent: &str| {
        snap.deployments
            .iter()
            .find(|d| d.entry.agent_id == agent)
            .unwrap()
            .status
            .clone()
    };
    assert_eq!(status("zcode"), "modified");
    assert_eq!(status("claude"), "update");
    let r = assign(
        &f.library,
        &f.root,
        "skills",
        &outcome.resource_id,
        &["zcode".to_string(), "claude".to_string()],
    )
    .unwrap();
    assert_eq!(r.applied, 1);
    assert_eq!(
        fs::read_to_string(f.root.join(".claude/skills/release-tagger/SKILL.md")).unwrap(),
        body_v2
    );

    let r = assign(
        &f.library,
        &f.root,
        "skills",
        &outcome.resource_id,
        &["zcode".to_string()],
    )
    .unwrap();
    assert_eq!(r.applied, 1);
    assert!(!f.root.join(".claude/skills/release-tagger").exists());
    assert!(dir.join("SKILL.md").exists());

    // 解除来源 Agent:仅删记录保留文件;最后一个部署解除时清理本地记录,回到非托管。
    let r = assign(&f.library, &f.root, "skills", &outcome.resource_id, &[]).unwrap();
    assert_eq!(r.applied, 1);
    assert!(dir.join("SKILL.md").exists());
    let snap = snapshot(&f.library, &f.root, "skills").unwrap();
    assert!(snap.deployments.is_empty());
    assert!(snap.resources.is_empty());
}

#[test]
fn claim_local_mcp_deploys_translated_and_preserves_origin() {
    let f = Fixture::new();
    fs::create_dir_all(f.root.join(".zcode")).unwrap();
    fs::write(
        f.root.join(".zcode/config.json"),
        json!({ "mcp": { "servers": { "ctx": { "command": "node", "args": ["s.js"] } } } })
            .to_string(),
    )
    .unwrap();
    let outcome = claim_local(
        &f.library,
        &f.root,
        "mcp",
        ".zcode/config.json",
        Some("ctx"),
    )
    .unwrap();
    assert_eq!(outcome.resource_id, "local:mcp:.zcode/config.json#ctx");
    // 不入库
    let servers: Vec<McpServer> = f.library.read_mcp_json().unwrap();
    assert!(servers.is_empty());

    let r = assign(
        &f.library,
        &f.root,
        "mcp",
        &outcome.resource_id,
        &["zcode".to_string(), "claude".to_string()],
    )
    .unwrap();
    assert!(r.failures.is_empty());
    let doc: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(f.root.join(".mcp.json")).unwrap()).unwrap();
    assert_eq!(doc["mcpServers"]["ctx"]["command"], "node");

    // 移除:副本删除,来源文件保持原样,本地记录清理。
    let r = remove_resource(&f.library, &f.root, "mcp", &outcome.resource_id).unwrap();
    assert!(r.failures.is_empty());
    let doc: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(f.root.join(".mcp.json")).unwrap()).unwrap();
    assert!(doc["mcpServers"].get("ctx").is_none());
    let origin: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(f.root.join(".zcode/config.json")).unwrap())
            .unwrap();
    assert_eq!(origin["mcp"]["servers"]["ctx"]["command"], "node");
    let snap = snapshot(&f.library, &f.root, "mcp").unwrap();
    assert!(snap.deployments.is_empty());
    assert!(snap.resources.is_empty());
}

#[test]
fn delete_unmanaged_removes_files_and_rejects_managed() {
    let f = Fixture::new();
    // skills:未托管目录整体删除
    fs::create_dir_all(f.root.join(".claude/skills/local-skill")).unwrap();
    fs::write(f.root.join(".claude/skills/local-skill/SKILL.md"), "body").unwrap();
    delete_unmanaged(
        &f.library,
        &f.root,
        "skills",
        ".claude/skills/local-skill",
        None,
    )
    .unwrap();
    assert!(!f.root.join(".claude/skills/local-skill").exists());
    // 路径不在任何 Agent skills 目录下 → 拒绝
    assert!(delete_unmanaged(&f.library, &f.root, "skills", "docs/guide", None).is_err());
    // mcp:仅从配置文件中移除指定条目,其余保留
    fs::write(
        f.root.join(".mcp.json"),
        json!({"mcpServers": {"orphan": {"command": "node"}, "keep": {"command": "node"}}})
            .to_string(),
    )
    .unwrap();
    delete_unmanaged(&f.library, &f.root, "mcp", ".mcp.json", Some("orphan")).unwrap();
    let text = fs::read_to_string(f.root.join(".mcp.json")).unwrap();
    assert!(!text.contains("orphan"));
    assert!(text.contains("keep"));
    // 已部署(托管)的路径/条目 → 拒绝
    f.skill("s1", "review", "v1");
    f.apply("skills", "claude", &["s1"]);
    assert!(
        delete_unmanaged(&f.library, &f.root, "skills", ".claude/skills/review", None).is_err()
    );
    f.mcp();
    f.apply("mcp", "claude", &["m1"]);
    assert!(delete_unmanaged(&f.library, &f.root, "mcp", ".mcp.json", Some("context")).is_err());
}
