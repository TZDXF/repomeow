use std::collections::{HashMap, HashSet};

use regex::Regex;

use crate::commands::wiki::WikiOutlinePage;

fn unescape_xml(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn tag_text(block: &str, tag: &str) -> String {
    Regex::new(&format!(r"(?s)<{tag}>(.*?)</{tag}>"))
        .ok()
        .and_then(|pattern| pattern.captures(block))
        .and_then(|captures| captures.get(1))
        .map(|value| unescape_xml(value.as_str().trim()))
        .unwrap_or_default()
}

fn tag_texts(block: &str, tag: &str) -> Vec<String> {
    let Ok(pattern) = Regex::new(&format!(r"(?s)<{tag}>(.*?)</{tag}>")) else {
        return Vec::new();
    };
    pattern
        .captures_iter(block)
        .filter_map(|captures| captures.get(1))
        .map(|value| unescape_xml(value.as_str().trim()))
        .filter(|value| !value.is_empty())
        .collect()
}

fn structure_region(raw: &str) -> Option<String> {
    let fence = Regex::new(r"(?i)```(?:xml)?").ok()?;
    let without_fence = fence.replace_all(raw, "");
    let start = without_fence.find("<wiki_structure")?;
    let mut region = without_fence[start..].to_string();
    if !region.contains("</wiki_structure>") {
        region.push_str("</page></pages></wiki_structure>");
    }
    Some(region)
}

fn parse_sections(region: &str) -> HashMap<String, String> {
    let mut sections = HashMap::new();
    let Ok(pattern) = Regex::new(r"(?s)<section\b[^>]*>(.*?)</section>") else {
        return sections;
    };
    for captures in pattern.captures_iter(region) {
        let block = captures
            .get(1)
            .map(|value| value.as_str())
            .unwrap_or_default();
        let title = tag_text(block, "title");
        if title.is_empty() {
            continue;
        }
        for id in tag_text(block, "pages")
            .split(|character: char| character.is_whitespace() || character == ',')
        {
            if !id.is_empty() {
                sections.insert(id.to_string(), title.clone());
            }
        }
    }
    sections
}

fn slugify(value: &str, fallback: &str) -> String {
    let mut slug = String::new();
    let mut dash = false;
    for character in value.to_ascii_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            if dash && !slug.is_empty() {
                slug.push('-');
            }
            dash = false;
            slug.push(character);
            if slug.len() >= 48 {
                break;
            }
        } else {
            dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        fallback.to_string()
    } else {
        slug
    }
}

