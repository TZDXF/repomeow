//! 共享 diff 计算:对齐 `packages/agent/src/harness/tools/edit-diff.ts`
//! (diff 引擎以 `similar` crate 实现 jsdiff 的 diffLines/createTwoFilesPatch 子集)。

use similar::{ChangeTag, TextDiff};

/// 检测主导行结尾(对齐 TS `detectLineEnding`)。
pub fn detect_line_ending(content: &str) -> &'static str {
    let crlf_idx = content.find("\r\n");
    let lf_idx = content.find('\n');
    match (crlf_idx, lf_idx) {
        (_, None) => "\n",
        (None, _) => "\n",
        (Some(crlf), Some(lf)) => {
            if crlf < lf {
                "\r\n"
            } else {
                "\n"
            }
        }
    }
}

/// 规范化为 LF(对齐 TS `normalizeToLF`)。
pub fn normalize_to_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// 还原行结尾(对齐 TS `restoreLineEndings`)。
pub fn restore_line_endings(text: &str, ending: &str) -> String {
    if ending == "\r\n" {
        text.replace('\n', "\r\n")
    } else {
        text.to_string()
    }
}

/// 模糊匹配归一化:NFKC 摘要、行尾空白剥离、智能引号/破折号/特殊空格 → ASCII
/// (对齐 TS `normalizeForFuzzyMatch`;NFKC 摘要以显式映射近似)。
pub fn normalize_for_fuzzy_match(text: &str) -> String {
    text.split('\n')
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}

/// 保留行结尾的行拆分(每行含 `\n`;对齐 TS `splitLinesWithEndings`)。
fn split_lines_with_endings(content: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (index, byte) in content.bytes().enumerate() {
        if byte == b'\n' {
            lines.push(&content[start..=index]);
            start = index + 1;
        }
    }
    if start < content.len() {
        lines.push(&content[start..]);
    }
    lines
}

struct LineSpan {
    start: usize,
    end: usize,
}

#[derive(Clone, Debug)]
pub struct TextReplacement {
    match_index: usize,
    match_length: usize,
    new_text: String,
}

fn get_line_spans(content: &str) -> Vec<LineSpan> {
    let mut offset = 0;
    split_lines_with_endings(content)
        .into_iter()
        .map(|line| {
            let span = LineSpan {
                start: offset,
                end: offset + line.len(),
            };
            offset = span.end;
            span
        })
        .collect()
}

fn get_replacement_line_range(
    lines: &[LineSpan],
    replacement: &TextReplacement,
) -> Result<(usize, usize), String> {
    let replacement_start = replacement.match_index;
    let replacement_end = replacement.match_index + replacement.match_length;

    let mut start_line = None;
    for (index, line) in lines.iter().enumerate() {
        if replacement_start >= line.start && replacement_start < line.end {
            start_line = Some(index);
            break;
        }
    }
    let Some(mut start_line) = start_line else {
        return Err("Replacement range is outside the base content.".to_string());
    };

    while start_line < lines.len() && lines[start_line].end < replacement_end {
        start_line += 1;
    }
    if start_line >= lines.len() {
        return Err("Replacement range is outside the base content.".to_string());
    }

    Ok((start_line, start_line + 1))
}

fn apply_replacements(content: &str, replacements: &[TextReplacement], offset: usize) -> String {
    let mut result = content.to_string();
    for replacement in replacements.iter().rev() {
        let match_index = replacement.match_index - offset;
        result.replace_range(
            match_index..match_index + replacement.match_length,
            &replacement.new_text,
        );
    }
    result
}

