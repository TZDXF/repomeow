use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::params;
use serde_json::json;

use crate::commands::git::run_git;
use crate::commands::wiki::wiki_dir_in;
use crate::db::Db;
use crate::path_util::clean_str;

use super::git_tool::*;
use super::project_tool::*;
use super::report_tool::*;
use super::util::*;
use super::wiki_tool::*;
use super::*;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!(
        "repomeow-mcp-{tag}-{}-{}",
        std::process::id(),
        crate::time_util::now_ts_nanos(),
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn git(root: &Path, args: &[&str]) {
    let root = root.to_string_lossy();
    run_git(&root, args).unwrap();
}

#[test]
fn commit_paths_only_accept_repository_relative_paths() {
    assert!(normalize_commit_paths(Some(vec!["src/main.rs".into()])).is_ok());
    assert!(normalize_commit_paths(Some(vec!["../secret".into()])).is_err());
    assert!(normalize_commit_paths(Some(vec!["C:/secret".into()])).is_err());
    assert!(normalize_commit_paths(Some(Vec::new())).is_err());
}

#[test]
fn tool_groups_are_opt_in_and_filter_visible_routes() {
    const ALL_ROUTES: &[&str] = &[
        "commit_code",
        "get_git_status",
        "get_wiki_directory",
        "list_wiki_pages",
        "read_wiki_page",
        "sem_find",
        "sem_context",
        "sem_relations",
        "sem_diff",
        "read_project_file",
        "list_reports",
        "list_custom_commands",
        "generate_report",
    ];
    let disabled = RepoMeowMcpServer::new(McpToolGroups::default());
    for route in ALL_ROUTES {
        assert!(
            !disabled.tool_router.has_route(route),
            "route should be off by default: {route}"
        );
    }

    let enabled = RepoMeowMcpServer::new(McpToolGroups {
        git_commit: true,
        wiki: true,
        sem: true,
        project: true,
        report: true,
    });
    for route in ALL_ROUTES {
        assert!(
            enabled.tool_router.has_route(route),
            "route should be on: {route}"
        );
    }

    // 单组开关互不影响
    let wiki_only = RepoMeowMcpServer::new(McpToolGroups {
        wiki: true,
        ..McpToolGroups::default()
    });
    assert!(wiki_only.tool_router.has_route("list_wiki_pages"));
    assert!(!wiki_only.tool_router.has_route("sem_find"));
    assert!(!wiki_only.tool_router.has_route("generate_report"));
}

#[test]
fn tool_group_settings_accept_store_strings_and_json_booleans() {
    let settings = json!({
        "mcpGitCommitEnabled": "true",
        "mcpWikiEnabled": true,
        "mcpSemEnabled": "true",
        "mcpProjectEnabled": true,
        "mcpReportEnabled": "true",
    });
    assert!(setting_bool(&settings, GIT_COMMIT_ENABLED_KEY));
    assert!(setting_bool(&settings, WIKI_ENABLED_KEY));
    assert!(setting_bool(&settings, SEM_ENABLED_KEY));
    assert!(setting_bool(&settings, PROJECT_ENABLED_KEY));
    assert!(setting_bool(&settings, REPORT_ENABLED_KEY));
    assert!(!setting_bool(&settings, "missing"));
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

#[test]
fn commit_code_can_commit_selected_files() {
    let root = temp_dir("commit-selected");
    git(&root, &["init", "-b", "main"]);
    git(&root, &["config", "user.email", "mcp@example.com"]);
    git(&root, &["config", "user.name", "RepoMeow MCP"]);
    fs::write(root.join("a.txt"), "a\n").unwrap();
    fs::write(root.join("b.txt"), "b\n").unwrap();

    let output = commit_code_impl(CommitCodeInput {
        directory: root.to_string_lossy().into_owned(),
        message: "test: 仅提交 a".into(),
        files: Some(vec!["a.txt".into()]),
    })
    .unwrap();

    assert_eq!(output.branch.as_deref(), Some("main"));
    assert_eq!(output.committed_files, vec!["a.txt"]);
    let status = git_output(&output.directory, &["status", "--porcelain"], "status").unwrap();
    assert!(status.contains("?? b.txt"));

    let _ = fs::remove_dir_all(root);
}

fn seed_wiki(data_root: &Path, project_path: &str) {
    let dir = wiki_dir_in(&data_root.join(WIKI_DIR_NAME), project_path);
    fs::create_dir_all(dir.join("pages")).unwrap();
    fs::write(
        dir.join(WIKI_META_FILE),
        r#"{"status":"completed","version":1,"generatedAt":"2026-09-01 10:00","outline":[{"id":"overview","file":"01-overview.md","title":"总览","description":"项目总览","relevantFiles":["src/main.ts"]}]}"#,
    )
    .unwrap();
    fs::write(dir.join("pages").join("01-overview.md"), "# 总览\n\n这是内容。\n").unwrap();
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
