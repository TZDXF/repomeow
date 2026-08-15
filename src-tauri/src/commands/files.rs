use std::path::Path;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::commands::walk;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::{
    ComposeFile, ComposePort, ComposeService, FilePreview, ProjectFileEntry, ReadmeContent,
    TextSearchHit, TextSearchLine, TextSearchOutcome,
};

/// README 候选文件名,按优先级排列(大小写常见变体)
const README_CANDIDATES: &[&str] = &[
    "README.md",
    "readme.md",
    "README.MD",
    "Readme.md",
    "README.markdown",
    "README.txt",
    "README",
];

/// README 读取上限 512KB,避免超大文件拖垮前端渲染
const README_MAX_BYTES: u64 = 512 * 1024;

/// compose 文件大小上限 256KB,超过的直接跳过(正常 compose 文件远小于此)
const COMPOSE_MAX_BYTES: u64 = 256 * 1024;

pub(crate) fn ensure_dir(path: &str) -> AppResult<()> {
    if !Path::new(path).is_dir() {
        return Err(AppError::coded(ErrorCode::InvalidPath, path));
    }
    Ok(())
}

/// 在目录中按候选名查找文件,返回第一个存在的文件名。
/// 用 read_dir 做大小写精确匹配,避免 Windows/macOS 大小写不敏感文件系统
/// 把 readme.md 误判成 README.md,保证候选优先级在所有平台行为一致。
fn find_file(dir: &Path, candidates: &[&str]) -> Option<String> {
    let existing: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    candidates
        .iter()
        .find(|name| existing.iter().any(|f| f == *name))
        .map(|name| name.to_string())
}

/// 读取项目 README;不存在时返回 None
#[tauri::command]
pub fn read_readme(path: String) -> AppResult<Option<ReadmeContent>> {
    ensure_dir(&path)?;
    let dir = Path::new(&path);
    let Some(file_name) = find_file(dir, README_CANDIDATES) else {
        return Ok(None);
    };
    let file = dir.join(&file_name);
    // 超过上限只取前 README_MAX_BYTES 字节(按 UTF-8 边界截断)
    let meta = std::fs::metadata(&file)?;
    let content = if meta.len() > README_MAX_BYTES {
        let bytes = std::fs::read(&file)?;
        // 按 UTF-8 边界截断:跳过 continuation byte(0b10xxxxxx)
        let mut end = README_MAX_BYTES as usize;
        while end > 0 && (bytes[end] & 0xC0) == 0x80 {
            end -= 1;
        }
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    } else {
        std::fs::read_to_string(&file)?
    };
    Ok(Some(ReadmeContent { file_name, content }))
}

/// 列出项目内某目录的直接子项(文件树逐层懒加载;dir 为 None/空串时列根层)。
/// 含隐藏文件与 node_modules,仅跳过 .git;文件与目录都返回(空目录可见),
/// ignored 标记是否被 .gitignore / .ignore 排除(前端灰显用);结果按路径排序
#[tauri::command]
pub fn list_project_files(path: String, dir: Option<String>) -> AppResult<Vec<ProjectFileEntry>> {
    ensure_dir(&path)?;
    let root = Path::new(&path);
    let rel = dir.unwrap_or_default();
    // dir 必须解析到 root 内的目录:canonicalize 后比较前缀,拒绝 .. 越界与符号链接逃逸
    let root_canon = std::fs::canonicalize(root)?;
    let target = if rel.is_empty() {
        root_canon.clone()
    } else {
        std::fs::canonicalize(root.join(&rel))
            .map_err(|_| AppError::coded(ErrorCode::InvalidPath, rel.clone()))?
    };
    if !target.starts_with(&root_canon) || !target.is_dir() {
        return Err(AppError::coded(ErrorCode::InvalidPath, rel));
    }
    Ok(walk::dir_entries(root, Path::new(&rel))
        .into_iter()
        .map(|e| ProjectFileEntry {
            path: walk::to_slash(&e.path),
            ignored: e.ignored,
            is_dir: e.is_dir,
        })
        .collect())
}

/// 文件名搜索(文件树头部搜索框):在未被 .gitignore / .ignore 排除的文件中
/// 按相对路径做大小写不敏感子串匹配(与原前端「被排除文件不参与」口径一致),
/// 遍历命中 limit 条即提前退出;结果按路径排序。空查询返回空
#[tauri::command]
pub fn search_project_files(
    path: String,
    query: String,
    limit: Option<u32>,
) -> AppResult<Vec<ProjectFileEntry>> {
    ensure_dir(&path)?;
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.unwrap_or(50).max(1) as usize;
    Ok(walk::search_file_paths(Path::new(&path), &needle, limit)
        .into_iter()
        .map(|p| ProjectFileEntry {
            path: walk::to_slash(&p),
            ignored: false,
            is_dir: false,
        })
        .collect())
}

