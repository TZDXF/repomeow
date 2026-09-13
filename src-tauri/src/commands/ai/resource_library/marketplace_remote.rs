//! 固定 GitHub 来源与 skills.sh 公开审计。远程 HTML 只输出纯文本，不执行脚本。
use std::io::{Cursor, Read};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use scraper::{Html, Selector};
use serde::Serialize;
use serde_json::Value;

use super::errors::{codes, RlError, RlResult};
use super::marketplace::{
    client, fetch_text, latest_commit_sha, parse_marketplace_id, read_limited, validate_part,
};
use super::models::{
    MarketplaceDownload, MarketplaceFile, MarketplaceList, MarketplaceSkill, MarketplaceSource,
};
use super::store::is_safe_relative_path;

const MAX_ARCHIVE: u64 = 100 * 1024 * 1024;
const MAX_EXPANDED: u64 = 128 * 1024 * 1024;
const MAX_PAGE: u64 = 4 * 1024 * 1024;

fn invalid(detail: impl Into<String>) -> RlError {
    RlError::coded(codes::MARKETPLACE_INVALID_RESPONSE, detail.into())
}

pub(super) fn normalize_source(input: &str) -> RlResult<String> {
    let input = input.trim();
    let source = input
        .strip_prefix("https://github.com/")
        .unwrap_or(input)
        .trim_end_matches('/');
    let source = source.strip_suffix(".git").unwrap_or(source);
    let parts: Vec<_> = source.split('/').collect();
    if parts.len() != 2 || !parts.iter().all(|p| validate_part(p)) {
        return Err(RlError::coded(codes::MARKETPLACE_SOURCE_INVALID, input));
    }
    Ok(source.to_ascii_lowercase())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryInfo {
    pub source: String,
    pub stars: u64,
    pub url: String,
}

pub(super) fn repository_info(input: &str) -> RlResult<RepositoryInfo> {
    let source = normalize_source(input)?;
    let response = client()?
        .get(format!("https://api.github.com/repos/{source}"))
        .send()
        .map_err(|e| RlError::coded(codes::MARKETPLACE_UNAVAILABLE, e.to_string()))?;
    if matches!(response.status().as_u16(), 403 | 429) {
        return Err(RlError::coded(codes::MARKETPLACE_RATE_LIMITED, source));
    }
    if !response.status().is_success() {
        return Err(RlError::coded(
            codes::MARKETPLACE_UNAVAILABLE,
            response.status().to_string(),
        ));
    }
    let value: Value = serde_json::from_slice(&read_limited(response, MAX_PAGE)?)
        .map_err(|e| invalid(e.to_string()))?;
    Ok(RepositoryInfo {
        url: format!("https://github.com/{source}"),
        source,
        stars: value["stargazers_count"]
            .as_u64()
            .ok_or_else(|| invalid("missing stars"))?,
    })
}

struct RepositorySnapshot {
    source: String,
    revision: String,
    at: Instant,
    files: Vec<(String, Vec<u8>)>,
}
// 一个仓库快照，列表与逐项安装共用，内存上限明确；刷新时替换，不做磁盘缓存。
static SNAPSHOT: LazyLock<Mutex<Option<Arc<RepositorySnapshot>>>> =
    LazyLock::new(|| Mutex::new(None));

fn snapshot(source: &str, refresh: bool) -> RlResult<Arc<RepositorySnapshot>> {
    if !refresh {
        if let Some(cached) = SNAPSHOT
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|s| s.source == source && s.at.elapsed() < Duration::from_secs(600))
        {
            return Ok(cached.clone());
        }
    }
    let revision =
        latest_commit_sha(source, "")?.ok_or_else(|| invalid("missing repository revision"))?;
    if !revision.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid("invalid revision"));
    }
    let response = client()?
        .get(format!(
            "https://codeload.github.com/{source}/zip/{revision}"
        ))
        .timeout(Duration::from_secs(120))
        .send()
        .map_err(|e| RlError::coded(codes::MARKETPLACE_UNAVAILABLE, e.to_string()))?;
    if !response.status().is_success() {
        return Err(RlError::coded(
            codes::MARKETPLACE_UNAVAILABLE,
            response.status().to_string(),
        ));
    }
    let bytes = read_limited(response, MAX_ARCHIVE)?;
    let files = unpack(&bytes)?;
    let result = Arc::new(RepositorySnapshot {
        source: source.into(),
        revision,
        at: Instant::now(),
        files,
    });
    *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = Some(result.clone());
    Ok(result)
}

