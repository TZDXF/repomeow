use std::path::Path;

use serde_json::{json, Value};

use crate::commands::wiki::{
    has_wiki_in, load_config_in, load_wiki_at, remove_wiki_dir, save_config_in, wiki_dir_in,
    WikiGenerationConfig,
};

use super::types::{
    GetWikiDirectoryInput, ProjectDirectoryInput, ReadWikiPageInput, WikiDirectoryOutput,
    WikiPageOutput, WikiPagesOutput,
};
use super::util::{
    data_root_or_default, repomeow_data_root, truncate_text, ToolFailure, WIKI_DIR_NAME,
    WIKI_META_FILE,
};
use crate::path_util::clean_str;
use std::fs;

/// read_wiki_page 单页正文字节上限(对齐 chat 工具)。
const WIKI_PAGE_MAX_BYTES: usize = 24 * 1024;

pub(super) fn get_wiki_directory_impl(
    input: GetWikiDirectoryInput,
    data_root: Option<&Path>,
) -> Result<WikiDirectoryOutput, ToolFailure> {
    let project_directory = clean_str(&input.project_directory);
    if project_directory.trim().is_empty() {
        return Err(ToolFailure::new(
            "invalid_project_directory",
            "项目目录不能为空",
        ));
    }

    let data_root = match data_root {
        Some(root) => root.to_path_buf(),
        None => repomeow_data_root()?,
    };
    let wiki_directory = wiki_dir_in(&data_root.join(WIKI_DIR_NAME), &project_directory);
    let meta_path = wiki_directory.join(WIKI_META_FILE);
    let raw = fs::read_to_string(&meta_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ToolFailure::new("wiki_not_generated", "该项目尚未生成 Wiki")
                .with_detail(meta_path.to_string_lossy())
        } else {
            ToolFailure::new("wiki_meta_read_failed", "读取 Wiki meta.json 失败")
                .with_detail(error.to_string())
        }
    })?;
    let meta: Value = serde_json::from_str(&raw).map_err(|error| {
        ToolFailure::new("wiki_meta_invalid", "Wiki meta.json 格式无效")
            .with_detail(error.to_string())
    })?;
    if meta.get("status").and_then(Value::as_str) != Some("completed") {
        return Err(
            ToolFailure::new("wiki_not_generated", "该项目的 Wiki 尚未生成完成")
                .with_detail(meta_path.to_string_lossy()),
        );
    }

    Ok(WikiDirectoryOutput {
        project_directory,
        wiki_directory: wiki_directory.to_string_lossy().into_owned(),
        meta_path: meta_path.to_string_lossy().into_owned(),
        meta,
    })
}

// ── Wiki 查询 ─────────────────────────────────────────────────────────

pub(super) fn load_project_wiki(
    project_directory: &str,
    data_root: Option<&Path>,
) -> Result<(String, crate::commands::wiki::WikiData), ToolFailure> {
    let project_directory = clean_str(project_directory);
    if project_directory.trim().is_empty() {
        return Err(ToolFailure::new(
            "invalid_project_directory",
            "项目目录不能为空",
        ));
    }
    let data_root = data_root_or_default(data_root)?;
    let data = load_wiki_at(&data_root, &project_directory).ok_or_else(|| {
        ToolFailure::new(
            "wiki_not_generated",
            "该项目尚未生成 Wiki(或 Wiki 未生成完成)",
        )
    })?;
    Ok((project_directory, data))
}

pub(super) fn list_wiki_pages_impl(
    input: ProjectDirectoryInput,
    data_root: Option<&Path>,
) -> Result<WikiPagesOutput, ToolFailure> {
    let (project_directory, data) = load_project_wiki(&input.project_directory, data_root)?;
    let pages = data
        .meta
        .outline
        .iter()
        .map(|page| {
            json!({
                "id": page.id,
                "title": page.title,
                "description": page.description,
                "section": page.section,
                "relevantFiles": page.relevant_files,
            })
        })
        .collect();
    Ok(WikiPagesOutput {
        project_directory,
        stale: data.stale,
        generated_at: data.meta.generated_at,
        head_sha: data.meta.head_sha,
        generator: data.meta.generator,
        model: data.meta.model,
        pages,
    })
}

