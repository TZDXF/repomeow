//! 内置 Agent runner 集成测试:脚本化 LLM 流 + 真实受限环境与暂存事务,
//! 覆盖直接写入、预览、越权写拒绝、校验修复与大纲纠错重试。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::builtin_backend::{generate_outline_with, generate_page_with};
use super::WikiRetryNotice;
use crate::agent::agent_loop::testing::{
    scripted_stream_fn, test_model, test_tool_call, text_script, tool_call_script, Script,
};
use crate::agent::llm::types::ModelThinkingLevel;
use crate::commands::wiki::WikiContext;
use crate::commands::wiki::WikiOutlinePage;
use crate::db::Db;
use tokio_util::sync::CancellationToken;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "repomeow-builtin-wiki-{tag}-{}-{}",
        std::process::id(),
        crate::time_util::now_ts_nanos()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn seed_project(root: &Path) {
    write_file(
        &root.join("README.md"),
        "# Demo\n\nA demo project for wiki tests.\n",
    );
    write_file(&root.join("src/main.rs"), "fn main() {}\n");
    write_file(&root.join("src/lib.rs"), "pub fn help() {}\n");
}

fn sample_page() -> WikiOutlinePage {
    WikiOutlinePage {
        id: "01-overview".to_string(),
        file: "01-overview.md".to_string(),
        title: "Overview".to_string(),
        description: "High level architecture".to_string(),
        section: None,
        importance: "high".to_string(),
        relevant_files: vec![
            "README.md".to_string(),
            "src/main.rs".to_string(),
            "src/lib.rs".to_string(),
        ],
        related_pages: Vec::new(),
    }
}

/// 合法页面:精确 H1 + 3 条 sources + 块后无内容
fn page_markdown(title: &str) -> String {
    format!(
        "# {title}\n\nBody text for {title}.\n\n<!-- sources\nREADME.md:1\nsrc/main.rs:1\nsrc/lib.rs:1\n-->\n"
    )
}

/// 非法页面:只有 2 条 sources(校验要求 3-10)
fn invalid_page_markdown(title: &str) -> String {
    format!(
        "# {title}\n\nBody.\n\n<!-- sources\nREADME.md:1\nsrc/main.rs:1\n-->\n"
    )
}

/// 暂存路径与 storage.rs 的 `staging_path_in` 派生规则一致(pages/.staging_{run}_{file})
fn staging_path(wiki_dir: &Path, run_id: &str, file: &str) -> PathBuf {
    wiki_dir.join("pages").join(format!(".staging_{run_id}_{file}"))
}

fn write_script(draft: &Path, content: &str) -> Script {
    tool_call_script(
        vec![test_tool_call(
            "w1",
            "write",
            serde_json::json!({ "path": draft.to_string_lossy(), "content": content }),
        )],
        "",
    )
}

fn noop_text() -> Arc<dyn Fn(String) + Send + Sync> {
    Arc::new(|_| {})
}

fn noop_retry() -> Arc<dyn Fn(WikiRetryNotice) + Send + Sync> {
    Arc::new(|_| {})
}

fn test_db(dir: &Path) -> Db {
    Db::open(&dir.join("projects.db")).expect("open test db")
}

fn usage_rows(db: &Db) -> i64 {
    db.0.lock().unwrap()
        .query_row("SELECT COUNT(*) FROM ai_usage_log", [], |r| r.get(0))
        .unwrap()
}

fn last_user_text(call: &crate::agent::agent_loop::testing::CapturedCall) -> String {
    match call.context.messages.last() {
        Some(crate::agent::llm::types::Message::User(user)) => user.content.to_plain_text(),
        _ => String::new(),
    }
}