fn unpack(bytes: &[u8]) -> RlResult<Vec<(String, Vec<u8>)>> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| invalid(e.to_string()))?;
    if archive.len() > 100_000 {
        return Err(invalid("too many archive entries"));
    }
    let mut files = Vec::new();
    let mut total = 0u64;
    let mut paths = std::collections::HashSet::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| invalid(e.to_string()))?;
        if entry.is_dir() || entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            continue;
        }
        let (_, path) = entry
            .name()
            .split_once('/')
            .ok_or_else(|| invalid("invalid archive root"))?;
        if !is_safe_relative_path(path) {
            return Err(invalid("unsafe archive path"));
        }
        let path = path.to_string();
        if !paths.insert(path.to_ascii_lowercase()) {
            return Err(invalid("duplicate archive path"));
        }
        if entry.size() > MAX_EXPANDED.saturating_sub(total) {
            return Err(RlError::coded(
                codes::MARKETPLACE_RESPONSE_TOO_LARGE,
                "archive expanded size",
            ));
        }
        let mut data = Vec::new();
        (&mut entry)
            .take(MAX_EXPANDED - total + 1)
            .read_to_end(&mut data)?;
        total += data.len() as u64;
        if total > MAX_EXPANDED {
            return Err(RlError::coded(
                codes::MARKETPLACE_RESPONSE_TOO_LARGE,
                "archive expanded size",
            ));
        }
        files.push((path, data));
    }
    Ok(files)
}

fn parse_id(id: &str) -> RlResult<(String, String)> {
    let rest = id
        .strip_prefix("github:")
        .ok_or_else(|| invalid("not a GitHub skill"))?;
    let mut parts = rest.splitn(3, '/');
    let source = normalize_source(&format!(
        "{}/{}",
        parts.next().unwrap_or(""),
        parts.next().unwrap_or("")
    ))?;
    let path = parts.next().unwrap_or("");
    if path != "SKILL.md" && !path.ends_with("/SKILL.md") || !is_safe_relative_path(path) {
        return Err(RlError::coded(codes::MARKETPLACE_ID_INVALID, id));
    }
    Ok((
        source,
        path.strip_suffix("SKILL.md")
            .unwrap()
            .trim_end_matches('/')
            .into(),
    ))
}

pub(super) fn source_for(id: &str) -> RlResult<MarketplaceSource> {
    let (source, repo_dir) = parse_id(id)?;
    Ok(MarketplaceSource {
        id: id.into(),
        url: format!("https://github.com/{source}/tree/HEAD/{repo_dir}"),
        source,
        repo_dir,
        ..Default::default()
    })
}

/// 来源模式与旧 skills.sh 安装记录按仓库路径去重，不覆盖本地内容。
pub(super) fn same_skill(existing: &MarketplaceSource, id: &str) -> bool {
    existing.id == id
        || parse_id(id).is_ok_and(|(source, dir)| {
            existing.source.eq_ignore_ascii_case(&source) && existing.repo_dir == dir
        })
}

pub(super) fn list(input: &str, refresh: bool) -> RlResult<MarketplaceList> {
    let source = normalize_source(input)?;
    let snapshot = snapshot(&source, refresh)?;
    Ok(snapshot_skills(&snapshot))
}