/// 文件预览读取上限 512KB,与 README 一致;超出按 UTF-8 边界截断
const PREVIEW_MAX_BYTES: u64 = 512 * 1024;

/// 二进制嗅探:读取前缀内出现 NUL 字节即视为二进制(与 git diff 的嗅探口径一致)
const BINARY_SNIFF_BYTES: usize = 8_000;

/// 读取项目内单个文件的预览内容。
/// root 为项目根目录,rel_path 为 list_project_files 返回的相对路径;
/// canonicalize 后必须仍位于 root 内,拒绝 `..` 越界与符号链接逃逸。
/// 二进制文件返回 text = None;文本超过 512KB 截断并置 truncated。
#[tauri::command]
pub fn read_file_preview(root: String, rel_path: String) -> AppResult<FilePreview> {
    ensure_dir(&root)?;
    let root_canon = std::fs::canonicalize(&root)?;
    let file = std::fs::canonicalize(root_canon.join(&rel_path))
        .map_err(|_| AppError::coded(ErrorCode::InvalidPath, rel_path.clone()))?;
    if !file.starts_with(&root_canon) || !file.is_file() {
        return Err(AppError::coded(ErrorCode::InvalidPath, rel_path));
    }
    let bytes = std::fs::read(&file)?;
    let sniff = &bytes[..bytes.len().min(BINARY_SNIFF_BYTES)];
    if sniff.contains(&0) {
        return Ok(FilePreview {
            text: None,
            truncated: false,
        });
    }
    let truncated = bytes.len() as u64 > PREVIEW_MAX_BYTES;
    let mut end = bytes.len().min(PREVIEW_MAX_BYTES as usize);
    // 按 UTF-8 边界截断:跳过 continuation byte(0b10xxxxxx)
    while end > 0 && end < bytes.len() && (bytes[end] & 0xC0) == 0x80 {
        end -= 1;
    }
    Ok(FilePreview {
        text: Some(String::from_utf8_lossy(&bytes[..end]).into_owned()),
        truncated,
    })
}

/// 单文件搜索的读取上限,与预览一致(512KB):预览只展示这么多,
/// 超出部分的命中跳不过去,搜了也无法定位
const SEARCH_MAX_FILE_BYTES: u64 = 512 * 1024;
/// 匹配总数 / 命中文件数上限,防止超大仓库的结果拖垮前端渲染
const SEARCH_MAX_MATCHES: u32 = 1000;
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
#[tauri::command]
pub fn search_project_text(
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

/// 写入文本到指定路径(供 Markdown 代码块/表格"下载"按钮走 Tauri save dialog 后调用)。
/// 内容上限为 512KB,目标路径不能为空且父目录必须存在。
/// 写入会创建或覆盖目标文件。
const SAVE_TEXT_MAX_BYTES: usize = 512 * 1024;

#[tauri::command]
pub fn save_text_file(path: String, content: String) -> AppResult<()> {
    if path.trim().is_empty() {
        return Err(AppError::coded(ErrorCode::SavePathRequired, ""));
    }
    if content.len() > SAVE_TEXT_MAX_BYTES {
        return Err(AppError::coded(
            ErrorCode::SaveContentTooLarge,
            SAVE_TEXT_MAX_BYTES.to_string(),
        ));
    }
    // 父目录必须存在
    let p = Path::new(&path);
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            return Err(AppError::coded(
                ErrorCode::SaveParentDirMissing,
                parent.display().to_string(),
            ));
        }
    }
    std::fs::write(p, content.as_bytes())?;
    Ok(())
}

/// 判断 YAML 内容是否为 Docker Compose 格式:顶层含 mapping 类型的 services。
/// 是则返回服务列表(含可访问端口);非法 YAML / 无 services(CI 配置等)返回 None。
fn parse_compose(content: &str) -> Option<Vec<ComposeService>> {
    let yaml = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(content).ok()?;
    let services = yaml.get("services")?.as_mapping()?;
    Some(
        services
            .iter()
            .filter_map(|(k, v)| {
                let mut ports = extract_ports(v);
                ports.sort_by_key(|p| (p.published, p.target));
                ports.dedup();
                Some(ComposeService {
                    name: k.as_str()?.to_string(),
                    ports,
                })
            })
            .collect(),
    )
}