pub(super) fn read_wiki_page_impl(
    input: ReadWikiPageInput,
    data_root: Option<&Path>,
) -> Result<WikiPageOutput, ToolFailure> {
    let (_directory, data) = load_project_wiki(&input.project_directory, data_root)?;
    let page_id = input.page_id.trim();
    let Some(page) = data.pages.iter().find(|page| page.id == page_id) else {
        return Err(ToolFailure::new(
            "wiki_page_not_found",
            format!("未找到页面 id「{page_id}」,可用 wiki pages 查看页面清单"),
        ));
    };
    let (content, truncated) = truncate_text(&page.content, WIKI_PAGE_MAX_BYTES);
    Ok(WikiPageOutput {
        id: page.id.clone(),
        title: page.title.clone(),
        file: page.file.clone(),
        stale: data.stale,
        content,
        truncated,
    })
}

// ── Wiki 管理(配置与删除)───────────────────────────────────────────────

fn wiki_dir_for(
    project_directory: &str,
    data_root: Option<&Path>,
) -> Result<std::path::PathBuf, ToolFailure> {
    let project_directory = clean_str(project_directory);
    if project_directory.trim().is_empty() {
        return Err(ToolFailure::new(
            "invalid_project_directory",
            "项目目录不能为空",
        ));
    }
    let data_root = data_root_or_default(data_root)?;
    Ok(wiki_dir_in(
        &data_root.join(WIKI_DIR_NAME),
        &project_directory,
    ))
}

/// 读取项目 Wiki 生成配置;从未配置过时返回默认配置(exists=false)。
pub(super) fn wiki_config_get_impl(
    project_directory: &str,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let dir = wiki_dir_for(project_directory, data_root)?;
    let exists = dir.join("config.json").is_file();
    let config =
        load_config_in(&dir).map_err(|error| ToolFailure::from_app("读取 Wiki 配置失败", error))?;
    Ok(json!({
        "projectDirectory": clean_str(project_directory),
        "wikiDirectory": dir.to_string_lossy(),
        "exists": exists,
        "config": config,
    }))
}

/// 覆盖式写入项目 Wiki 生成配置(所有字段可选,缺省即跟随应用默认)。
pub(super) fn wiki_config_set_impl(
    project_directory: &str,
    model: Option<String>,
    thinking: Option<String>,
    concurrency: Option<usize>,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let dir = wiki_dir_for(project_directory, data_root)?;
    let config = WikiGenerationConfig {
        model: model
            .map(|value| value.trim().to_string())
            .filter(|v| !v.is_empty()),
        thinking: thinking
            .map(|value| value.trim().to_string())
            .filter(|v| !v.is_empty()),
        concurrency: concurrency.filter(|value| *value > 0),
        ..Default::default()
    };
    save_config_in(&dir, config.clone())
        .map_err(|error| ToolFailure::from_app("保存 Wiki 配置失败", error))?;
    Ok(json!({
        "projectDirectory": clean_str(project_directory),
        "wikiDirectory": dir.to_string_lossy(),
        "config": config,
    }))
}

/// 删除项目已生成的 Wiki(整个 wiki 目录,含其 git 历史);不删除项目登记本身。
pub(super) fn delete_wiki_impl(
    project_directory: &str,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let dir = wiki_dir_for(project_directory, data_root)?;
    let existed = has_wiki_in(&dir);
    remove_wiki_dir(&dir).map_err(|error| ToolFailure::from_app("删除 Wiki 失败", error))?;
    Ok(json!({
        "projectDirectory": clean_str(project_directory),
        "wikiDirectory": dir.to_string_lossy(),
        "deleted": existed,
    }))
}
