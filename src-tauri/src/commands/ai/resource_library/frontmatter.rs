//! SKILL.md 的极简 YAML frontmatter 处理(不引入 YAML 解析器)。
//!
//! 约定:`---` 围栏内的 `name:` / `description:` 单行键值对为技能名称与
//! 描述的事实源。后端在 skill_create / skill_update 时生成或同步
//! frontmatter(正文原样保留),body_write 时反向读取 frontmatter 回填
//! skills.json,正文编辑绝不丢内容。

/// 生成最小 frontmatter 块(含尾随换行;值一律双引号转义)
pub fn build(name: &str, description: &str) -> String {
    let description = description.replace('\n', " ");
    format!(
        "---\nname: {}\ndescription: {}\n---\n",
        quote(name),
        quote(&description)
    )
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// 剥离引号(单/双引号)与首尾空白
fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

fn is_fence(line: &str) -> bool {
    line.trim_start_matches('\u{feff}').trim() == "---"
}

fn is_blank_or_bom(line: &str) -> bool {
    line.trim_start_matches('\u{feff}').trim().is_empty()
}

/// 拆分 frontmatter(允许前导空行/BOM):
/// 返回 (围栏内内容, 围栏后的正文);无 frontmatter 返回 None
pub fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let mut offset = 0usize;
    let mut seen_fence = false;
    let mut inner_start = 0usize;
    for line in content.split_inclusive('\n') {
        if is_fence(line) {
            if !seen_fence {
                seen_fence = true;
                inner_start = offset + line.len();
            } else {
                // 围栏内内容去掉末尾换行(parse 按行解析,重建时统一换行)
                let inner = content[inner_start..offset].trim_end_matches(['\r', '\n']);
                return Some((inner, &content[offset + line.len()..]));
            }
        } else if !seen_fence && !is_blank_or_bom(line) {
            return None;
        }
        offset += line.len();
    }
    None
}

/// 是否以 frontmatter 开头(用于判定 create 时用户是否自带 frontmatter)
pub fn starts_with_frontmatter(content: &str) -> bool {
    split_frontmatter(content).is_some()
}

/// 解析围栏内的 name / description(单行 key: value,宽容解析)
pub fn parse(frontmatter: &str) -> (Option<String>, Option<String>) {
    let mut name = None;
    let mut description = None;
    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("name:") {
            name = Some(unquote(value));
        } else if let Some(value) = line.strip_prefix("description:") {
            description = Some(unquote(value));
        }
    }
    (name, description)
}

/// 从正文整体解析 name / description(无 frontmatter 时均为 None)
pub fn name_description_of(content: &str) -> (Option<String>, Option<String>) {
    match split_frontmatter(content) {
        Some((frontmatter, _)) => parse(frontmatter),
        None => (None, None),
    }
}

/// 替换(或前置)frontmatter,正文原样保留
pub fn with_frontmatter(content: &str, name: &str, description: &str) -> String {
    match split_frontmatter(content) {
        Some((_, rest)) => format!("{}{}", build(name, description), rest),
        None => format!("{}{}", build(name, description), content),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_quotes_values() {
        let fm = build("审查代码", "含 \"引号\" 的描述");
        assert!(fm.starts_with("---\nname: \"审查代码\"\n"));
        assert!(fm.contains("description: \"含 \\\"引号\\\" 的描述\"\n---\n"));
    }

    #[test]
    fn split_returns_inner_and_rest_preserved() {
        let content = "---\nname: 甲\ndescription: d\n---\n正文第一行\n第二行\n";
        let (inner, rest) = split_frontmatter(content).unwrap();
        assert_eq!(inner, "name: 甲\ndescription: d");
        assert_eq!(rest, "正文第一行\n第二行\n");
    }

    #[test]
    fn split_tolerates_bom_and_leading_blanks() {
        let content = "\u{feff}\n\n---\nname: x\n---\nbody";
        let (inner, rest) = split_frontmatter(content).unwrap();
        assert_eq!(inner, "name: x");
        assert_eq!(rest, "body");
    }

    #[test]
    fn no_frontmatter_returns_none() {
        assert!(split_frontmatter("普通正文\n# 标题").is_none());
        assert!(!starts_with_frontmatter("---\n不闭合"));
    }

    #[test]
    fn parse_reads_name_and_description() {
        let (name, description) = parse("name: \"甲\"\ndescription: 描述\nother: x");
        assert_eq!(name.as_deref(), Some("甲"));
        assert_eq!(description.as_deref(), Some("描述"));
        let (name, description) = parse("name:甲");
        assert_eq!(name.as_deref(), Some("甲"));
        assert_eq!(description, None);
    }

    #[test]
    fn with_frontmatter_replaces_and_preserves_body() {
        let content = "---\nname: 旧名\ndescription: 旧描述\n---\n# 正文\n内容";
        let out = with_frontmatter(content, "新名", "新描述");
        assert!(out.starts_with("---\nname: \"新名\"\ndescription: \"新描述\"\n---\n"));
        assert!(out.ends_with("# 正文\n内容"));
        // 无 frontmatter → 前置
        let out = with_frontmatter("裸正文", "技能", "");
        assert!(out.starts_with("---\nname: \"技能\"\ndescription: \"\"\n---\n"));
        assert!(out.ends_with("裸正文"));
    }

    #[test]
    fn body_name_description_roundtrip() {
        let content = build("审查", "描述");
        let (name, description) = name_description_of(&content);
        assert_eq!(name.as_deref(), Some("审查"));
        assert_eq!(description.as_deref(), Some("描述"));
    }
}