/// 提取服务 ports 中可访问的宿主机端口映射:
/// 短语法 "8080:80" / "127.0.0.1:8080:80" / 长语法 { target, published } 取发布端口;
/// 仅容器端口(宿主机随机分配)、UDP、端口段范围无法确定入口,跳过。
fn extract_ports(service: &serde_yaml_ng::Value) -> Vec<ComposePort> {
    use serde_yaml_ng::Value;
    let Some(list) = service.get("ports").and_then(Value::as_sequence) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|item| match item {
            Value::String(s) => port_from_short(s),
            Value::Mapping(m) => port_from_long(m),
            // 纯数字仅声明容器端口,宿主机端口随机,不可直接访问
            _ => None,
        })
        .collect()
}

/// 短语法:"[IP:]发布端口:容器端口[/协议]"。发布端口恒为末段容器端口前的一段,
/// IPv6 带括号写法([::1]:8080:80)按 ':' 切分后该规律仍成立。
fn port_from_short(s: &str) -> Option<ComposePort> {
    let resolved = resolve_env(s)?;
    let (addr, proto) = resolved
        .split_once('/')
        .map_or((resolved.as_str(), "tcp"), |(a, p)| (a, p));
    if !proto.trim().eq_ignore_ascii_case("tcp") {
        return None; // UDP 等无法通过浏览器访问
    }
    let parts: Vec<&str> = addr.split(':').collect();
    if parts.len() < 2 {
        return None; // 仅容器端口,宿主机端口随机
    }
    Some(ComposePort {
        published: parse_port(parts[parts.len() - 2])?,
        target: parse_port(parts[parts.len() - 1])?,
    })
}

/// 长语法:{ target: 80, published: 8080, protocol: tcp }
fn port_from_long(m: &serde_yaml_ng::Mapping) -> Option<ComposePort> {
    use serde_yaml_ng::Value;
    let proto = m.get("protocol").and_then(Value::as_str).unwrap_or("tcp");
    if !proto.eq_ignore_ascii_case("tcp") {
        return None;
    }
    let parse_field = |key: &str| -> Option<u16> {
        match m.get(key)? {
            Value::Number(n) => n.as_u64().and_then(|v| u16::try_from(v).ok()),
            Value::String(s) => parse_port(&resolve_env(s)?),
            _ => None,
        }
    };
    Some(ComposePort {
        published: parse_field("published")?,
        target: parse_field("target")?,
    })
}

/// 替换 "${VAR:-default}" / "${VAR-default}" 为默认值;
/// 存在无默认值的变量时端口无法确定,整条映射返回 None。
/// 必须先于 ':' 切分执行——默认值语法自身含冒号,直接切分会把变量拆碎。
fn resolve_env(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let end = rest[start..].find('}')? + start;
        let inner = &rest[start + 2..end];
        let default = inner
            .split_once(":-")
            .or_else(|| inner.split_once('-'))
            .map(|(_, d)| d)?;
        out.push_str(default.trim());
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// 解析端口文本:纯数字直接取;端口段范围(8080-8081)无法确定,跳过。
fn parse_port(raw: &str) -> Option<u16> {
    let s = raw.trim();
    if s.contains('-') {
        return None;
    }
    s.parse().ok()
}

/// 是否为可能包含 compose 定义的 YAML 文件(按扩展名粗筛)
fn is_yaml_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("yml") || e.eq_ignore_ascii_case("yaml"))
}

/// compose 判定的廉价粗筛(在完整 YAML 解析之前):
/// - 文件名含 compose(docker-compose.yml / compose.yaml 等惯例命名)直接通过;
/// - 否则只认第 0 列的顶层 `services:` 键——compose 规范要求 services 位于顶层,
///   缩进的同名键是嵌套字段(如 CI 配置里的 services),不算;
/// - 引号包裹键名等罕见写法可能漏过,但只要文件名带 compose 仍会被识别。
fn maybe_compose_file(file_name: &str, content: &str) -> bool {
    if file_name.to_ascii_lowercase().contains("compose") {
        return true;
    }
    content.lines().any(|line| {
        line.strip_prefix("services")
            .is_some_and(|rest| rest.trim_start().starts_with(':'))
    })
}

