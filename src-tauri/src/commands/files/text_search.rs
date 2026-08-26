use std::path::Path;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use super::{ensure_dir, BINARY_SNIFF_BYTES};
use crate::commands::walk;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::{TextSearchHit, TextSearchLine, TextSearchOutcome};

/// 单文件搜索的读取上限,与预览一致(512KB):预览只展示这么多,
/// 超出部分的命中跳不过去,搜了也无法定位
const SEARCH_MAX_FILE_BYTES: u64 = 512 * 1024;
/// 匹配总数 / 命中文件数上限,防止超大仓库的结果拖垮前端渲染
pub(super) const SEARCH_MAX_MATCHES: u32 = 1000;
const SEARCH_MAX_FILES: usize = 200;
/// 结果行预览最大字节数,超出截取首个匹配附近窗口(首尾以 … 标记)
const SEARCH_LINE_PREVIEW_MAX: usize = 500;
/// 窗口在匹配前后保留的上下文字节数
const SEARCH_LINE_CONTEXT: usize = 80;

/// 搜索匹配器:大小写敏感的纯文本走 find 循环,其余统一走 regex——
/// Unicode 大小写折叠可能改变字节长度(İ → i̇),小写化后的下标无法映射回原文
enum SearchMatcher {
    Literal(String),
    Regex(regex::Regex),
}

impl SearchMatcher {
    fn build(
        query: &str,
        case_sensitive: bool,
        whole_word: bool,
        use_regex: bool,
    ) -> AppResult<Self> {
        if case_sensitive && !whole_word && !use_regex {
            return Ok(Self::Literal(query.to_string()));
        }
        let pattern = match (use_regex, whole_word) {
            (true, true) => format!(r"\b(?:{query})\b"),
            (true, false) => query.to_string(),
            (false, true) => format!(r"\b{}\b", regex::escape(query)),
            (false, false) => regex::escape(query),
        };
        let re = regex::RegexBuilder::new(&pattern)
            .case_insensitive(!case_sensitive)
            .build()
            .map_err(|e| AppError::coded(ErrorCode::SearchInvalidRegex, e.to_string()))?;
        Ok(Self::Regex(re))
    }

    /// 行内全部匹配的 [起,止) 字节偏移(升序)
    fn find_all(&self, line: &str) -> Vec<(usize, usize)> {
        match self {
            Self::Literal(needle) => {
                let mut out = Vec::new();
                let mut from = 0;
                while let Some(i) = line[from..].find(needle) {
                    let start = from + i;
                    from = start + needle.len();
                    out.push((start, from));
                }
                out
            }
            Self::Regex(re) => re.find_iter(line).map(|m| (m.start(), m.end())).collect(),
        }
    }
}

/// 文件包含/排除筛选,匹配工作区根目录下的 '/' 分隔相对路径。
/// VS Code 搜索视图中不带路径分隔符的模式默认匹配任意层级;
/// 目录模式通过同时检查文件路径及其祖先路径实现。
struct SearchFileFilter {
    include: GlobSet,
    exclude: GlobSet,
}

impl SearchFileFilter {
    fn build(include: &str, exclude: &str) -> AppResult<Self> {
        Ok(Self {
            include: build_search_glob_set(include)?,
            exclude: build_search_glob_set(exclude)?,
        })
    }

    fn matches(&self, path: &str) -> bool {
        let included = self.include.is_empty() || path_matches_glob(&self.include, path);
        included && !path_matches_glob(&self.exclude, path)
    }
}

/// 编译 VS Code Search view 风格的逗号分隔 glob 列表。
fn build_search_glob_set(input: &str) -> AppResult<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    let mut count = 0;
    for raw in split_search_globs(input) {
        let Some(pattern) = normalize_search_glob(&raw) else {
            continue;
        };
        let mut glob = GlobBuilder::new(&pattern);
        glob.literal_separator(true)
            .backslash_escape(false)
            .case_insensitive(cfg!(windows) || cfg!(target_os = "macos"));
        let glob = glob
            .build()
            .map_err(|e| AppError::coded(ErrorCode::SearchInvalidGlob, e.to_string()))?;
        builder.add(glob);
        count += 1;
    }
    if count == 0 {
        return Ok(GlobSet::empty());
    }
    builder
        .build()
        .map_err(|e| AppError::coded(ErrorCode::SearchInvalidGlob, e.to_string()))
}