/// 把匹配到 `base_content` 的替换应用回 `original_content`,保留未变更行块
/// (对齐 TS `applyReplacementsPreservingUnchangedLines`)。
pub fn apply_replacements_preserving_unchanged_lines(
    original_content: &str,
    base_content: &str,
    replacements: &[TextReplacement],
) -> Result<String, String> {
    let original_lines = split_lines_with_endings(original_content);
    let base_lines = get_line_spans(base_content);
    if original_lines.len() != base_lines.len() {
        return Err(
            "Cannot preserve unchanged lines because the base content has a different line count."
                .to_string(),
        );
    }

    let mut groups: Vec<(usize, usize, Vec<TextReplacement>)> = Vec::new();
    let mut sorted_replacements: Vec<TextReplacement> = replacements.to_vec();
    sorted_replacements.sort_by_key(|replacement| replacement.match_index);
    for replacement in sorted_replacements {
        let (start_line, end_line) =
            get_replacement_line_range(&base_lines, &replacement).map_err(|error| error)?;
        match groups.last_mut() {
            Some(last) if start_line < last.1 => {
                last.1 = last.1.max(end_line);
                last.2.push(replacement);
            }
            _ => {
                groups.push((start_line, end_line, vec![replacement]));
            }
        }
    }

    let mut original_line_index = 0;
    let mut result = String::new();
    for (start_line, end_line, group_replacements) in &groups {
        result.push_str(&original_lines[original_line_index..*start_line].concat());

        let group_start_offset = base_lines[*start_line].start;
        let group_end_offset = base_lines[end_line - 1].end;
        result.push_str(&apply_replacements(
            &base_content[group_start_offset..group_end_offset],
            group_replacements,
            group_start_offset,
        ));
        original_line_index = *end_line;
    }
    result.push_str(&original_lines[original_line_index..].concat());

    Ok(result)
}

/// 模糊查找结果(对齐 TS `FuzzyMatchResult`)。
#[derive(Clone, Debug)]
pub struct FuzzyMatchResult {
    pub found: bool,
    pub index: usize,
    pub match_length: usize,
    pub used_fuzzy_match: bool,
    pub content_for_replacement: String,
}

/// 单条替换(对齐 TS `Edit`)。
#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    pub old_text: String,
    pub new_text: String,
}

/// 应用结果(对齐 TS `AppliedEditsResult`)。
#[derive(Clone, Debug, PartialEq)]
pub struct AppliedEditsResult {
    pub base_content: String,
    pub new_content: String,
}

/// 精确匹配优先、模糊匹配兜底(对齐 TS `fuzzyFindText`)。
pub fn fuzzy_find_text(content: &str, old_text: &str) -> FuzzyMatchResult {
    if let Some(exact_index) = content.find(old_text) {
        return FuzzyMatchResult {
            found: true,
            index: exact_index,
            match_length: old_text.len(),
            used_fuzzy_match: false,
            content_for_replacement: content.to_string(),
        };
    }

    let fuzzy_content = normalize_for_fuzzy_match(content);
    let fuzzy_old_text = normalize_for_fuzzy_match(old_text);
    match fuzzy_content.find(&fuzzy_old_text) {
        None => FuzzyMatchResult {
            found: false,
            index: 0,
            match_length: 0,
            used_fuzzy_match: false,
            content_for_replacement: content.to_string(),
        },
        Some(fuzzy_index) => FuzzyMatchResult {
            found: true,
            index: fuzzy_index,
            match_length: fuzzy_old_text.len(),
            used_fuzzy_match: true,
            content_for_replacement: fuzzy_content,
        },
    }
}

/// 剥离 UTF-8 BOM(对齐 TS `stripBom`)。
pub fn strip_bom(content: &str) -> (String, String) {
    if let Some(stripped) = content.strip_prefix('\u{FEFF}') {
        ("\u{FEFF}".to_string(), stripped.to_string())
    } else {
        (String::new(), content.to_string())
    }
}

fn count_occurrences(content: &str, old_text: &str) -> usize {
    let fuzzy_content = normalize_for_fuzzy_match(content);
    let fuzzy_old_text = normalize_for_fuzzy_match(old_text);
    if fuzzy_old_text.is_empty() {
        return 0;
    }
    fuzzy_content.matches(&fuzzy_old_text).count()
}

fn get_not_found_error(path: &str, edit_index: usize, total_edits: usize) -> String {
    if total_edits == 1 {
        format!(
            "Could not find the exact text in {path}. The old text must match exactly including all whitespace and newlines."
        )
    } else {
        format!(
            "Could not find edits[{edit_index}] in {path}. The oldText must match exactly including all whitespace and newlines."
        )
    }
}