/// 递归扫描项目内的 Docker Compose 文件(尊重 git 排除规则,按内容识别)。
/// 前端走合并扫描 scan_project_assets,此入口保留给测试
#[cfg_attr(not(test), allow(dead_code))]
pub fn scan_compose_files(path: String) -> AppResult<Vec<ComposeFile>> {
    ensure_dir(&path)?;
    let dir = Path::new(&path);
    Ok(compose_files_from_files(dir, &walk::project_files(dir)))
}

/// 在已遍历的文件清单上提取 compose 文件(供合并扫描复用,避免重复 walk)
pub(crate) fn compose_files_from_files(
    dir: &Path,
    walked: &[std::path::PathBuf],
) -> Vec<ComposeFile> {
    let mut files: Vec<ComposeFile> = walked
        .iter()
        .filter(|rel| is_yaml_file(rel))
        .filter(|rel| {
            std::fs::metadata(dir.join(rel))
                .map(|m| m.len() <= COMPOSE_MAX_BYTES)
                .unwrap_or(false)
        })
        .filter_map(|rel| {
            let content = std::fs::read_to_string(dir.join(rel)).ok()?;
            let file_name = rel.file_name()?.to_string_lossy().into_owned();
            // 廉价粗筛:大项目里 yaml 文件可达上百个,逐个完整解析 YAML 开销可观,
            // 先按文件名/顶层 services 键过滤,只对疑似文件做完整解析
            if !maybe_compose_file(&file_name, &content) {
                return None;
            }
            let services = parse_compose(&content)?;
            Some(ComposeFile {
                path: walk::to_slash(rel),
                file_name,
                services,
            })
        })
        .collect();
    // 根目录文件优先,同级按路径字典序
    files.sort_by(|a, b| (a.path.contains('/'), &a.path).cmp(&(b.path.contains('/'), &b.path)));
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// 建一个带唯一名字的临时目录,返回路径字符串
    fn temp_project_dir(tag: &str) -> String {
        let dir = std::env::temp_dir().join(format!(
            "repomeow-files-{tag}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().to_string()
    }

    #[test]
    fn list_project_files_lists_single_level_and_marks_ignored() {
        let dir = temp_project_dir("list");
        let p = Path::new(&dir);
        fs::write(p.join(".gitignore"), "logs/\n").unwrap();
        fs::create_dir_all(p.join("node_modules/dep")).unwrap();
        fs::write(p.join("node_modules/dep/package.json"), "{}").unwrap();
        fs::create_dir_all(p.join("logs")).unwrap();
        fs::write(p.join("logs/app.log"), "x").unwrap();
        fs::create_dir_all(p.join("empty")).unwrap();
        fs::write(p.join(".env"), "A=1").unwrap();
        fs::write(p.join("src.rs"), "fn main() {}").unwrap();

        // 根层:只有直接子项,文件与目录(含空目录)都返回
        let entries = list_project_files(dir.clone(), None).unwrap();
        let by_path: std::collections::HashMap<&str, &ProjectFileEntry> =
            entries.iter().map(|e| (e.path.as_str(), e)).collect();
        for expected in ["node_modules", "logs", "empty", ".env", ".gitignore", "src.rs"] {
            assert!(by_path.contains_key(expected), "缺少 {expected}");
        }
        assert!(!by_path.keys().any(|k| k.contains('/')), "根层不应出现嵌套路径");
        assert!(by_path["node_modules"].is_dir && by_path["empty"].is_dir);
        assert!(!by_path["src.rs"].is_dir);
        // 被 .gitignore 排除的目录整体标 ignored;未排除的 node_modules 不标
        assert!(by_path["logs"].ignored);
        assert!(!by_path["node_modules"].ignored);
        assert!(!by_path[".env"].ignored && !by_path["src.rs"].ignored);

        // 子目录层
        let entries = list_project_files(dir.clone(), Some("logs".into())).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "logs/app.log");
        assert!(!entries[0].is_dir);
        assert!(entries[0].ignored, "父目录被排除时其内文件同样 ignored");

        // dir 越界 / 指向文件被拒绝
        assert!(list_project_files(dir.clone(), Some("../".into())).is_err());
        assert!(list_project_files(dir.clone(), Some("src.rs".into())).is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_project_files_filters_and_limits() {
        let dir = temp_project_dir("filesearch");
        let p = Path::new(&dir);
        fs::write(p.join("App.vue"), "<template />").unwrap();
        fs::create_dir_all(p.join("src")).unwrap();
        fs::write(p.join("src/app.ts"), "x").unwrap();
        fs::write(p.join("other.txt"), "x").unwrap();
        fs::create_dir_all(p.join("logs")).unwrap();
        fs::write(p.join("logs/app.log"), "x").unwrap();
        fs::write(p.join(".gitignore"), "logs/\n").unwrap();

        // 大小写不敏感;gitignore 排除的不参与;ignored/is_dir 恒为 false
        let r = search_project_files(dir.clone(), "app".into(), Some(50)).unwrap();
        let paths: Vec<&str> = r.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["App.vue", "src/app.ts"]);
        assert!(r.iter().all(|e| !e.ignored && !e.is_dir));

        // 空查询返回空
        assert!(search_project_files(dir.clone(), "  ".into(), None)
            .unwrap()
            .is_empty());
        // limit 生效
        assert_eq!(
            search_project_files(dir.clone(), "app".into(), Some(1))
                .unwrap()
                .len(),
            1
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_file_preview_text_binary_and_escape() {
        let dir = temp_project_dir("preview");
        let p = Path::new(&dir);
        fs::write(p.join("a.txt"), "hello").unwrap();
        // 前 8KB 内带 NUL → 二进制
        fs::write(p.join("b.bin"), b"AB\0CD").unwrap();
        fs::create_dir_all(p.join("sub")).unwrap();
        let outside = temp_project_dir("preview-outside");
        fs::write(Path::new(&outside).join("secret.txt"), "secret").unwrap();

        let r = read_file_preview(dir.clone(), "a.txt".into()).unwrap();
        assert_eq!(r.text.as_deref(), Some("hello"));
        assert!(!r.truncated);

        let r = read_file_preview(dir.clone(), "b.bin".into()).unwrap();
        assert!(r.text.is_none());

        // 目录不是文件
        assert!(read_file_preview(dir.clone(), "sub".into()).is_err());
        // .. 越界读取项目外文件被拒绝
        let escape = format!(
            "../{}/secret.txt",
            Path::new(&outside).file_name().unwrap().to_string_lossy()
        );
        assert!(read_file_preview(dir.clone(), escape).is_err());
        // 不存在的文件
        assert!(read_file_preview(dir.clone(), "nope.txt".into()).is_err());

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn read_file_preview_truncates_on_utf8_boundary() {
        let dir = temp_project_dir("truncate");
        let p = Path::new(&dir);
        // 超出 512KB 的纯 ASCII + 边界处放多字节字符
        let mut content = "a".repeat(PREVIEW_MAX_BYTES as usize);
        content.push('中');
        content.push_str(&"b".repeat(64));
        fs::write(p.join("big.txt"), &content).unwrap();

        let r = read_file_preview(dir.clone(), "big.txt".into()).unwrap();
        assert!(r.truncated);
        let text = r.text.unwrap();
        assert!(text.len() <= PREVIEW_MAX_BYTES as usize);
        assert!(text.chars().all(|c| c == 'a'));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn readme_missing_and_found() {
        let dir = temp_project_dir("readme");
        let p = Path::new(&dir);

        assert!(read_readme(dir.clone()).unwrap().is_none());

        fs::write(p.join("readme.md"), "# Hello").unwrap();
        let r = read_readme(dir.clone()).unwrap().unwrap();
        assert_eq!(r.file_name, "readme.md");
        assert_eq!(r.content, "# Hello");

        // 优先级:README.md 高于 readme.md。
        // 注意先删 readme.md:Windows/macOS 大小写不敏感文件系统上,
        // 直接写 README.md 会覆盖同名文件但保留原有目录项大小写。
        fs::remove_file(p.join("readme.md")).unwrap();
        fs::write(p.join("README.md"), "# Priority").unwrap();
        let r = read_readme(dir.clone()).unwrap().unwrap();
        assert_eq!(r.file_name, "README.md");
        assert_eq!(r.content, "# Priority");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn readme_rejects_missing_dir() {
        assert!(matches!(read_readme("D:/no/such/dir-xyz".into()),
                Err(ref e) if e.is_code(crate::error::ErrorCode::InvalidPath)));
    }

    #[test]
    fn compose_scan_by_content_not_name() {
        let dir = temp_project_dir("compose-content");
        let p = Path::new(&dir);

        // 非标准文件名,但内容是 compose 格式 -> 识别
        fs::write(p.join("app.yml"), "services:\n  web:\n    image: nginx\n").unwrap();
        // 标准文件名但无 services -> 不识别
        fs::write(p.join("docker-compose.yml"), "name: demo\n").unwrap();
        // CI 配置(yml 但非 compose)-> 不识别
        fs::write(p.join("ci.yaml"), "on: push\njobs: {}\n").unwrap();
        // 非法 YAML -> 不识别
        fs::write(p.join("broken.yml"), "services: [not a map").unwrap();
        // 非 yml 文件不参与
        fs::write(p.join("services.txt"), "services:\n  x: {}\n").unwrap();

        let files = scan_compose_files(dir.clone()).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "app.yml");
        assert_eq!(files[0].file_name, "app.yml");
        assert_eq!(files[0].services[0].name, "web");
        assert!(files[0].services[0].ports.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compose_scan_nested_and_gitignored() {
        let dir = temp_project_dir("compose-nested");
        let p = Path::new(&dir);

        // 嵌套子目录中的 compose
        fs::create_dir_all(p.join("deploy/prod")).unwrap();
        fs::write(
            p.join("deploy/prod/stack.yaml"),
            "services:\n  api:\n    build: .\n  db:\n    image: postgres:16\n",
        )
        .unwrap();
        // 被 .gitignore 排除的目录不扫描
        fs::create_dir_all(p.join("ignored")).unwrap();
        fs::write(p.join("ignored/svc.yml"), "services:\n  x: {}\n").unwrap();
        fs::write(p.join(".gitignore"), "ignored/\n").unwrap();

        let files = scan_compose_files(dir.clone()).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "deploy/prod/stack.yaml");
        let names: Vec<&str> = files[0].services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["api", "db"]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compose_scan_root_first_ordering() {
        let dir = temp_project_dir("compose-order");
        let p = Path::new(&dir);

        fs::create_dir_all(p.join("abc")).unwrap();
        fs::write(p.join("abc/x.yml"), "services:\n  a: {}\n").unwrap();
        // 根目录文件名字典序更大,但仍应排在前面
        fs::write(p.join("z.yml"), "services:\n  z: {}\n").unwrap();
        fs::write(p.join("a.yml"), "services:\n  a: {}\n").unwrap();

        let files = scan_compose_files(dir.clone()).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["a.yml", "z.yml", "abc/x.yml"]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn maybe_compose_prefilter() {
        // 文件名含 compose 直接通过(哪怕内容暂未嗅探到顶层 services)
        assert!(maybe_compose_file("docker-compose.yml", "name: demo\n"));
        assert!(maybe_compose_file("COMPOSE.YAML", "x: 1\n"));
        // 顶层 services 键:标准写法、键后空格、CRLF 行尾都认
        assert!(maybe_compose_file("app.yml", "services:\n  web: {}\n"));
        assert!(maybe_compose_file("app.yml", "services :\n  web: {}\n"));
        assert!(maybe_compose_file("app.yml", "---\nservices:\r\n  web: {}\n"));
        // 缩进的嵌套 services(如 CI 配置)不算顶层键
        assert!(!maybe_compose_file("ci.yaml", "jobs:\n  build:\n    services:\n      db: {}\n"));
        // 注释与普通键不误判
        assert!(!maybe_compose_file("a.yml", "# services:\nx: 1\n"));
        assert!(!maybe_compose_file("a.yml", "serviceName: web\n"));
    }

    #[test]
    fn save_text_file_writes_and_validates() {
        let dir = temp_project_dir("save-text");
        let p = Path::new(&dir);

        // 正常写入
        let target = p.join("out.csv");
        save_text_file(target.to_string_lossy().into_owned(), "a,b\n1,2\n".into()).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "a,b\n1,2\n");

        // 空路径报错
        assert!(matches!(
            save_text_file("".into(), "x".into()),
            Err(ref e) if e.is_code(ErrorCode::SavePathRequired)
        ));

        // 父目录不存在报错
        let bad = p.join("missing/out.txt");
        assert!(matches!(
            save_text_file(bad.to_string_lossy().into_owned(), "x".into()),
            Err(ref e) if e.is_code(ErrorCode::SaveParentDirMissing)
        ));

        // 超大内容报错
        let huge = "x".repeat(SAVE_TEXT_MAX_BYTES + 1);
        let target2 = p.join("huge.txt");
        assert!(matches!(
            save_text_file(target2.to_string_lossy().into_owned(), huge),
            Err(ref e) if e.is_code(ErrorCode::SaveContentTooLarge)
        ));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compose_extracts_accessible_ports() {
        let content = r#"
services:
  web:
    image: nginx
    ports:
      - "8080:80"
      - "127.0.0.1:9090:90/tcp"
      - "53:53/udp"
      - "3000"
      - "8081-8082:81-82"
      - target: 443
        published: 8443
        protocol: tcp
      - target: 541
        published: 541
        protocol: udp
  api:
    image: app
    ports:
      - "${API_PORT:-8000}:8000"
  db:
    image: postgres
"#;
        let services = parse_compose(content).unwrap();
        assert_eq!(services.len(), 3);
        // web: 8080/9090/8443 可访问;udp、仅容器端口、端口段范围跳过;去重升序
        let port = |published: u16, target: u16| ComposePort { published, target };
        assert_eq!(
            services[0].ports,
            vec![port(8080, 80), port(8443, 443), port(9090, 90)]
        );
        assert_eq!(services[1].ports, vec![port(8000, 8000)]);
        assert!(services[2].ports.is_empty());
    }

    // ── 全文搜索 ────────────────────────────────────────────────────────────

    /// 便捷:断言某文件的命中行号集合
    fn hit_lines(outcome: &TextSearchOutcome, path: &str) -> Vec<u32> {
        outcome
            .hits
            .iter()
            .find(|h| h.path == path)
            .unwrap_or_else(|| panic!("缺少命中文件 {path}"))
            .lines
            .iter()
            .map(|l| l.line)
            .collect()
    }

    #[test]
    fn search_project_text_modes() {
        let dir = temp_project_dir("search-modes");
        let p = Path::new(&dir);
        // 默认大小写不敏感:第 1、2 行命中
        fs::write(p.join("a.txt"), "Hello world\nhello WORLD\nbye\n").unwrap();

        let r = search_project_text(dir.clone(), "hello".into(), false, false, false, "".into(), "".into()).unwrap();
        assert_eq!(hit_lines(&r, "a.txt"), vec![1, 2]);
        assert_eq!(r.hits[0].count, 2);
        assert!(!r.truncated);

        // 大小写敏感:仅第 2 行(hello WORLD)
        let r = search_project_text(dir.clone(), "hello".into(), true, false, false, "".into(), "".into()).unwrap();
        assert_eq!(hit_lines(&r, "a.txt"), vec![2]);

        // 全字匹配:"world" 不命中 "worldwide"
        fs::write(p.join("b.txt"), "world\nworldwide\n").unwrap();
        let r = search_project_text(dir.clone(), "world".into(), false, true, false, "".into(), "".into()).unwrap();
        assert_eq!(hit_lines(&r, "b.txt"), vec![1]);

        // 正则模式 + 全字组合:\b(?:c.t)\b
        fs::write(p.join("c.txt"), "cat cut\nconcat\n").unwrap();
        let r = search_project_text(dir.clone(), "c.t".into(), false, true, true, "".into(), "".into()).unwrap();
        assert_eq!(hit_lines(&r, "c.txt"), vec![1]);

        // 同行多次匹配计入 count,行只出现一次
        fs::write(p.join("d.txt"), "ab ab ab\n").unwrap();
        let r = search_project_text(dir.clone(), "ab".into(), false, false, false, "".into(), "".into()).unwrap();
        assert_eq!(hit_lines(&r, "d.txt"), vec![1]);
        let d = r.hits.iter().find(|h| h.path == "d.txt").unwrap();
        assert_eq!(d.count, 3);
        assert_eq!(d.lines.len(), 1);

        // 空查询返回空结果
        let r = search_project_text(dir.clone(), "  ".into(), false, false, false, "".into(), "".into()).unwrap();
        assert!(r.hits.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_project_text_scope_and_binary() {
        let dir = temp_project_dir("search-scope");
        let p = Path::new(&dir);
        fs::write(p.join(".env"), "SECRET_TOKEN=x\n").unwrap();
        fs::write(p.join("b.bin"), b"to\0ken").unwrap();
        fs::create_dir_all(p.join("node_modules/dep")).unwrap();
        fs::write(p.join("node_modules/dep/token.txt"), "token\n").unwrap();
        fs::create_dir_all(p.join("logs")).unwrap();
        fs::write(p.join("logs/token.log"), "token\n").unwrap();
        fs::write(p.join(".gitignore"), "logs/\n").unwrap();
        fs::create_dir_all(p.join(".git")).unwrap();
        fs::write(p.join(".git/token"), "token\n").unwrap();

        let r = search_project_text(dir.clone(), "token".into(), false, false, false, "".into(), "".into()).unwrap();
        let paths: Vec<&str> = r.hits.iter().map(|h| h.path.as_str()).collect();
        // 隐藏文件可搜;二进制、node_modules、git 忽略目录、.git 内部跳过
        assert_eq!(paths, vec![".env"]);
        // .gitignore 自身无命中,不出现在结果里

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_project_text_long_line_window() {
        let dir = temp_project_dir("search-window");
        let p = Path::new(&dir);
        let mut line = "x".repeat(3000);
        line.push_str("NEEDLE");
        line.push_str(&"y".repeat(100));
        fs::write(p.join("min.js"), &line).unwrap();

        let r = search_project_text(dir.clone(), "NEEDLE".into(), false, false, false, "".into(), "".into()).unwrap();
        let text = &r.hits[0].lines[0].text;
        assert!(text.starts_with('…'), "超长行窗口应带前省略号:{text}");
        assert!(text.ends_with('…'), "超长行窗口应带后省略号:{text}");
        assert!(text.contains("NEEDLE"));
        assert!(text.len() < line.len());

        // 短行原样返回
        fs::write(p.join("short.txt"), "NEEDLE here\n").unwrap();
        let r = search_project_text(dir.clone(), "NEEDLE".into(), false, false, false, "".into(), "".into()).unwrap();
        let short = r.hits.iter().find(|h| h.path == "short.txt").unwrap();
        assert_eq!(short.lines[0].text, "NEEDLE here");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_project_text_caps_truncated() {
        let dir = temp_project_dir("search-caps");
        let p = Path::new(&dir);
        // 单文件 1100 个匹配:达到 1000 上限后停止累计并置 truncated
        let content = (0..1100).map(|i| format!("hit {i}")).collect::<Vec<_>>().join("\n");
        fs::write(p.join("many.txt"), &content).unwrap();

        let r = search_project_text(dir.clone(), "hit".into(), false, false, false, "".into(), "".into()).unwrap();
        assert!(r.truncated);
        let hit = &r.hits[0];
        assert_eq!(hit.count, SEARCH_MAX_MATCHES);
        assert_eq!(hit.lines.len(), SEARCH_MAX_MATCHES as usize);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_project_text_filters_by_include_and_exclude_globs() {
        let dir = temp_project_dir("search-globs");
        let p = Path::new(&dir);
        fs::create_dir_all(p.join("src/nested")).unwrap();
        fs::create_dir_all(p.join("tests")).unwrap();
        fs::create_dir_all(p.join("dist")).unwrap();
        fs::write(p.join("src/main.ts"), "needle\n").unwrap();
        fs::write(p.join("src/nested/worker.ts"), "needle\n").unwrap();
        fs::write(p.join("tests/main.ts"), "needle\n").unwrap();
        fs::write(p.join("dist/bundle.ts"), "needle\n").unwrap();
        fs::write(p.join("README.md"), "needle\n").unwrap();

        let r = search_project_text(
            dir.clone(),
            "needle".into(),
            false,
            false,
            false,
            "src/**/*.ts, README.md".into(),
            "src/nested/**".into(),
        )
        .unwrap();
        let paths: Vec<&str> = r.hits.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(paths, vec!["README.md", "src/main.ts"]);

        // 无斜杠模式按 VS Code Search view 语义匹配任意层级;
        // ./ 只匹配工作区根层文件。
        let r = search_project_text(
            dir.clone(),
            "needle".into(),
            false,
            false,
            false,
            "*.ts".into(),
            "".into(),
        )
        .unwrap();
        let paths: Vec<&str> = r.hits.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(paths, vec!["dist/bundle.ts", "src/main.ts", "src/nested/worker.ts", "tests/main.ts"]);

        let r = search_project_text(
            dir.clone(),
            "needle".into(),
            false,
            false,
            false,
            "./*.md".into(),
            "".into(),
        )
        .unwrap();
        assert_eq!(r.hits.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(), vec!["README.md"]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_project_text_rejects_invalid_glob() {
        let dir = temp_project_dir("search-badglob");
        assert!(matches!(
            search_project_text(
                dir.clone(),
                "needle".into(),
                false,
                false,
                false,
                "src/[".into(),
                "".into(),
            ),
            Err(ref e) if e.is_code(ErrorCode::SearchInvalidGlob)
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_project_text_invalid_regex() {
        let dir = temp_project_dir("search-badre");
        assert!(matches!(
            search_project_text(dir.clone(), "([".into(), false, false, true, "".into(), "".into()),
            Err(ref e) if e.is_code(ErrorCode::SearchInvalidRegex)
        ));
        let _ = fs::remove_dir_all(&dir);
    }
}
