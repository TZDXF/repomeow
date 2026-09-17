use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::params;
use serde_json::json;

use crate::commands::wiki::wiki_dir_in;
use crate::db::Db;
use crate::path_util::clean_str;

use super::ai_tool::*;
use super::file_tool::*;
use super::pin_tool::*;
use super::project_tool::*;
use super::report_tool::*;
use super::script_tool::*;
use super::tag_tool::*;
use super::util::*;
use super::wiki_tool::*;
use super::*;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!(
        "repomeow-cli-{tag}-{}-{}",
        std::process::id(),
        crate::time_util::now_ts_nanos(),
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}


#[test]
fn cli_head_detection_only_accepts_known_subcommands() {
    for head in [
        "wiki", "sem", "project", "report", "tag", "script", "pin", "hidden", "ai",
        "prompt", "account", "file",
    ] {
        assert!(is_cli_head(head), "should enter CLI mode: {head}");
    }
    for other in ["status", "commit_code", "--mcp", "--autostart", "repomeow"] {
        assert!(!is_cli_head(other), "should not enter CLI mode: {other}");
    }
}

#[test]
fn cli_parses_report_generate_defaults() {
    let cli = Cli::try_parse_from([
        "repomeow",
        "report",
        "generate",
        "-p",
        "D:/a,D:/b",
        "--period-type",
        "weekly",
    ])
    .unwrap();
    let Commands::Report(ReportCommands::Generate {
        project_directories,
        period_type,
        date_from,
        author_mode,
        ..
    }) = cli.command
    else {
        panic!("expected report generate");
    };
    assert_eq!(
        project_directories,
        vec!["D:/a".to_string(), "D:/b".to_string()]
    );
    assert_eq!(period_type, "weekly");
    assert!(date_from.is_none());
    assert!(author_mode.is_none());
}

#[test]
fn cli_rejects_unknown_subcommand() {
    assert!(Cli::try_parse_from(["repomeow", "sem", "frobnicate"]).is_err());
    assert!(Cli::try_parse_from(["repomeow", "unknown"]).is_err());
}
#[test]
fn wiki_directory_returns_completed_meta() {
    let data_root = temp_dir("wiki-completed");
    let project = temp_dir("wiki-project");
    let project_path = project.to_string_lossy().into_owned();
    let dir = wiki_dir_in(&data_root.join(WIKI_DIR_NAME), &project_path);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(WIKI_META_FILE),
        r#"{"status":"completed","version":1,"outline":[]}"#,
    )
    .unwrap();

    let output = get_wiki_directory_impl(
        GetWikiDirectoryInput {
            project_directory: project_path,
        },
        Some(&data_root),
    )
    .unwrap();
    assert_eq!(output.meta["status"], "completed");
    assert_eq!(PathBuf::from(output.meta_path), dir.join(WIKI_META_FILE));

    let _ = fs::remove_dir_all(data_root);
    let _ = fs::remove_dir_all(project);
}

#[test]
fn wiki_directory_rejects_missing_or_incomplete_meta() {
    let data_root = temp_dir("wiki-missing");
    let project_path = "D:/projects/missing".to_string();
    let missing = get_wiki_directory_impl(
        GetWikiDirectoryInput {
            project_directory: project_path.clone(),
        },
        Some(&data_root),
    )
    .unwrap_err();
    assert_eq!(missing.code, "wiki_not_generated");

    let dir = wiki_dir_in(&data_root.join(WIKI_DIR_NAME), &project_path);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(WIKI_META_FILE), r#"{"status":"generating"}"#).unwrap();
    let incomplete = get_wiki_directory_impl(
        GetWikiDirectoryInput {
            project_directory: project_path,
        },
        Some(&data_root),
    )
    .unwrap_err();
    assert_eq!(incomplete.code, "wiki_not_generated");

    let _ = fs::remove_dir_all(data_root);
}