fn get_duplicate_error(
    path: &str,
    edit_index: usize,
    total_edits: usize,
    occurrences: usize,
) -> String {
    if total_edits == 1 {
        format!(
            "Found {occurrences} occurrences of the text in {path}. The text must be unique. Please provide more context to make it unique."
        )
    } else {
        format!(
            "Found {occurrences} occurrences of edits[{edit_index}] in {path}. Each oldText must be unique. Please provide more context to make it unique."
        )
    }
}

fn get_empty_old_text_error(path: &str, edit_index: usize, total_edits: usize) -> String {
    if total_edits == 1 {
        format!("oldText must not be empty in {path}.")
    } else {
        format!("edits[{edit_index}].oldText must not be empty in {path}.")
    }
}

fn get_no_change_error(path: &str, _total_edits: usize) -> String {
    format!(
        "No changes made to {path}. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected."
    )
}

/// 把多条精确文本替换应用到 LF 归一化内容(对齐 TS `applyEditsToNormalizedContent`)。
pub fn apply_edits_to_normalized_content(
    normalized_content: &str,
    edits: &[Edit],
    path: &str,
) -> Result<AppliedEditsResult, String> {
    let normalized_edits: Vec<Edit> = edits
        .iter()
        .map(|edit| Edit {
            old_text: normalize_to_lf(&edit.old_text),
            new_text: normalize_to_lf(&edit.new_text),
        })
        .collect();

    for (index, edit) in normalized_edits.iter().enumerate() {
        if edit.old_text.is_empty() {
            return Err(get_empty_old_text_error(
                path,
                index,
                normalized_edits.len(),
            ));
        }
    }

    let initial_matches: Vec<FuzzyMatchResult> = normalized_edits
        .iter()
        .map(|edit| fuzzy_find_text(normalized_content, &edit.old_text))
        .collect();
    let used_fuzzy_match = initial_matches.iter().any(|found| found.used_fuzzy_match);
    let replacement_base_content = if used_fuzzy_match {
        normalize_for_fuzzy_match(normalized_content)
    } else {
        normalized_content.to_string()
    };

    let mut matched_edits: Vec<(usize, TextReplacement)> = Vec::new();
    for (index, edit) in normalized_edits.iter().enumerate() {
        let match_result = fuzzy_find_text(&replacement_base_content, &edit.old_text);
        if !match_result.found {
            return Err(get_not_found_error(path, index, normalized_edits.len()));
        }

        let occurrences = count_occurrences(&replacement_base_content, &edit.old_text);
        if occurrences > 1 {
            return Err(get_duplicate_error(
                path,
                index,
                normalized_edits.len(),
                occurrences,
            ));
        }

        matched_edits.push((
            index,
            TextReplacement {
                match_index: match_result.index,
                match_length: match_result.match_length,
                new_text: edit.new_text.clone(),
            },
        ));
    }

    matched_edits.sort_by_key(|(_, replacement)| replacement.match_index);
    for window in matched_edits.windows(2) {
        let (previous_index, previous) = &window[0];
        let (current_index, current) = &window[1];
        if previous.match_index + previous.match_length > current.match_index {
            return Err(format!(
                "edits[{previous_index}] and edits[{current_index}] overlap in {path}. Merge them into one edit or target disjoint regions."
            ));
        }
    }

    let base_content = normalized_content.to_string();
    let replacements: Vec<TextReplacement> = matched_edits.into_iter().map(|(_, r)| r).collect();
    let new_content = if used_fuzzy_match {
        apply_replacements_preserving_unchanged_lines(
            normalized_content,
            &replacement_base_content,
            &replacements,
        )
        .map_err(|error| error)?
    } else {
        apply_replacements(&replacement_base_content, &replacements, 0)
    };

    if base_content == new_content {
        return Err(get_no_change_error(path, normalized_edits.len()));
    }

    Ok(AppliedEditsResult {
        base_content,
        new_content,
    })
}

/// 标准统一补丁(对齐 TS `generateUnifiedPatch`,context 默认 4,仅文件头)。
pub fn generate_unified_patch(
    path: &str,
    old_content: &str,
    new_content: &str,
    context_lines: usize,
) -> String {
    let diff = TextDiff::from_lines(old_content, new_content);
    let old_header = format!("a/{path}");
    let new_header = format!("b/{path}");
    diff.unified_diff()
        .context_radius(context_lines)
        .header(&old_header, &new_header)
        .to_string()
}

