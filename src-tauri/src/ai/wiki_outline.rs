use std::collections::{HashMap, HashSet};

use serde::Deserialize;

use crate::commands::wiki::WikiOutlinePage;
use crate::path_util::repo_relative_str;

/// 严格形态:未知字段直接报错,驱动上层「带错误反馈」的纠错重试
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OutlineDocument {
    title: String,
    description: String,
    sections: Vec<OutlineSection>,
    pages: Vec<OutlinePage>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OutlineSection {
    id: String,
    title: String,
    pages: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OutlinePage {
    id: String,
    title: String,
    description: String,
    importance: String,
    relevant_files: Vec<String>,
    related_pages: Vec<String>,
}

/// 宽松形态:容忍 agent 附加的未知字段(语义校验不变,仍由下方逐条检查)
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LenientOutlineDocument {
    title: String,
    description: String,
    sections: Vec<LenientOutlineSection>,
    pages: Vec<LenientOutlinePage>,
}

#[derive(Deserialize)]
struct LenientOutlineSection {
    id: String,
    title: String,
    pages: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LenientOutlinePage {
    id: String,
    title: String,
    description: String,
    importance: String,
    relevant_files: Vec<String>,
    related_pages: Vec<String>,
}

impl From<LenientOutlineDocument> for OutlineDocument {
    fn from(doc: LenientOutlineDocument) -> Self {
        Self {
            title: doc.title,
            description: doc.description,
            sections: doc
                .sections
                .into_iter()
                .map(|s| OutlineSection {
                    id: s.id,
                    title: s.title,
                    pages: s.pages,
                })
                .collect(),
            pages: doc
                .pages
                .into_iter()
                .map(|p| OutlinePage {
                    id: p.id,
                    title: p.title,
                    description: p.description,
                    importance: p.importance,
                    relevant_files: p.relevant_files,
                    related_pages: p.related_pages,
                })
                .collect(),
        }
    }
}

/// 从可能夹带前言/围栏/尾缀的文本中提取第一个完整平衡的 JSON 对象
/// (字符串字面量与转义正确跳过)。找不到返回 None。
fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in text[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..=start + offset]);
                }
            }
            _ => {}
        }
    }
    None
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
        })
}

fn validation_error(errors: Vec<String>) -> Result<Vec<WikiOutlinePage>, String> {
    Err(format!(
        "wiki outline JSON validation failed:\n- {}",
        errors.join("\n- ")
    ))
}