fn seed_wiki(data_root: &Path, project_path: &str) {
    let dir = wiki_dir_in(&data_root.join(WIKI_DIR_NAME), project_path);
    fs::create_dir_all(dir.join("pages")).unwrap();
    fs::write(
        dir.join(WIKI_META_FILE),
        r#"{"status":"completed","version":1,"generatedAt":"2026-09-01 10:00","outline":[{"id":"overview","file":"01-overview.md","title":"总览","description":"项目总览","relevantFiles":["src/main.ts"]}]}"#,
    )
    .unwrap();
    fs::write(
        dir.join("pages").join("01-overview.md"),
        "# 总览\n\n这是内容。\n",
    )
    .unwrap();
}

#[test]
fn wiki_pages_list_and_read_page() {
    let data_root = temp_dir("wiki-pages");
    let project = temp_dir("wiki-pages-project");
    let project_path = project.to_string_lossy().into_owned();
    seed_wiki(&data_root, &project_path);

    let list = list_wiki_pages_impl(
        ProjectDirectoryInput {
            project_directory: project_path.clone(),
        },
        Some(&data_root),
    )
    .unwrap();
    assert_eq!(list.pages.len(), 1);
    assert_eq!(list.pages[0]["id"], json!("overview"));
    assert!(!list.stale);

    let page = read_wiki_page_impl(
        ReadWikiPageInput {
            project_directory: project_path.clone(),
            page_id: "overview".into(),
        },
        Some(&data_root),
    )
    .unwrap();
    assert!(page.content.contains("这是内容"));
    assert!(!page.truncated);

    let missing = read_wiki_page_impl(
        ReadWikiPageInput {
            project_directory: project_path,
            page_id: "nope".into(),
        },
        Some(&data_root),
    )
    .unwrap_err();
    assert_eq!(missing.code, "wiki_page_not_found");

    let _ = fs::remove_dir_all(data_root);
    let _ = fs::remove_dir_all(project);
}

#[test]
fn read_project_file_windows_lines_and_bounds() {
    let project = temp_dir("read-file");
    fs::write(project.join("a.txt"), "l1\nl2\nl3\nl4\nl5\n").unwrap();
    let root = project.to_string_lossy().into_owned();

    let page = read_project_file_impl(ReadProjectFileInput {
        project_directory: root.clone(),
        path: "a.txt".into(),
        offset_line: Some(2),
        max_lines: Some(2),
    })
    .unwrap();
    assert_eq!(page.start_line, 2);
    assert_eq!(page.end_line, 3);
    assert!(page.has_more);
    assert_eq!(page.content, "2: l2\n3: l3");

    let tail = read_project_file_impl(ReadProjectFileInput {
        project_directory: root.clone(),
        path: "a.txt".into(),
        offset_line: Some(4),
        max_lines: None,
    })
    .unwrap();
    assert!(!tail.has_more);
    assert_eq!(tail.end_line, 5);

    let out_of_range = read_project_file_impl(ReadProjectFileInput {
        project_directory: root.clone(),
        path: "a.txt".into(),
        offset_line: Some(99),
        max_lines: None,
    })
    .unwrap_err();
    assert_eq!(out_of_range.code, "offset_out_of_range");

    fs::write(project.join("bin.dat"), [0u8, 1, 2]).unwrap();
    let binary = read_project_file_impl(ReadProjectFileInput {
        project_directory: root.clone(),
        path: "bin.dat".into(),
        offset_line: None,
        max_lines: None,
    })
    .unwrap_err();
    assert_eq!(binary.code, "binary_file");

    let escape = read_project_file_impl(ReadProjectFileInput {
        project_directory: root,
        path: "../outside.txt".into(),
        offset_line: None,
        max_lines: None,
    })
    .unwrap_err();
    assert!(!escape.code.is_empty());

    let _ = fs::remove_dir_all(project);
}

fn seed_db(data_root: &Path) {
    let db = Db::open(&data_root.join(PROJECTS_DB_FILE)).unwrap();
    let conn = db.0.lock().unwrap();
    conn.execute(
        "INSERT INTO projects (path, name, created_at, updated_at) VALUES (?1, ?2, 0, 0)",
        params![clean_str("D:/projects/demo"), "demo"],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO custom_commands (project_id, name, command) VALUES (1, 'dev', 'pnpm dev')",
        [],
    )
    .unwrap();
}