#[tokio::test]
async fn builtin_page_agent_writes_draft_and_promotes() {
    let project = temp_dir("page-ok-proj");
    seed_project(&project);
    let wiki_dir = temp_dir("page-ok-wiki");
    let db = test_db(&wiki_dir);
    let page = sample_page();
    let draft = staging_path(&wiki_dir, "run-1", &page.file);
    let markdown = page_markdown("Overview");
    let (stream_fn, calls) = scripted_stream_fn(vec![
        write_script(&draft, &markdown),
        text_script("done"),
    ]);

    let progress_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let progress_sink = progress_log.clone();
    let model = generate_page_with(
        &db,
        test_model(),
        stream_fn,
        ModelThinkingLevel::Off,
        "run-1",
        &project.to_string_lossy(),
        &wiki_dir,
        &page,
        "en-US",
        &[],
        &CancellationToken::new(),
        Arc::new(move |content| progress_sink.lock().unwrap().push(content)),
        noop_text(),
        noop_retry(),
    )
    .await
    .expect("page generation should succeed");

    assert_eq!(model, "test-model");
    // 正式页被 Agent 直接写入的内容替换,暂存文件已提升(消失)
    let official = wiki_dir.join("pages").join(&page.file);
    assert_eq!(fs::read_to_string(&official).unwrap(), markdown);
    assert!(!draft.exists(), "staging file should be promoted away");
    // 流式预览来自 write 参数内容
    assert!(
        progress_log.lock().unwrap().iter().any(|p| p.contains("# Overview")),
        "preview should receive write content"
    );
    // 逐请求用量已落库(2 次 LLM 调用)
    assert_eq!(usage_rows(&db), 2);
    // prompt 注入:唯一可写草稿路径 + 行号前缀源文件 + 空草稿说明
    let captured = calls.lock().unwrap();
    assert_eq!(captured.len(), 2);
    let first = last_user_text(&captured[0]);
    assert!(first.contains(draft.to_string_lossy().as_ref()), "{first}");
    assert!(first.contains("The writable draft is empty"), "{first}");
    assert!(first.contains("1: fn main() {}"), "{first}");
}

#[tokio::test]
async fn builtin_page_invalid_draft_gets_one_repair_round() {
    let project = temp_dir("page-repair-proj");
    seed_project(&project);
    let wiki_dir = temp_dir("page-repair-wiki");
    let db = test_db(&wiki_dir);
    let page = sample_page();
    let draft = staging_path(&wiki_dir, "run-2", &page.file);
    let bad = invalid_page_markdown("Overview");
    let good = page_markdown("Overview");
    let (stream_fn, calls) = scripted_stream_fn(vec![
        write_script(&draft, &bad),
        text_script("done"),
        write_script(&draft, &good),
        text_script("fixed"),
    ]);

    let result = generate_page_with(
        &db,
        test_model(),
        stream_fn,
        ModelThinkingLevel::Off,
        "run-2",
        &project.to_string_lossy(),
        &wiki_dir,
        &page,
        "en-US",
        &[],
        &CancellationToken::new(),
        noop_text(),
        noop_text(),
        noop_retry(),
    )
    .await;

    assert!(result.is_ok(), "{result:?}");
    let official = wiki_dir.join("pages").join(&page.file);
    assert_eq!(fs::read_to_string(&official).unwrap(), good);
    // 第二轮 prompt 携带具体校验错误
    let captured = calls.lock().unwrap();
    assert_eq!(captured.len(), 4);
    let repair_prompt = last_user_text(&captured[2]);
    assert!(repair_prompt.contains("failed validation"), "{repair_prompt}");
    assert!(repair_prompt.contains("sources"), "{repair_prompt}");
}

#[tokio::test]
async fn builtin_page_write_outside_draft_is_rejected_then_recovers() {
    let project = temp_dir("page-guard-proj");
    seed_project(&project);
    let wiki_dir = temp_dir("page-guard-wiki");
    let db = test_db(&wiki_dir);
    let page = sample_page();
    let draft = staging_path(&wiki_dir, "run-3", &page.file);
    let source = project.join("src/main.rs");
    let good = page_markdown("Overview");
    let (stream_fn, _calls) = scripted_stream_fn(vec![
        // 越权写项目源码:受限环境拒绝,agent 换目标草稿
        write_script(&source, "tampered"),
        write_script(&draft, &good),
        text_script("done"),
    ]);

    let result = generate_page_with(
        &db,
        test_model(),
        stream_fn,
        ModelThinkingLevel::Off,
        "run-3",
        &project.to_string_lossy(),
        &wiki_dir,
        &page,
        "en-US",
        &[],
        &CancellationToken::new(),
        noop_text(),
        noop_text(),
        noop_retry(),
    )
    .await;

    assert!(result.is_ok(), "{result:?}");
    assert_eq!(fs::read_to_string(&source).unwrap(), "fn main() {}\n");
    let official = wiki_dir.join("pages").join(&page.file);
    assert_eq!(fs::read_to_string(&official).unwrap(), good);
}