fn snapshot_skills(snapshot: &RepositorySnapshot) -> MarketplaceList {
    let source = &snapshot.source;
    let mut skills = Vec::new();
    for (path, bytes) in &snapshot.files {
        if path != "SKILL.md" && !path.ends_with("/SKILL.md") {
            continue;
        }
        let text = String::from_utf8_lossy(bytes);
        let (name, _) = super::frontmatter::name_description_of(&text);
        skills.push(MarketplaceSkill {
            id: format!("github:{source}/{path}"),
            name: name.unwrap_or_else(|| path.clone()),
            source: source.clone(),
            installs: 0,
            url: format!(
                "https://github.com/{source}/blob/{}/{}",
                snapshot.revision, path
            ),
            installed_skill_id: None,
        });
    }
    skills.sort_by(|a, b| a.id.cmp(&b.id));
    MarketplaceList { skills }
}

pub(super) fn download(id: &str) -> RlResult<MarketplaceDownload> {
    let (source, repo_dir) = parse_id(id)?;
    let snapshot = snapshot(&source, false)?;
    let prefix = if repo_dir.is_empty() {
        String::new()
    } else {
        format!("{repo_dir}/")
    };
    let mut files = Vec::new();
    let mut binary_files = Vec::new();
    let mut skill_md = None;
    for (path, bytes) in &snapshot.files {
        let Some(relative) = path.strip_prefix(&prefix) else {
            continue;
        };
        if relative == "SKILL.md" {
            skill_md = Some(String::from_utf8(bytes.clone()).map_err(|e| invalid(e.to_string()))?);
        }
        match String::from_utf8(bytes.clone()) {
            Ok(contents) => files.push(MarketplaceFile {
                path: relative.into(),
                contents,
            }),
            Err(_) => binary_files.push((relative.into(), bytes.clone())),
        }
    }
    Ok(MarketplaceDownload {
        files,
        binary_files,
        skill_md: skill_md.ok_or_else(|| invalid("SKILL.md not found"))?,
        repo_dir,
        revision: Some(snapshot.revision.clone()),
    })
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicAudit {
    pub provider: String,
    pub name: String,
    pub status: String,
    pub url: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicAuditDetail {
    pub url: String,
    /// 页面头部「Audited by … on <date>」提取的审核时间;展示在标题后,正文不再重复
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audited_at: Option<String>,
    /// 由 <main> 正文转换而来的 Markdown:保留标题/列表/表格结构,已剔除面包屑与元数据卡片
    pub markdown: String,
}

fn selector(css: &str) -> Selector {
    Selector::parse(css).expect("static CSS selector")
}

pub(super) fn audit_list(id: &str) -> RlResult<Vec<PublicAudit>> {
    parse_marketplace_id(id)?;
    let response = client()?
        .get(format!("https://skills.sh/{id}"))
        .send()
        .map_err(|e| RlError::coded(codes::MARKETPLACE_UNAVAILABLE, e.to_string()))?;
    if response.status().as_u16() == 404 {
        return Ok(Vec::new());
    }
    if !response.status().is_success() {
        return Err(RlError::coded(
            codes::MARKETPLACE_UNAVAILABLE,
            response.status().to_string(),
        ));
    }
    let html =
        String::from_utf8(read_limited(response, MAX_PAGE)?).map_err(|e| invalid(e.to_string()))?;
    parse_audits(id, &html)
}

fn parse_audits(id: &str, html: &str) -> RlResult<Vec<PublicAudit>> {
    let document = Html::parse_document(html);
    if document.select(&selector("main h1")).next().is_none() {
        return Err(invalid("unrecognized skill page"));
    }
    let prefix = format!("/{id}/security/");
    let mut result = Vec::new();
    for link in document.select(&selector("a[href]")) {
        let Some(provider) = link
            .value()
            .attr("href")
            .and_then(|href| href.strip_prefix(&prefix))
        else {
            continue;
        };
        if !validate_part(provider)
            || result
                .iter()
                .any(|audit: &PublicAudit| audit.provider == provider)
        {
            continue;
        }
        let parts: Vec<_> = link
            .text()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        let status = parts.last().copied().unwrap_or("Unknown");
        let name = parts[..parts.len().saturating_sub(1)].join(" ");
        result.push(PublicAudit {
            provider: provider.into(),
            name: if name.is_empty() {
                provider.into()
            } else {
                name
            },
            status: status.into(),
            url: format!("https://skills.sh/{id}/security/{provider}"),
        });
    }
    Ok(result)
}

pub(super) fn audit_detail(id: &str, provider: &str) -> RlResult<PublicAuditDetail> {
    parse_marketplace_id(id)?;
    if !validate_part(provider) {
        return Err(invalid("invalid audit provider"));
    }
    let url = format!("https://skills.sh/{id}/security/{provider}");
    let html = fetch_text(&url, MAX_PAGE)?;
    let (markdown, audited_at) = audit_markdown(&html)?;
    Ok(PublicAuditDetail {
        url,
        audited_at,
        markdown,
    })
}

/// 判定「正文块」的选择器:含这些元素的子树视为实质内容。section 必须在内——
/// Socket 页 Checks 区块只有 div/span,漏掉会导致外层容器被元数据规则整棵误删。
const PROSE_BLOCKS: &str = "p,h1,h2,h3,h4,h5,h6,section,ul,ol,table,pre,blockquote";

/// 正文块之外需要整棵跳过的子树:脚本/样式/导航/图标、面包屑(仅链接)
/// 与审计元数据卡片(含 dt/dd 说明列表且无正文块),只保留实质分析内容。
fn skip_subtree(el: &scraper::ElementRef<'_>) -> bool {
    let name = el.value().name();
    if matches!(name, "script" | "style" | "nav" | "svg") {
        return true;
    }
    if el.select(&selector(PROSE_BLOCKS)).next().is_some() {
        return false;
    }
    if el.select(&selector("a")).count() >= 2 {
        return true;
    }
    el.select(&selector("dt,dd")).next().is_some()
}

/// 块级元素:容器递归时遇到它们走结构化分支,其余(纯行内)合并为一段文本。
const BLOCK_TAGS: &str = "p,h1,h2,h3,h4,h5,h6,section,ul,ol,li,table,pre,blockquote,dt,dd";

fn is_blockish(el: &scraper::ElementRef<'_>) -> bool {
    BLOCK_TAGS.contains(&el.value().name()) || el.select(&selector(BLOCK_TAGS)).next().is_some()
}

/// 整段恰为状态词徽章(允许 `**` 加粗包裹,如 `**Pass**`):
/// 状态已展示在标题行,正文段落/行叶子段不再重复。
fn status_badge(text: &str) -> bool {
    matches!(
        text.trim_matches('*').to_ascii_lowercase().as_str(),
        "pass" | "warn" | "fail" | "safe" | "low" | "medium" | "high" | "critical"
    )
}

/// 行叶子段:整段恰为状态词(页头徽章)或是 Risk Level 芯片行时剔除
/// (状态已展示在标题行);其余返回行内文本(徽章/标题 class 已由 inline_text 加粗)。
fn style_leaf(el: scraper::ElementRef<'_>) -> Option<String> {
    let text = inline_text(el);
    if header_noise(&text) || status_badge(&text) {
        return None;
    }
    Some(text)
}

fn collapse_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 行内文本:跳过脚本/样式/图标,code 包反引号;不输出链接语法,避免站内相对链接进入 Markdown。
fn inline_text(el: scraper::ElementRef<'_>) -> String {
    fn walk_el(el: scraper::ElementRef<'_>, out: &mut String, in_pre: bool) {
        let name = el.value().name();
        if matches!(name, "script" | "style" | "nav" | "svg") {
            return;
        }
        if let Some(class) = el.value().attr("class") {
            // 状态徽章与小节标题带 uppercase / snyk-sev class,加粗以保留视觉层次
            if class.contains("uppercase") || class.contains("snyk-sev") {
                let inner: String = collapse_ws(&el.text().collect::<Vec<_>>().join(" "));
                if !inner.is_empty() {
                    out.push_str("**");
                    out.push_str(&inner);
                    out.push_str("** ");
                }
                return;
            }
        }
        if name == "code" && !in_pre {
            let inner: String = el.text().collect::<Vec<_>>().join(" ");
            out.push('`');
            out.push_str(inner.trim());
            out.push('`');
            return;
        }
        let in_pre = in_pre || name == "pre";
        for child in el.children() {
            match child.value() {
                scraper::node::Node::Text(text) => {
                    out.push_str(&text);
                    out.push(' ');
                }
                scraper::node::Node::Element(_) => {
                    if let Some(child_el) = scraper::ElementRef::wrap(child) {
                        walk_el(child_el, out, in_pre);
                    }
                }
                _ => {}
            }
        }
    }
    let mut out = String::new();
    walk_el(el, &mut out, false);
    collapse_ws(&out)
}

fn push_para(text: &str, out: &mut String) {
    if !text.is_empty() {
        out.push_str(text);
        out.push_str("\n\n");
    }
}

fn append_element(el: scraper::ElementRef<'_>, out: &mut String) {
    let name = el.value().name();
    match name {
        // h1 恒为技能名,应用内标题已标明,不重复展示
        "h1" => {}
        "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level = name[1..].parse::<usize>().unwrap_or(2);
            out.push_str(&"#".repeat(level));
            out.push(' ');
            push_para(&inline_text(el), out);
        }
        "p" | "blockquote" => {
            let text = inline_text(el);
            if !header_noise(&text) && !status_badge(&text) {
                push_para(&text, out);
            }
        }
        "ul" => {
            for li in el.select(&selector("li")) {
                out.push_str("- ");
                out.push_str(&inline_text(li));
                out.push('\n');
            }
            out.push('\n');
        }
        "ol" => {
            for (index, li) in el.select(&selector("li")).enumerate() {
                out.push_str(&format!("{}. ", index + 1));
                out.push_str(&inline_text(li));
                out.push('\n');
            }
            out.push('\n');
        }
        "pre" => {
            out.push_str("```\n");
            out.push_str(&inline_text(el));
            out.push_str("\n```\n\n");
        }
        "table" => {
            let mut header_done = false;
            for tr in el.select(&selector("tr")) {
                let cells: Vec<String> = tr.select(&selector("th,td")).map(inline_text).collect();
                if cells.is_empty() {
                    continue;
                }
                let header = tr.select(&selector("th")).next().is_some();
                out.push_str("| ");
                out.push_str(&cells.join(" | "));
                out.push_str(" |\n");
                if header && !header_done {
                    out.push_str("| ");
                    out.push_str(&"--- | ".repeat(cells.len()));
                    out.push_str("|");
                    out.push('\n');
                    header_done = true;
                }
            }
            out.push('\n');
        }
        "dt" => {
            out.push_str("**");
            out.push_str(&inline_text(el));
            out.push_str("**\n");
        }
        "dd" => {
            out.push_str(&inline_text(el));
            out.push('\n');
        }
        "br" => out.push('\n'),
        _ => {
            // 容器:块级子元素递归;每个行内子元素/文本段独立成段,保留原页面的行结构
            for child in el.children() {
                match child.value() {
                    scraper::node::Node::Text(text) => {
                        let text = collapse_ws(&text);
                        if !status_badge(&text) {
                            push_para(&text, out);
                        }
                    }
                    scraper::node::Node::Element(_) => {
                        if let Some(child_el) = scraper::ElementRef::wrap(child) {
                            if skip_subtree(&child_el) {
                                continue;
                            }
                            if is_blockish(&child_el) {
                                append_element(child_el, out);
                            } else {
                                // 行容器(>=2 个纯 div 行,如 Checks 列表)逐行展开;
                                // 其余行内子元素(标题+说明、图标+文字)合并为一段
                                let mut div_rows = 0usize;
                                let mut has_inline = false;
                                for c in child_el.children() {
                                    if let Some(ce) = scraper::ElementRef::wrap(c) {
                                        if skip_subtree(&ce) {
                                            continue;
                                        }
                                        if ce.value().name() == "div" {
                                            div_rows += 1;
                                        } else {
                                            has_inline = true;
                                        }
                                    }
                                }
                                if !has_inline && div_rows >= 2 {
                                    append_element(child_el, out);
                                } else if let Some(text) = style_leaf(child_el) {
                                    push_para(&text, out);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// 审计页页头在应用内已有等价信息(技能名/状态/来源),正文转换时剔除:
/// h1 技能名、状态词徽章段、「Audited by …」段、Risk Level 芯片行;
/// 审核时间提取为结构化字段,由前端展示在标题后。
const HEADER_NOISE_PREFIXES: &[&str] = &["Audited by", "Risk Level:"];

fn header_noise(text: &str) -> bool {
    HEADER_NOISE_PREFIXES
        .iter()
        .any(|prefix| text.starts_with(prefix))
}

/// 把审计详情 <main> 转为保留结构的 Markdown(标题/列表/表格),返回 (正文, 审核时间)。
fn audit_markdown(html: &str) -> RlResult<(String, Option<String>)> {
    let document = Html::parse_document(html);
    let main = document
        .select(&selector("main"))
        .next()
        .ok_or_else(|| invalid("audit main not found"))?;
    if main.select(&selector("h1")).next().is_none()
        || main.select(&selector("section")).next().is_none()
    {
        return Err(invalid("unrecognized audit page"));
    }
    // 审核时间:头部「Audited by … on <date>」段落,日期取行尾
    // 头部「Audited by … on <date>」段:判定首个文本节点,日期取最后一个文本节点,
    // 不经过扁平化文本解析(页面以 HTML 注释分隔 provider 与日期)
    let audited_at = main.select(&selector("p")).find_map(|p| {
        let starts = p
            .text()
            .next()
            .map(|text| {
                text.trim_start()
                    .to_ascii_lowercase()
                    .starts_with("audited by")
            })
            .unwrap_or(false);
        if !starts {
            return None;
        }
        // 真实页面日期是独立文本节点;单节点 HTML(如测试夹具)取最后一个 " on " 之后的部分
        let last = p
            .text()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .last()?;
        let lower = last.to_ascii_lowercase();
        let date = match lower.rfind(" on ") {
            Some(idx) => last[idx + 4..].trim(),
            None => last,
        };
        Some(date.to_string())
    });
    let mut out = String::new();
    for child in main.children() {
        if let Some(el) = scraper::ElementRef::wrap(child) {
            if !skip_subtree(&el) {
                append_element(el, &mut out);
            }
        }
    }
    let markdown = out
        .split("\n\n")
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if markdown.is_empty() {
        return Err(invalid("empty audit"));
    }
    Ok((markdown, audited_at))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_validation() {
        assert_eq!(
            normalize_source(" https://github.com/Acme/Skills.git/ ").unwrap(),
            "acme/skills"
        );
        for source in [
            "a/..",
            "https://evil.test/a/b",
            "a/b/tree/main",
            "a/b?x=1",
            "a/b/c",
        ] {
            assert!(normalize_source(source).is_err());
        }
        assert!(parse_id("github:a/b/../../SKILL.md").is_err());
        assert_eq!(
            parse_id("github:a/b/skills/demo/SKILL.md").unwrap().1,
            "skills/demo"
        );
        assert_eq!(parse_id("github:a/b/SKILL.md").unwrap().1, "");
    }
    #[test]
    fn audits_are_scoped_to_skill_and_keep_unknown_status() {
        let html = r#"<main><h1>Demo</h1><a href="/a/b/demo/security/snyk"><span>Snyk</span><span>Warn</span></a><a href="/a/b/other/security/socket">Socket Pass</a><a href="/a/b/demo/security/new"><span>New</span><span>Pending</span></a></main>"#;
        let audits = parse_audits("a/b/demo", html).unwrap();
        assert_eq!(audits.len(), 2);
        assert_eq!(audits[0].status, "Warn");
        assert_eq!(audits[1].status, "Pending");
        assert!(parse_audits("a/b/demo", "<html>Challenge</html>").is_err());
    }
    #[test]
    fn audit_markdown_keeps_structure_and_skips_chrome() {
        let html = r#"<main>
            <div><a href="/">skills</a><span>/</span><a href="/a">a</a><span>/</span><a href="/a/b">b</a></div>
            <div><h1>find-skills</h1><div><span>Warn</span><p>Audited by Snyk on Mar 15, 2026</p></div></div>
            <div>
                <div><p>Risk Level: MEDIUM</p></div>
                <section><div>Full Analysis</div><div><div>
                    <p><span>MEDIUM</span> W011: Third-party content exposure.</p>
                    <ul><li>First issue detail.</li><li>Second issue detail.</li></ul>
                </div></div></section>
                <section><h2>Issues (1)</h2><div><div><span>W011</span><span>MEDIUM</span></div><div><p>Exposure description.</p></div></div></section>
                <div><div>Audit Metadata</div><div><div><dt>Risk Level</dt><dd>MEDIUM</dd><dt>Analyzed</dt><dd>Mar 15, 2026</dd></div></div></div>
            </div>
            <script>evil()</script>
        </main>"#;
        let (md, audited_at) = audit_markdown(html).unwrap();
        assert_eq!(audited_at.as_deref(), Some("Mar 15, 2026"));
        // 页头信息(技能名/状态徽章/Audited by/Risk Level)不再出现在正文
        assert!(!md.contains("# find-skills"), "{md}");
        assert!(!md.contains("Audited by"), "{md}");
        assert!(!md.contains("**Warn**"), "{md}");
        assert!(!md.contains("Risk Level"), "{md}");
        assert!(md.contains("## Issues (1)"), "{md}");
        assert!(md.contains("- First issue detail."), "{md}");
        assert!(md.contains("W011: Third-party content exposure."), "{md}");
        assert!(!md.contains("skills / a / b"), "{md}");
        assert!(!md.contains("Audit Metadata"), "{md}");
        assert!(!md.contains("Analyzed"), "{md}");
        assert!(!md.contains("evil"), "{md}");
    }
    #[test]
    fn audit_markdown_keeps_socket_checks_section() {
        // Socket 页结构:Checks 区块只有 div/span(无 p),与含 dt 的元数据列同在 grid 下
        let html = r#"<main>
            <div><h1>find-skills</h1><p>Audited by Socket on Mar 18, 2026</p></div>
            <div>
                <div>
                    <section>
                        <p><span class="uppercase">Pass</span></p>
                        <div><div class="text-sm font-mono uppercase">Checks</div><div>
                            <div><svg/><div><span>Malicious behavior</span><span>Injection, exfiltration</span></div></div>
                            <div><svg/><div><span>Code obfuscation</span><span>Hidden code</span></div></div>
                        </div></div>
                    </section>
                </div>
                <div><div>Audit Metadata</div><div><div><dt>Analyzed At</dt><dd>Mar 18, 2026</dd></div></div></div>
            </div>
        </main>"#;
        let (md, audited_at) = audit_markdown(html).unwrap();
        assert_eq!(audited_at.as_deref(), Some("Mar 18, 2026"));
        // 页头(技能名/Audited by)已剔除,Checks 各项独立成段
        assert!(!md.contains("# find-skills"), "{md}");
        assert!(!md.contains("Audited by"), "{md}");
        assert!(md.contains("**Checks**"), "{md}");
        // 正文状态徽章段(与标题行状态重复的 **Pass**)被剔除
        assert!(!md.contains("Pass"), "{md}");
        let paragraphs: Vec<&str> = md.split("\n\n").collect();
        assert!(
            paragraphs.contains(&"Malicious behavior Injection, exfiltration"),
            "{md}"
        );
        assert!(paragraphs.contains(&"Code obfuscation Hidden code"), "{md}");
    }
    #[test]
    fn audit_markdown_converts_tables_and_inline_code() {
        let html = r#"<main><h1>Demo</h1><section><table><tr><th>Check</th><th>Result</th></tr><tr><td><code>npx skills</code></td><td>Pass</td></tr></table></section></main>"#;
        let (md, _) = audit_markdown(html).unwrap();
        assert!(md.contains("| Check | Result |"), "{md}");
        assert!(md.contains("| --- | --- |"), "{md}");
        assert!(md.contains("| `npx skills` | Pass |"), "{md}");
    }
    #[test]
    fn archive_keeps_all_skills_and_binary_attachments() {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for i in 0..75 {
            zip.start_file(format!("repo-sha/skills/skill-{i}/SKILL.md"), options)
                .unwrap();
            zip.write_all(format!("---\nname: skill-{i}\n---\nDemo").as_bytes())
                .unwrap();
        }
        zip.start_file("repo-sha/skills/skill-0/image.png", options)
            .unwrap();
        zip.write_all(&[0, 255, 128]).unwrap();
        let snapshot = RepositorySnapshot {
            source: "owner/repo".into(),
            revision: "abc123".into(),
            at: Instant::now(),
            files: unpack(&zip.finish().unwrap().into_inner()).unwrap(),
        };
        assert_eq!(snapshot_skills(&snapshot).skills.len(), 75);
        assert_eq!(
            snapshot
                .files
                .iter()
                .find(|(p, _)| p.ends_with("image.png"))
                .unwrap()
                .1,
            vec![0, 255, 128]
        );
    }
    #[test]
    fn archive_rejects_traversal_and_duplicate_case_paths() {
        use std::io::Write;
        for paths in [vec!["root/../bad"], vec!["root/a.txt", "root/A.txt"]] {
            let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
            for path in paths {
                zip.start_file(path, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(b"x").unwrap();
            }
            assert!(unpack(&zip.finish().unwrap().into_inner()).is_err());
        }
    }
    #[test]
    fn legacy_install_matches_repository_directory_not_just_name() {
        let existing = MarketplaceSource {
            id: "owner/repo/demo".into(),
            source: "Owner/Repo".into(),
            repo_dir: "skills/demo".into(),
            ..Default::default()
        };
        assert!(same_skill(
            &existing,
            "github:owner/repo/skills/demo/SKILL.md"
        ));
        assert!(!same_skill(
            &existing,
            "github:owner/repo/other/demo/SKILL.md"
        ));
        assert!(!same_skill(
            &existing,
            "github:other/repo/skills/demo/SKILL.md"
        ));
    }
    #[test]
    #[ignore = "requires public GitHub and skills.sh network access"]
    fn live_find_skills_source_and_audits() {
        // GitHub 未登录 API 限流(60 次/小时)时跳过 GitHub 相关断言,只验证 skills.sh 审计转换
        let mut github_ok = false;
        match repository_info("vercel-labs/skills") {
            Ok(repo) => {
                assert!(repo.stars > 0);
                github_ok = true;
            }
            Err(err) if err.code() == codes::MARKETPLACE_RATE_LIMITED => {
                eprintln!("GitHub API 限流,跳过来源/下载断言");
            }
            Err(err) => panic!("repository_info: {err}"),
        }
        if github_ok {
            let skills = list("vercel-labs/skills", true).unwrap();
            let skill = skills
                .skills
                .iter()
                .find(|s| s.name == "find-skills")
                .expect("find-skills in repository");
            let download = download(&skill.id).unwrap();
            assert!(download.skill_md.contains("find-skills"));
            assert!(download.revision.is_some());
        }
        let audits = audit_list("vercel-labs/skills/find-skills").unwrap();
        assert!(audits.iter().any(|a| a.provider == "snyk"));
        for audit in audits {
            let detail = audit_detail("vercel-labs/skills/find-skills", &audit.provider).unwrap();
            assert!(
                !detail.markdown.contains("# find-skills"),
                "{}",
                detail.markdown
            );
            assert!(!detail.markdown.contains("__next_f"));
            assert!(!detail.markdown.contains("scroll-area-viewport"));
            assert!(detail.audited_at.is_some());
        }
    }
}