pub fn parse_outline(
    raw: &str,
    valid_files: &HashSet<String>,
) -> Result<Vec<WikiOutlinePage>, String> {
    let trimmed = raw.trim();
    // 容错降级(参照 deepwiki-open):agent 输出夹带前言/Markdown 围栏/控制尾缀时,
    // 先提取第一个完整平衡的 JSON 对象再解析;完全没有对象才判定格式错误
    let text = if trimmed.starts_with('{') && trimmed.ends_with('}') {
        trimmed.to_string()
    } else {
        match extract_json_object(trimmed) {
            Some(object) => object.to_string(),
            None => return Err(
                "wiki outline response must contain one complete JSON object starting with `{` and ending with `}`; remove commentary, fences, control tokens, and truncated content"
                    .to_string(),
            ),
        }
    };
    // 先严格解析(未知字段报错,驱动精确纠错);仅在「多出未知字段」时降级宽松解析
    let document: OutlineDocument = match serde_json::from_str(&text) {
        Ok(document) => document,
        Err(strict_error) => {
            match serde_json::from_str::<LenientOutlineDocument>(&text) {
                Ok(lenient) => lenient.into(),
                // 宽松也失败 = 存在比未知字段更根本的语法/类型错误,报宽松错误更准确
                Err(lenient_error) => {
                    let _ = strict_error;
                    return Err(format!(
                        "wiki outline JSON parse failed at line {}, column {}: {lenient_error}",
                        lenient_error.line(),
                        lenient_error.column(),
                    ));
                }
            }
        }
    };

    let mut errors = Vec::new();
    if document.title.trim().is_empty() {
        errors.push("root `title` must not be empty".to_string());
    }
    if document.description.trim().is_empty() {
        errors.push("root `description` must not be empty".to_string());
    }
    if !(6..=10).contains(&document.pages.len()) {
        errors.push(format!(
            "`pages` must contain 6-10 items, got {}",
            document.pages.len()
        ));
    }

    let mut page_ids = HashSet::new();
    for page in &document.pages {
        if !valid_id(&page.id) {
            errors.push(format!(
                "page id `{}` must be lowercase, numeric, and hyphen-separated",
                page.id
            ));
        }
        if !page_ids.insert(page.id.clone()) {
            errors.push(format!("duplicate page id `{}`", page.id));
        }
    }

    let mut section_ids = HashSet::new();
    let mut membership = HashMap::<String, String>::new();
    for section in &document.sections {
        if !valid_id(&section.id) {
            errors.push(format!(
                "section id `{}` must be lowercase, numeric, and hyphen-separated",
                section.id
            ));
        }
        if !section_ids.insert(section.id.clone()) {
            errors.push(format!("duplicate section id `{}`", section.id));
        }
        if section.title.trim().is_empty() {
            errors.push(format!("section `{}` has an empty title", section.id));
        }
        if section.pages.is_empty() {
            errors.push(format!(
                "section `{}` must reference at least one page",
                section.id
            ));
        }
        let mut section_pages = HashSet::new();
        for page_id in &section.pages {
            if !section_pages.insert(page_id) {
                errors.push(format!(
                    "section `{}` references page `{page_id}` more than once",
                    section.id
                ));
            }
            if !page_ids.contains(page_id) {
                errors.push(format!(
                    "section `{}` references unknown page `{page_id}`",
                    section.id
                ));
            }
            if let Some(previous) = membership.insert(page_id.clone(), section.title.clone()) {
                errors.push(format!(
                    "page `{page_id}` belongs to multiple sections (`{previous}` and `{}`)",
                    section.title
                ));
            }
        }
    }
    if !document.sections.is_empty() {
        for page_id in &page_ids {
            if !membership.contains_key(page_id) {
                errors.push(format!("page `{page_id}` is not assigned to any section"));
            }
        }
    }

    let mut normalized_files = HashMap::<String, Vec<String>>::new();
    for page in &document.pages {
        if page.title.trim().is_empty() {
            errors.push(format!("page `{}` has an empty title", page.id));
        }
        if page.description.trim().is_empty() {
            errors.push(format!("page `{}` has an empty description", page.id));
        }
        if !matches!(page.importance.as_str(), "high" | "medium" | "low") {
            errors.push(format!(
                "page `{}` has invalid importance `{}`; expected high, medium, or low",
                page.id, page.importance
            ));
        }
        if !(3..=10).contains(&page.relevant_files.len()) {
            errors.push(format!(
                "page `{}` must list 3-10 relevantFiles, got {}",
                page.id,
                page.relevant_files.len()
            ));
        }
        let mut seen_files = HashSet::new();
        let mut files = Vec::new();
        for raw_path in &page.relevant_files {
            let path = repo_relative_str(raw_path);
            if path.is_empty() {
                errors.push(format!("page `{}` contains an empty file path", page.id));
            } else if !valid_files.contains(&path) {
                errors.push(format!(
                    "page `{}` references nonexistent file `{path}`",
                    page.id
                ));
            } else if !seen_files.insert(path.clone()) {
                errors.push(format!(
                    "page `{}` lists file `{path}` more than once",
                    page.id
                ));
            } else {
                files.push(path);
            }
        }
        normalized_files.insert(page.id.clone(), files);

        let mut seen_related = HashSet::new();
        for related_id in &page.related_pages {
            if related_id == &page.id {
                errors.push(format!("page `{}` must not relate to itself", page.id));
            } else if !page_ids.contains(related_id) {
                errors.push(format!(
                    "page `{}` references unknown related page `{related_id}`",
                    page.id
                ));
            } else if !seen_related.insert(related_id) {
                errors.push(format!(
                    "page `{}` lists related page `{related_id}` more than once",
                    page.id
                ));
            }
        }
    }

    if !errors.is_empty() {
        return validation_error(errors);
    }

    Ok(document
        .pages
        .into_iter()
        .enumerate()
        .map(|(index, page)| WikiOutlinePage {
            file: format!("{:02}-{}.md", index + 1, page.id),
            section: membership.get(&page.id).cloned(),
            relevant_files: normalized_files.remove(&page.id).unwrap_or_default(),
            id: page.id,
            title: page.title,
            description: page.description,
            importance: page.importance,
            related_pages: page.related_pages,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;

    fn valid_files() -> HashSet<String> {
        HashSet::from([
            "README.md".to_string(),
            "package.json".to_string(),
            "src/main.ts".to_string(),
        ])
    }

    fn valid_document() -> Value {
        let pages = (1..=6)
            .map(|index| {
                json!({
                    "id": format!("page-{index}"),
                    "title": format!("页面 {index}"),
                    "description": format!("页面 {index} 的说明"),
                    "importance": if index == 1 { "high" } else { "medium" },
                    "relevantFiles": ["README.md", "./package.json", "src\\main.ts"],
                    "relatedPages": if index == 1 { vec!["page-2"] } else { Vec::<&str>::new() },
                })
            })
            .collect::<Vec<_>>();
        json!({
            "title": "项目 Wiki",
            "description": "项目说明",
            "sections": [{
                "id": "section-overview",
                "title": "概览",
                "pages": ["page-1", "page-2", "page-3", "page-4", "page-5", "page-6"]
            }],
            "pages": pages,
        })
    }

    #[test]
    fn parses_and_validates_complete_json_outline() {
        let pages = parse_outline(&valid_document().to_string(), &valid_files()).unwrap();
        assert_eq!(pages.len(), 6);
        assert_eq!(pages[0].id, "page-1");
        assert_eq!(pages[0].file, "01-page-1.md");
        assert_eq!(pages[0].section.as_deref(), Some("概览"));
        assert_eq!(pages[0].related_pages, vec!["page-2"]);
        assert_eq!(
            pages[0].relevant_files,
            vec!["README.md", "package.json", "src/main.ts"]
        );
    }

    #[test]
    fn salvages_json_from_commentary_fences_and_control_tokens() {
        let raw = valid_document().to_string();
        // 前言 / Markdown 围栏 / 控制尾缀:提取第一个完整 JSON 对象后正常解析
        for salvageable in [
            format!("正在生成。\n{raw}"),
            format!("```json\n{raw}\n```"),
            format!("{raw}<]minimax[>"),
        ] {
            let pages = parse_outline(&salvageable, &valid_files()).unwrap();
            assert_eq!(pages.len(), 6);
        }
        // 截断(没有闭合的 `}`)仍拒绝
        assert!(parse_outline(&raw[..raw.len() - 1], &valid_files()).is_err());
        // 完全不是 JSON 仍拒绝
        assert!(parse_outline("无法生成大纲", &valid_files()).is_err());
    }

    #[test]
    fn tolerates_unknown_fields_but_reports_schema_errors() {
        // 未知字段(agent 常附加 extra 键):宽松解析放行
        let mut document = valid_document();
        document["unexpected"] = json!(true);
        document["pages"][0]["extra"] = json!({"note": "agent added"});
        let pages = parse_outline(&document.to_string(), &valid_files()).unwrap();
        assert_eq!(pages.len(), 6);

        // 缺失必需字段:仍报错驱动纠错重试
        let mut document = valid_document();
        document.as_object_mut().unwrap().remove("sections");
        let error = parse_outline(&document.to_string(), &valid_files()).unwrap_err();
        assert!(error.contains("missing field `sections`"));
    }

    #[test]
    fn reports_schema_and_cross_reference_errors() {
        let mut document = valid_document();
        document["pages"][0]["relevantFiles"] = json!(["README.md", "missing.rs"]);
        document["pages"][0]["relatedPages"] = json!(["missing-page"]);
        let error = parse_outline(&document.to_string(), &valid_files()).unwrap_err();
        assert!(error.contains("must list 3-10 relevantFiles, got 2"));
        assert!(error.contains("nonexistent file `missing.rs`"));
        assert!(error.contains("unknown related page `missing-page`"));
    }

    #[test]
    fn rejects_wrong_page_count() {
        let mut document = valid_document();
        document["pages"] = json!([]);
        let error = parse_outline(&document.to_string(), &valid_files()).unwrap_err();
        assert!(error.contains("must contain 6-10 items"));
    }

    #[test]
    fn extracts_first_balanced_json_object_respecting_strings() {
        // 字符串内的花括号与转义引号不影响配平
        let text = r#"前言 {"title": "a } \" {", "pages": []} 尾随"#;
        assert_eq!(
            extract_json_object(text),
            Some(r#"{"title": "a } \" {", "pages": []}"#)
        );
        assert_eq!(extract_json_object("no json here"), None);
        assert_eq!(extract_json_object("{ unclosed"), None);
    }
}