#[test]
fn project_scoped_tools_resolve_registered_projects() {
    let data_root = temp_dir("mcp-db");
    seed_db(&data_root);

    let commands = list_custom_commands_impl(
        ProjectDirectoryInput {
            project_directory: "D:/projects/demo".into(),
        },
        Some(&data_root),
    )
    .unwrap();
    assert_eq!(commands["commands"][0]["name"], json!("dev"));

    let unknown = list_custom_commands_impl(
        ProjectDirectoryInput {
            project_directory: "D:/projects/ghost".into(),
        },
        Some(&data_root),
    )
    .unwrap_err();
    assert_eq!(unknown.code, "project_not_found");

    let reports = list_reports_impl(
        ListReportsInput {
            project_directory: None,
            limit: None,
        },
        Some(&data_root),
    )
    .unwrap();
    assert!(reports["reports"].as_array().unwrap().is_empty());

    let _ = fs::remove_dir_all(data_root);
}

#[tokio::test]
async fn generate_report_validates_before_touching_disk() {
    let bad_period = generate_report_impl(
        GenerateReportInput {
            project_directories: vec!["D:/projects/demo".into()],
            period_type: "monthly".into(),
            date_from: None,
            date_to: None,
            author_mode: None,
            language: None,
        },
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(bad_period.code, "invalid_period_type");

    let no_projects = generate_report_impl(
        GenerateReportInput {
            project_directories: Vec::new(),
            period_type: "daily".into(),
            date_from: None,
            date_to: None,
            author_mode: None,
            language: None,
        },
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(no_projects.code, "project_directories_required");
}

#[test]
fn entity_token_split_and_truncation() {
    assert_eq!(
        split_entity_token("src/a.ts::function::run"),
        Some((Some("src/a.ts::function::run".to_string()), None))
    );
    assert_eq!(
        split_entity_token("run"),
        Some((None, Some("run".to_string())))
    );
    assert_eq!(split_entity_token("  "), None);

    let (text, truncated) = truncate_text("hello", 10);
    assert!(!truncated);
    assert_eq!(text, "hello");
    let (text, truncated) = truncate_text("你好世界", 7);
    assert!(truncated);
    assert_eq!(text, "你好");
}

#[test]
fn cli_parse_failures_are_json() {
    let cases = [
        vec!["repomeow", "project", "add"],
        vec!["repomeow", "tag", "create"],
        vec![
            "repomeow", "sem", "context", "-d", "D:/repo", "-e", "run", "--budget", "invalid",
        ],
    ];
    for args in cases {
        let error = Cli::try_parse_from(args).unwrap_err();
        let expected_detail = error.to_string();
        let (exit_code, output) = format_parse_error(error);
        assert_eq!(exit_code, 2);
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["code"], "invalid_arguments");
        assert_eq!(value["message"], "CLI 参数无效");
        assert_eq!(value["detail"], expected_detail);
    }
}

#[test]
fn cli_help_and_version_remain_successful_text() {
    for args in [
        vec!["repomeow", "--help"],
        vec!["repomeow", "wiki", "--help"],
        vec!["repomeow", "--version"],
    ] {
        let error = Cli::try_parse_from(args).unwrap_err();
        let expected = error.to_string();
        let (exit_code, output) = format_parse_error(error);
        assert_eq!(exit_code, 0);
        assert_eq!(output, expected);
        assert!(!output.is_empty());
        assert!(serde_json::from_str::<serde_json::Value>(&output).is_err());
    }
}

// ── 新分组解析测试 ─────────────────────────────────────────────────────

#[test]
fn cli_parses_project_management_commands() {
    let cli = Cli::try_parse_from([
        "repomeow", "project", "add", "-p", "D:/repo", "--name", "demo",
    ])
    .unwrap();
    let Commands::Project(ProjectCommands::Add {
        project_directory,
        name,
        ..
    }) = cli.command
    else {
        panic!("expected project add");
    };
    assert_eq!(project_directory, "D:/repo");
    assert_eq!(name.as_deref(), Some("demo"));

    let cli = Cli::try_parse_from([
        "repomeow",
        "project",
        "set-auto-pull",
        "-p",
        "D:/repo",
        "--enabled",
        "true",
    ])
    .unwrap();
    let Commands::Project(ProjectCommands::SetAutoPull { enabled, .. }) = cli.command else {
        panic!("expected project set-auto-pull");
    };
    assert!(enabled);
}

#[test]
fn cli_parses_tag_script_pin_hidden_commands() {
    let cli = Cli::try_parse_from(["repomeow", "tag", "create", "--name", "后端"]).unwrap();
    assert!(matches!(
        cli.command,
        Commands::Tag(TagCommands::Create { .. })
    ));

    let cli = Cli::try_parse_from([
        "repomeow",
        "tag",
        "set-project",
        "-p",
        "D:/repo",
        "--tag-ids",
        "1,2",
    ])
    .unwrap();
    let Commands::Tag(TagCommands::SetProject { tag_ids, .. }) = cli.command else {
        panic!("expected tag set-project");
    };
    assert_eq!(tag_ids, vec![1, 2]);

    let cli = Cli::try_parse_from([
        "repomeow",
        "script",
        "create",
        "-p",
        "D:/repo",
        "--name",
        "dev",
        "--command",
        "pnpm dev",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Commands::Script(ScriptCommands::Create { .. })
    ));

    let cli = Cli::try_parse_from([
        "repomeow",
        "pin",
        "set",
        "-p",
        "D:/repo",
        "--kind",
        "customCommand",
        "--target-key",
        "1",
        "--pinned",
        "true",
    ])
    .unwrap();
    let Commands::Pin(PinCommands::Set { pinned, .. }) = cli.command else {
        panic!("expected pin set");
    };
    assert!(pinned);

    let cli = Cli::try_parse_from([
        "repomeow",
        "hidden",
        "set",
        "-p",
        "D:/repo",
        "--kind",
        "packageScript",
        "--target-key",
        "build",
        "--hidden",
        "false",
    ])
    .unwrap();
    let Commands::Hidden(HiddenCommands::Set { hidden, .. }) = cli.command else {
        panic!("expected hidden set");
    };
    assert!(!hidden);
}

#[test]
fn cli_parses_ai_account_file_commands() {
    let cli = Cli::try_parse_from([
        "repomeow",
        "ai",
        "usage-log",
        "--offset",
        "0",
        "--limit",
        "20",
        "--task-type",
        "report",
    ])
    .unwrap();
    let Commands::Ai(AiCommands::UsageLog { limit, .. }) = cli.command else {
        panic!("expected ai usage-log");
    };
    assert_eq!(limit, Some(20));

    let cli = Cli::try_parse_from([
        "repomeow",
        "account",
        "add",
        "--provider",
        "github",
        "--label",
        "work",
        "--token",
        "tok",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Commands::Account(AccountCommands::Add { .. })
    ));

    let cli = Cli::try_parse_from([
        "repomeow",
        "file",
        "save",
        "-p",
        "D:/repo",
        "--path",
        "docs/a.md",
        "--content",
        "hi",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Commands::File(FileCommands::Save { .. })
    ));


}

// ── 新分组实现测试(临时数据目录)──────────────────────────────────────

/// 项目登记管理全生命周期:add → list/get → 开关 → archive/unarchive → delete。
#[test]
fn project_management_lifecycle() {
    let data_root = temp_dir("project-mgmt");
    let project = temp_dir("project-mgmt-repo");
    let dir = project.to_string_lossy().into_owned();

    let added = add_project_impl(&dir, None, None, Some(&data_root)).unwrap();
    let id = added["id"].as_i64().unwrap();
    assert!(id > 0);
    // 缺省名称取目录 basename
    assert_eq!(
        added["name"],
        json!(project.file_name().unwrap().to_string_lossy())
    );

    let listed = list_projects_impl(false, Some(&data_root)).unwrap();
    assert_eq!(listed["projects"].as_array().unwrap().len(), 1);

    let got = get_project_impl(
        ProjectDirectoryInput {
            project_directory: dir.clone(),
        },
        Some(&data_root),
    )
    .unwrap();
    assert_eq!(got["id"], json!(id));

    let renamed =
        update_project_impl(&dir, Some("new-name".into()), None, Some(&data_root)).unwrap();
    assert_eq!(renamed["name"], json!("new-name"));

    let flag = set_project_flag_impl(&dir, "autoPull", true, Some(&data_root)).unwrap();
    assert_eq!(flag["enabled"], json!(true));

    // 归档后不再出现在未归档列表;get 仍可查到(含已归档)
    set_project_archived_impl(&dir, true, Some(&data_root)).unwrap();
    assert!(
        list_projects_impl(false, Some(&data_root)).unwrap()["projects"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        list_projects_impl(true, Some(&data_root)).unwrap()["projects"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    set_project_archived_impl(&dir, false, Some(&data_root)).unwrap();
    delete_project_impl(&dir, Some(&data_root)).unwrap();
    let missing = get_project_impl(
        ProjectDirectoryInput {
            project_directory: dir,
        },
        Some(&data_root),
    )
    .unwrap_err();
    assert_eq!(missing.code, "project_not_found");

    let _ = fs::remove_dir_all(data_root);
    let _ = fs::remove_dir_all(project);
}

/// 标签 + 项目绑定 + 自定义命令生命周期。
#[test]
fn tag_and_script_lifecycle() {
    let data_root = temp_dir("tag-script");
    let project = temp_dir("tag-script-repo");
    let dir = project.to_string_lossy().into_owned();
    add_project_impl(&dir, None, None, Some(&data_root)).unwrap();

    let tag = create_tag_impl("后端", Some("#ff0000"), Some(&data_root)).unwrap();
    let tag_id = tag["id"].as_i64().unwrap();
    assert_eq!(tag["name"], json!("后端"));

    let bound = set_project_tags_impl(&dir, vec![tag_id], Some(&data_root)).unwrap();
    assert_eq!(bound["tagIds"], json!([tag_id]));
    let got = get_project_impl(
        ProjectDirectoryInput {
            project_directory: dir.clone(),
        },
        Some(&data_root),
    )
    .unwrap();
    assert_eq!(got["tags"][0]["name"], json!("后端"));

    let cmd = create_command_impl(&dir, "dev", "pnpm dev", None, None, Some(&data_root)).unwrap();
    let cmd_id = cmd["id"].as_i64().unwrap();
    let updated = update_command_impl(
        cmd_id,
        "dev2",
        "pnpm dev2",
        Some("开发"),
        None,
        Some(&data_root),
    )
    .unwrap();
    assert_eq!(updated["name"], json!("dev2"));

    // pin 标记该命令,删除命令后 pin 同步移除
    set_pin_impl(
        &dir,
        "customCommand",
        &cmd_id.to_string(),
        true,
        Some("dev2"),
        Some("pnpm dev2"),
        None,
        Some(&data_root),
    )
    .unwrap();
    let pins = list_pins_impl(Some(&dir), Some(&data_root)).unwrap();
    assert_eq!(pins["pins"].as_array().unwrap().len(), 1);

    delete_command_impl(cmd_id, Some(&data_root)).unwrap();
    let pins = list_pins_impl(Some(&dir), Some(&data_root)).unwrap();
    assert!(pins["pins"].as_array().unwrap().is_empty());

    let hidden = set_hidden_impl(&dir, "packageScript", "build", true, Some(&data_root)).unwrap();
    assert_eq!(hidden["hidden"], json!(true));
    let listed = list_hidden_impl(&dir, Some(&data_root)).unwrap();
    assert_eq!(listed["hidden"].as_array().unwrap().len(), 1);

    delete_tag_impl(tag_id, Some(&data_root)).unwrap();
    assert!(list_tags_impl(Some(&data_root)).unwrap()["tags"]
        .as_array()
        .unwrap()
        .is_empty());

    let _ = fs::remove_dir_all(data_root);
    let _ = fs::remove_dir_all(project);
}

/// Wiki 配置读写与删除(临时数据目录,不动真实 wiki)。
#[test]
fn wiki_config_and_delete_lifecycle() {
    let data_root = temp_dir("wiki-mgmt");
    let project_path = "D:/projects/wiki-demo".to_string();

    let got = wiki_config_get_impl(&project_path, Some(&data_root)).unwrap();
    assert_eq!(got["exists"], json!(false));

    let set = wiki_config_set_impl(
        &project_path,
        Some("aether/step-3.7-flash".into()),
        Some("high".into()),
        Some(4),
        Some(&data_root),
    )
    .unwrap();
    assert_eq!(set["config"]["model"], json!("aether/step-3.7-flash"));
    assert_eq!(set["config"]["concurrency"], json!(4));

    let got = wiki_config_get_impl(&project_path, Some(&data_root)).unwrap();
    assert_eq!(got["exists"], json!(true));
    assert_eq!(got["config"]["thinking"], json!("high"));

    let deleted = delete_wiki_impl(&project_path, Some(&data_root)).unwrap();
    assert_eq!(deleted["deleted"], json!(true));

    let _ = fs::remove_dir_all(data_root);
}

/// 提示词读写往返(临时数据目录)。
#[test]
fn prompts_roundtrip_via_files() {
    let data_root = temp_dir("prompts");
    let prompt_file = temp_dir("prompt-src").join("commit.md");
    fs::write(&prompt_file, "自定义提交提示词").unwrap();

    let set = prompts_set_impl(
        Some(prompt_file.to_string_lossy().as_ref()),
        None,
        None,
        false,
        Some(&data_root),
    )
    .unwrap();
    assert_eq!(set["commit"], json!("自定义提交提示词"));

    let got = prompts_get_impl(Some(&data_root)).unwrap();
    assert_eq!(got["commit"], json!("自定义提交提示词"));
    assert_eq!(got["report"], json!(""));

    // --clear 恢复未指定项默认(清空 commit)
    let cleared = prompts_set_impl(None, None, None, true, Some(&data_root)).unwrap();
    assert_eq!(cleared["commit"], json!(""));

    let default = prompts_default_impl().unwrap();
    assert!(default["commit"].as_str().unwrap().len() > 10);

    let _ = fs::remove_dir_all(data_root);
}

/// 系统调度读写(临时数据目录)。
#[test]
fn system_schedule_roundtrip() {
    let data_root = temp_dir("syssched");
    seed_db(&data_root);

    let saved = save_system_schedule_impl(false, 30, Some(&data_root)).unwrap();
    assert_eq!(saved["enabled"], json!(false));
    assert_eq!(saved["intervalMinutes"], json!(30));

    let listed = list_system_schedules_impl(Some(&data_root)).unwrap();
    assert_eq!(listed["schedules"][0]["enabled"], json!(false));

    let _ = fs::remove_dir_all(data_root);
}

/// 文件写入:拒绝越界路径与双内容源。
#[test]
fn save_file_rejects_traversal_and_dual_source() {
    let project = temp_dir("file-save");

    let traversal = save_file_impl(&project.to_string_lossy(), "../escape.txt", Some("x"), None);
    assert!(traversal.is_err());
    assert_eq!(traversal.unwrap_err().code, "invalid_file_path");

    let dual = save_file_impl(
        &project.to_string_lossy(),
        "a.txt",
        Some("x"),
        Some("other.txt"),
    );
    assert!(dual.is_err());

    let ok = save_file_impl(&project.to_string_lossy(), "a.txt", Some("内容"), None).unwrap();
    assert_eq!(ok["saved"], json!(true));
    assert_eq!(fs::read_to_string(project.join("a.txt")).unwrap(), "内容");

    let _ = fs::remove_dir_all(project);
}