/// 面向展示的 diff 字符串(带行号与上下文;对齐 TS `generateDiffString`)。
/// jsdiff 的 diffLines 产出「同标记连续行合并」的 part;similar 的逐行 change
/// 按相同 tag 连续合并成等价 part。
pub fn generate_diff_string(
    old_content: &str,
    new_content: &str,
    context_lines: usize,
) -> (String, Option<usize>) {
    let diff = TextDiff::from_lines(old_content, new_content);
    let changes: Vec<(ChangeTag, String)> = diff
        .iter_all_changes()
        .map(|change| (change.tag(), change.value().to_string()))
        .collect();

    // 合并连续同 tag 的行 → (tag, 行列表),对齐 jsdiff part 语义。
    let mut parts: Vec<(ChangeTag, Vec<String>)> = Vec::new();
    for (tag, value) in changes {
        match parts.last_mut() {
            Some((last_tag, lines)) if *last_tag == tag => lines.push(value),
            _ => parts.push((tag, vec![value])),
        }
    }

    let old_lines_count = old_content.split('\n').count();
    let new_lines_count = new_content.split('\n').count();
    let max_line_num = old_lines_count.max(new_lines_count);
    let line_num_width = max_line_num.to_string().len();

    let mut output: Vec<String> = Vec::new();
    let mut old_line_num: usize = 1;
    let mut new_line_num: usize = 1;
    let mut last_was_change = false;
    let mut first_changed_line: Option<usize> = None;

    for (index, (tag, raw)) in parts.iter().enumerate() {
        let mut raw: Vec<&str> = raw
            .iter()
            .map(|line| line.trim_end_matches('\n'))
            .collect::<Vec<_>>();
        // 值以 \n 结尾时 trim 后可能出现空尾;按 jsdiff split("\n") 弹出空尾。
        if raw.last() == Some(&"") {
            raw.pop();
        }

        match tag {
            ChangeTag::Delete | ChangeTag::Insert => {
                if first_changed_line.is_none() {
                    first_changed_line = Some(new_line_num);
                }
                for line in &raw {
                    match tag {
                        ChangeTag::Insert => {
                            output.push(format!(
                                "+{} {}",
                                pad_left(new_line_num, line_num_width),
                                line
                            ));
                            new_line_num += 1;
                        }
                        _ => {
                            output.push(format!(
                                "-{} {}",
                                pad_left(old_line_num, line_num_width),
                                line
                            ));
                            old_line_num += 1;
                        }
                    }
                }
                last_was_change = true;
            }
            ChangeTag::Equal => {
                let next_part_is_change = index + 1 < parts.len()
                    && matches!(parts[index + 1].0, ChangeTag::Insert | ChangeTag::Delete);
                let has_leading_change = last_was_change;
                let has_trailing_change = next_part_is_change;

                if has_leading_change && has_trailing_change {
                    if raw.len() <= context_lines * 2 {
                        for line in &raw {
                            output.push(format!(
                                " {} {}",
                                pad_left(old_line_num, line_num_width),
                                line
                            ));
                            old_line_num += 1;
                            new_line_num += 1;
                        }
                    } else {
                        let leading: Vec<&str> = raw.iter().take(context_lines).copied().collect();
                        let trailing: Vec<&str> = raw
                            .iter()
                            .skip(raw.len() - context_lines)
                            .copied()
                            .collect();
                        let skipped = raw.len() - leading.len() - trailing.len();

                        for line in &leading {
                            output.push(format!(
                                " {} {}",
                                pad_left(old_line_num, line_num_width),
                                line
                            ));
                            old_line_num += 1;
                            new_line_num += 1;
                        }
                        output.push(format!(" {} ...", " ".repeat(line_num_width)));
                        old_line_num += skipped;
                        new_line_num += skipped;
                        for line in &trailing {
                            output.push(format!(
                                " {} {}",
                                pad_left(old_line_num, line_num_width),
                                line
                            ));
                            old_line_num += 1;
                            new_line_num += 1;
                        }
                    }
                } else if has_leading_change {
                    let shown: Vec<&str> = raw.iter().take(context_lines).copied().collect();
                    let skipped = raw.len() - shown.len();
                    for line in &shown {
                        output.push(format!(
                            " {} {}",
                            pad_left(old_line_num, line_num_width),
                            line
                        ));
                        old_line_num += 1;
                        new_line_num += 1;
                    }
                    if skipped > 0 {
                        output.push(format!(" {} ...", " ".repeat(line_num_width)));
                        old_line_num += skipped;
                        new_line_num += skipped;
                    }
                } else if has_trailing_change {
                    let skipped = raw.len().saturating_sub(context_lines);
                    if skipped > 0 {
                        output.push(format!(" {} ...", " ".repeat(line_num_width)));
                        old_line_num += skipped;
                        new_line_num += skipped;
                    }
                    for line in raw.iter().skip(skipped) {
                        output.push(format!(
                            " {} {}",
                            pad_left(old_line_num, line_num_width),
                            line
                        ));
                        old_line_num += 1;
                        new_line_num += 1;
                    }
                } else {
                    old_line_num += raw.len();
                    new_line_num += raw.len();
                }
                last_was_change = false;
            }
        }
    }

    (
        output.join(
            "
",
        ),
        first_changed_line,
    )
}