fn normalize_path(value: &str) -> String {
    value
        .replace('\\', "/")
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

pub fn parse_outline(
    raw: &str,
    valid_files: &HashSet<String>,
) -> Result<Vec<WikiOutlinePage>, String> {
    let region = structure_region(raw)
        .ok_or_else(|| "wiki outline: no <wiki_structure> found".to_string())?;
    let sections = parse_sections(&region);
    let page_split = Regex::new(r"<page\b").map_err(|error| error.to_string())?;
    let id_pattern =
        Regex::new(r#"^[^>]*?\bid\s*=\s*"([^"]+)""#).map_err(|error| error.to_string())?;
    let close_pattern = Regex::new(r"(?s)</page>.*$").map_err(|error| error.to_string())?;
    let mut pages = Vec::new();
    let mut used_ids = HashSet::new();

    for fragment in page_split.split(&region).skip(1) {
        let Some(raw_id) = id_pattern
            .captures(fragment)
            .and_then(|captures| captures.get(1))
            .map(|value| value.as_str().trim())
        else {
            continue;
        };
        let block = close_pattern.replace(fragment, "");
        let page_number = pages.len() + 1;
        let fallback = format!("page-{page_number}");
        let base_id = slugify(raw_id, &fallback);
        let mut id = base_id.clone();
        let mut suffix = 2;
        while used_ids.contains(&id) {
            id = format!("{base_id}-{suffix}");
            suffix += 1;
        }
        used_ids.insert(id.clone());
        let title = tag_text(&block, "title");
        let importance = match tag_text(&block, "importance").to_ascii_lowercase().as_str() {
            "high" => "high",
            "low" => "low",
            _ => "medium",
        }
        .to_string();
        let mut seen_files = HashSet::new();
        let relevant_files = tag_texts(&block, "file_path")
            .into_iter()
            .map(|path| normalize_path(&path))
            .filter(|path| valid_files.contains(path) && seen_files.insert(path.clone()))
            .collect();
        pages.push(WikiOutlinePage {
            id: id.clone(),
            file: format!("{page_number:02}-{id}.md"),
            title: if title.is_empty() {
                raw_id.to_string()
            } else {
                title
            },
            description: tag_text(&block, "description"),
            section: sections.get(raw_id).or_else(|| sections.get(&id)).cloned(),
            importance,
            relevant_files,
            related_pages: tag_texts(&block, "related"),
        });
    }

    if pages.is_empty() {
        Err("wiki outline: no <page> parsed".to_string())
    } else {
        Ok(pages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_XML: &str = r#"<wiki_structure>
<sections><section><title>Overview</title><pages>overview architecture</pages></section></sections>
<pages>
<page id="overview"><title>项目概览</title><description>What this project is</description><importance>high</importance><file_path>README.md</file_path><file_path>./package.json</file_path><related>architecture</related></page>
<page id="architecture"><title>Architecture &amp; Layers</title><importance>medium</importance><file_path>src\lib\ai.ts</file_path><file_path>not/exist.ts</file_path></page>
</pages></wiki_structure>"#;

    fn valid_files() -> HashSet<String> {
        HashSet::from([
            "README.md".to_string(),
            "package.json".to_string(),
            "src/lib/ai.ts".to_string(),
        ])
    }

    #[test]
    fn parses_fields_sections_entities_and_normalized_paths() {
        let pages = parse_outline(VALID_XML, &valid_files()).unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].id, "overview");
        assert_eq!(pages[0].file, "01-overview.md");
        assert_eq!(pages[0].title, "项目概览");
        assert_eq!(pages[0].section.as_deref(), Some("Overview"));
        assert_eq!(pages[0].importance, "high");
        assert_eq!(pages[0].related_pages, vec!["architecture"]);
        assert_eq!(pages[0].relevant_files, vec!["README.md", "package.json"]);
        assert_eq!(pages[1].title, "Architecture & Layers");
        assert_eq!(pages[1].relevant_files, vec!["src/lib/ai.ts"]);
    }

    #[test]
    fn parses_truncated_outline_and_filters_files() {
        let raw = r#"```xml
noise<wiki_structure><sections><section><title>Overview</title><pages>core</pages></section></sections><pages>
<page id="core"><title>核心 &amp; 架构</title><importance>high</importance><file_path>./src/main.rs</file_path></page>
<page id="core"><title>重复</title><importance>bad</importance>"#;
        let pages = parse_outline(raw, &HashSet::from(["src/main.rs".to_string()])).unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].id, "core");
        assert_eq!(pages[0].title, "核心 & 架构");
        assert_eq!(pages[0].section.as_deref(), Some("Overview"));
        assert_eq!(pages[0].relevant_files, vec!["src/main.rs"]);
        assert_eq!(pages[1].id, "core-2");
        assert_eq!(pages[1].importance, "medium");
    }

    #[test]
    fn accepts_fences_and_leading_noise() {
        let raw = format!("好的，以下是结构：\n```xml\n{VALID_XML}\n```");
        assert_eq!(parse_outline(&raw, &valid_files()).unwrap().len(), 2);
    }

    #[test]
    fn rejects_missing_structure_or_pages() {
        assert!(parse_outline("抱歉，无法完成", &HashSet::new()).is_err());
        assert!(parse_outline(
            "<wiki_structure><title>x</title></wiki_structure>",
            &HashSet::new(),
        )
        .is_err());
    }
}