#[tokio::test]
async fn builtin_page_failure_cleans_staging_and_keeps_old_page() {
    let project = temp_dir("page-fail-proj");
    seed_project(&project);
    let wiki_dir = temp_dir("page-fail-wiki");
    let db = test_db(&wiki_dir);
    let page = sample_page();
    let official = wiki_dir.join("pages").join(&page.file);
    let existing = page_markdown("Overview");
    write_file(&official, &existing);
    let draft = staging_path(&wiki_dir, "run-4", &page.file);
    let bad = invalid_page_markdown("Overview");
    let (stream_fn, _calls) = scripted_stream_fn(vec![
        // 更新场景:暂存从正式页复制旧内容;两次写入都非法 → 修复轮后仍失败
        write_script(&draft, &bad),
        text_script("done"),
        write_script(&draft, &bad),
        text_script("still bad"),
    ]);

    let result = generate_page_with(
        &db,
        test_model(),
        stream_fn,
        ModelThinkingLevel::Off,
        "run-4",
        &project.to_string_lossy(),
        &wiki_dir,
        &page,
        "en-US",
        &[],
        &CancellationToken::new(),
        noop_text(),
        noop_text(),
        noop_retry(),
    )
    .await;

    assert!(result.is_err());
    assert_eq!(fs::read_to_string(&official).unwrap(), existing);
    assert!(!draft.exists(), "failed staging should be cleaned up");
}

#[tokio::test]
async fn builtin_outline_retries_once_with_correction_prompt() {
    let project = temp_dir("outline-proj");
    fs::create_dir_all(&project).unwrap();
    let db = test_db(&project);
    let context = WikiContext {
        file_tree: "src/\n  main.rs\n".to_string(),
        paths: vec!["a.rs".to_string(), "b.rs".to_string(), "c.rs".to_string()],
        file_count: 3,
        tree_truncated: false,
        readme: None,
        manifests: vec![],
        head_sha: None,
    };
    let outline = valid_outline_json();
    let (stream_fn, calls) = scripted_stream_fn(vec![
        text_script("this is not json"),
        text_script(&outline),
    ]);

    let retry_log: Arc<Mutex<Vec<WikiRetryNotice>>> = Arc::new(Mutex::new(Vec::new()));
    let retry_sink = retry_log.clone();
    let (pages, model) = generate_outline_with(
        &db,
        test_model(),
        stream_fn,
        ModelThinkingLevel::Off,
        &context,
        &project.to_string_lossy(),
        "Demo",
        "en-US",
        &CancellationToken::new(),
        noop_text(),
        Arc::new(move |notice| retry_sink.lock().unwrap().push(notice)),
    )
    .await
    .expect("outline should parse on retry");

    assert_eq!(model, "test-model");
    assert_eq!(pages.len(), 6);
    assert_eq!(retry_log.lock().unwrap().len(), 1);
    let captured = calls.lock().unwrap();
    assert_eq!(captured.len(), 2);
    let correction = last_user_text(&captured[1]);
    assert!(correction.contains("Correction required"), "{correction}");
}

/// 最小合法大纲:6 页 × 3 个已存在文件,单 section 全覆盖
fn valid_outline_json() -> String {
    let files = ["a.rs", "b.rs", "c.rs"];
    let pages: Vec<serde_json::Value> = (1..=6)
        .map(|index| {
            serde_json::json!({
                "id": format!("page_{index}").replace('_', "-"),
                "title": format!("Page {index}"),
                "description": format!("Coverage for page {index}"),
                "importance": if index == 1 { "high" } else { "medium" },
                "relevantFiles": files,
                "relatedPages": [],
            })
        })
        .collect();
    serde_json::json!({
        "title": "Demo Wiki",
        "description": "Generated outline for tests",
        "sections": [{
            "id": "section-main",
            "title": "Main",
            "pages": ["page-1", "page-2", "page-3", "page-4", "page-5", "page-6"],
        }],
        "pages": pages,
    })
    .to_string()
}