fn pad_left(value: usize, width: usize) -> String {
    let text = value.to_string();
    if text.len() >= width {
        text
    } else {
        format!("{}{}", " ".repeat(width - text.len()), text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_ending_detection_and_restore() {
        assert_eq!(detect_line_ending("a\r\nb"), "\r\n");
        assert_eq!(detect_line_ending("a\nb\r\nc"), "\n");
        assert_eq!(detect_line_ending("no newline"), "\n");
        let normalized = normalize_to_lf("a\r\nb\rc\n");
        assert_eq!(normalized, "a\nb\nc\n");
        assert_eq!(restore_line_endings(&normalized, "\r\n"), "a\r\nb\r\nc\r\n");
    }

    #[test]
    fn fuzzy_normalization_maps_unicode() {
        assert_eq!(normalize_for_fuzzy_match("a\u{2019}b"), "a'b");
        assert_eq!(normalize_for_fuzzy_match("\u{201C}q\u{201D}"), "\"q\"");
        assert_eq!(normalize_for_fuzzy_match("x\u{2013}y"), "x-y");
        assert_eq!(normalize_for_fuzzy_match("a\u{00A0}b"), "a b");
        assert_eq!(normalize_for_fuzzy_match("line   \nnext"), "line\nnext");
    }

    #[test]
    fn strip_bom_works() {
        let (bom, text) = strip_bom("\u{FEFF}body");
        assert_eq!(bom, "\u{FEFF}");
        assert_eq!(text, "body");
        let (bom, text) = strip_bom("body");
        assert_eq!(bom, "");
        assert_eq!(text, "body");
    }

    #[test]
    fn applies_single_exact_edit() {
        let result = apply_edits_to_normalized_content(
            "ALPHA\nbeta\nGAMMA\ndelta\n",
            &[Edit {
                old_text: "beta".to_string(),
                new_text: "BETA".to_string(),
            }],
            "/f.txt",
        )
        .unwrap();
        assert_eq!(result.new_content, "ALPHA\nBETA\nGAMMA\ndelta\n");
        assert_eq!(result.base_content, "ALPHA\nbeta\nGAMMA\ndelta\n");
    }

    #[test]
    fn applies_multiple_disjoint_edits() {
        let result = apply_edits_to_normalized_content(
            "one\ntwo\nthree\nfour\n",
            &[
                Edit {
                    old_text: "one".to_string(),
                    new_text: "ONE".to_string(),
                },
                Edit {
                    old_text: "four".to_string(),
                    new_text: "FOUR".to_string(),
                },
            ],
            "/f.txt",
        )
        .unwrap();
        assert_eq!(result.new_content, "ONE\ntwo\nthree\nFOUR\n");
    }

    #[test]
    fn fuzzy_match_finds_normalized_text() {
        // 归一化内容中 "beta " 行尾空格被剥除。
        let result = apply_edits_to_normalized_content(
            "ALPHA\nbeta\u{00A0}\nGAMMA\n",
            &[Edit {
                old_text: "beta".to_string(),
                new_text: "BETA".to_string(),
            }],
            "/f.txt",
        );
        // 精确匹配直接命中 "beta",因此不走模糊路径。
        let result = result.unwrap();
        assert_eq!(result.new_content, "ALPHA\nBETA\u{00A0}\nGAMMA\n");

        // 模糊路径:oldText 带行尾空白 + NBSP。
        let result = apply_edits_to_normalized_content(
            "ALPHA\nbeta\u{00A0}\nGAMMA\n",
            &[Edit {
                old_text: "beta\u{00A0} ".to_string(),
                new_text: "BETA\n".to_string(),
            }],
            "/f.txt",
        )
        .unwrap();
        assert!(result.new_content.contains("BETA"));
    }

    #[test]
    fn rejects_not_found_duplicate_empty_overlap_no_change() {
        let not_found = apply_edits_to_normalized_content(
            "abc\n",
            &[Edit {
                old_text: "xyz".to_string(),
                new_text: "1".to_string(),
            }],
            "/f",
        )
        .unwrap_err();
        assert!(not_found.contains("Could not find the exact text"));

        let duplicate = apply_edits_to_normalized_content(
            "abc abc\n",
            &[Edit {
                old_text: "abc".to_string(),
                new_text: "1".to_string(),
            }],
            "/f",
        )
        .unwrap_err();
        assert!(duplicate.contains("occurrences"));

        let empty = apply_edits_to_normalized_content(
            "abc\n",
            &[Edit {
                old_text: String::new(),
                new_text: "1".to_string(),
            }],
            "/f",
        )
        .unwrap_err();
        assert!(empty.contains("must not be empty"));

        let overlap = apply_edits_to_normalized_content(
            "abcdef\n",
            &[
                Edit {
                    old_text: "abc".to_string(),
                    new_text: "1".to_string(),
                },
                Edit {
                    old_text: "cde".to_string(),
                    new_text: "2".to_string(),
                },
            ],
            "/f",
        )
        .unwrap_err();
        assert!(overlap.contains("overlap"));

        let no_change = apply_edits_to_normalized_content(
            "abc\n",
            &[Edit {
                old_text: "abc".to_string(),
                new_text: "abc".to_string(),
            }],
            "/f",
        )
        .unwrap_err();
        assert!(no_change.contains("No changes made"));
    }

    #[test]
    fn multi_edit_not_found_uses_index_message() {
        let error = apply_edits_to_normalized_content(
            "abc\ndef\n",
            &[
                Edit {
                    old_text: "abc".to_string(),
                    new_text: "1".to_string(),
                },
                Edit {
                    old_text: "missing".to_string(),
                    new_text: "2".to_string(),
                },
            ],
            "/f",
        )
        .unwrap_err();
        assert!(error.contains("edits[1]"));
    }

    #[test]
    fn unified_patch_contains_headers_and_changes() {
        let patch = generate_unified_patch(
            "/f.txt",
            "ALPHA\nbeta\nGAMMA\ndelta\n",
            "ALPHA\nBETA\nGAMMA\ndelta\n",
            4,
        );
        assert!(patch.contains("ALPHA"), "{patch}");
        assert!(patch.contains("-beta"));
        assert!(patch.contains("+BETA"));
    }

    #[test]
    fn diff_string_reports_first_changed_line() {
        let (diff, first_changed_line) = generate_diff_string(
            "ALPHA\nbeta\nGAMMA\ndelta\n",
            "ALPHA\nBETA\nGAMMA\ndelta\n",
            4,
        );
        assert!(diff.contains("-2 beta"), "{diff}");
        assert!(diff.contains("+2 BETA"));
        assert_eq!(first_changed_line, Some(2));
    }

    #[test]
    fn diff_string_skips_far_context() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\nn\no\np\n";
        let new = old.replace('b', "B");
        let (diff, _) = generate_diff_string(old, &new, 2);
        assert!(diff.contains("..."));
        // 未变更的远端行不应出现。
        assert!(
            !diff.contains(
                "o
p"
            ) && !diff.contains(" o"),
            "{diff}"
        );
    }
}