/// 在逗号分隔时保留 `{a,b}` 和 `[a,b]` 内部的逗号。
fn split_search_globs(input: &str) -> Vec<String> {
    let mut patterns = Vec::new();
    let mut current = String::new();
    let mut braces = 0usize;
    let mut classes = 0usize;
    for ch in input.chars() {
        match ch {
            '{' => braces += 1,
            '}' => braces = braces.saturating_sub(1),
            '[' => classes += 1,
            ']' => classes = classes.saturating_sub(1),
            ',' if braces == 0 && classes == 0 => {
                patterns.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(ch);
    }
    patterns.push(current);
    patterns
}

fn normalize_search_glob(raw: &str) -> Option<String> {
    let mut pattern = raw.trim().replace('\\', "/");
    if pattern.is_empty() {
        return None;
    }
    let root_only = pattern.starts_with("./");
    if root_only {
        pattern.drain(..2);
    }
    if pattern.ends_with('/') {
        pattern.push_str("**");
    }
    if !root_only && !pattern.contains('/') {
        pattern = format!("**/{pattern}");
    }
    Some(pattern)
}

fn path_matches_glob(set: &GlobSet, path: &str) -> bool {
    if set.is_empty() {
        return false;
    }
    let mut candidate = path;
    loop {
        if set.is_match(candidate) {
            return true;
        }
        let Some((parent, _)) = candidate.rsplit_once('/') else {
            break;
        };
        candidate = parent;
    }
    false
}

/// 超长行(压缩产物等)截取首个匹配前后的窗口,窗口边界向下取整到 UTF-8 字符边界
fn window_line(line: &str, matches: &[(usize, usize)]) -> String {
    if line.len() <= SEARCH_LINE_PREVIEW_MAX {
        return line.to_string();
    }
    let first = matches.first().map_or(0, |m| m.0);
    let last = matches.last().map_or(line.len(), |m| m.1);
    let floor = |i: usize| {
        let mut i = i.min(line.len());
        while i > 0 && !line.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let start = floor(first.saturating_sub(SEARCH_LINE_CONTEXT));
    let end = floor(last + SEARCH_LINE_CONTEXT).max(start);
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.push_str(&line[start..end]);
    if end < line.len() {
        out.push('…');
    }
    out
}

/// 搜索单个文件:返回 (匹配总数, 命中行);二进制 / 读取失败 / 已达上限返回 None。
/// 行号口径与前端预览一致:按 \n 切行,1-based。
/// 读取上限 SEARCH_MAX_FILE_BYTES(512KB,与 read_file_preview 一致):
/// 超出部分的字节直接丢弃(被裁的尾部可能漏掉命中,但前端预览本来也只展示这么多,
/// 搜到也无法在打开文件后跳转定位)
fn search_one_file(
    root: &Path,
    rel: &Path,
    matcher: &SearchMatcher,
    total: &std::sync::atomic::AtomicU32,
) -> Option<(u32, Vec<TextSearchLine>)> {
    use std::io::Read as _;
    if total.load(std::sync::atomic::Ordering::Relaxed) >= SEARCH_MAX_MATCHES {
        return None;
    }
    let mut file = std::fs::File::open(root.join(rel)).ok()?;
    let mut bytes = Vec::new();
    // 先读 8KB 做二进制嗅探,非二进制再补足到 512KB 上限,避免给大文件整段无谓 IO
    (&mut file)
        .take(BINARY_SNIFF_BYTES as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.contains(&0) {
        return None;
    }
    if bytes.len() == BINARY_SNIFF_BYTES {
        (&mut file)
            .take(SEARCH_MAX_FILE_BYTES - BINARY_SNIFF_BYTES as u64)
            .read_to_end(&mut bytes)
            .ok()?;
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = Vec::new();
    let mut count = 0u32;
    for (idx, line) in text.split('\n').enumerate() {
        // 达到总上限后停止累计(标志位由调用方在汇总时判定)
        if total.load(std::sync::atomic::Ordering::Relaxed) >= SEARCH_MAX_MATCHES {
            break;
        }
        let matches = matcher.find_all(line);
        if matches.is_empty() {
            continue;
        }
        count += matches.len() as u32;
        total.fetch_add(matches.len() as u32, std::sync::atomic::Ordering::Relaxed);
        lines.push(TextSearchLine {
            line: idx as u32 + 1,
            text: window_line(line, &matches),
        });
    }
    if count == 0 {
        None
    } else {
        Some((count, lines))
    }
}

/// 项目全文搜索(文件预览页左栏"搜索"视图)。遍历范围见 walk::searchable_files:
/// 尊重 git 忽略规则、含隐藏文件、跳过 node_modules 与 .git;
/// 二进制跳过,单文件只搜前 512KB(与预览一致,保证命中行可跳转)。
/// 匹配按线程并行;结果按路径排序,超上限时置 truncated 并截断。
pub(super) fn search_project_text(
    root: String,
    query: String,
    case_sensitive: bool,
    whole_word: bool,
    use_regex: bool,
    include: String,
    exclude: String,
) -> AppResult<TextSearchOutcome> {
    ensure_dir(&root)?;
    let query = query.trim();
    if query.is_empty() {
        return Ok(TextSearchOutcome {
            hits: Vec::new(),
            truncated: false,
        });
    }
    let matcher = SearchMatcher::build(query, case_sensitive, whole_word, use_regex)?;
    let filter = SearchFileFilter::build(&include, &exclude)?;
    let root_path = Path::new(&root);
    let files: Vec<_> = walk::searchable_files(root_path)
        .into_iter()
        .filter(|rel| filter.matches(&walk::to_slash(rel)))
        .collect();

    let total = std::sync::atomic::AtomicU32::new(0);
    let hits: std::sync::Mutex<Vec<TextSearchHit>> = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let chunk_size = (files.len() + threads - 1) / threads;
        let matcher = &matcher;
        for chunk in files.chunks(chunk_size.max(1)) {
            let hits = &hits;
            let total = &total;
            scope.spawn(move || {
                for rel in chunk {
                    if let Some((count, lines)) = search_one_file(root_path, rel, &matcher, total) {
                        hits.lock().unwrap().push(TextSearchHit {
                            path: walk::to_slash(rel),
                            count,
                            lines,
                        });
                    }
                }
            });
        }
    });
    let mut hits = hits.into_inner().unwrap();
    hits.sort_by(|a, b| a.path.cmp(&b.path));
    // 两侧都用 `>=`:达到上限即视为截断(用户能看到的范围就是 truncated 之后的内容)
    let truncated = total.load(std::sync::atomic::Ordering::Relaxed) >= SEARCH_MAX_MATCHES
        || hits.len() >= SEARCH_MAX_FILES;
    hits.truncate(SEARCH_MAX_FILES);
    Ok(TextSearchOutcome { hits, truncated })
}
