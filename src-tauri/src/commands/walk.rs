use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use ignore::WalkBuilder;
use notify::Watcher;

/// 递归列出项目内未被 git 排除的文件,返回相对 root 的路径。
///
/// 规则:
/// - 尊重 .gitignore / .ignore / 全局 gitignore 及父目录 gitignore;
///   `require_git(false)` 表示即使目录不是 git 仓库,只要存在 .gitignore 就生效。
/// - 默认跳过隐藏条目(.git、.github 等点开头目录/文件)。
/// - 无条件跳过 node_modules:未被 gitignore 时(如非 git 项目)扫描它既慢又吵,
///   其内部的 package.json / yml 对本工具没有价值。
///
/// 并行遍历:大项目(数万文件)单线程做 gitignore 匹配要数百毫秒,
/// build_parallel 按核数分发目录缩短冷扫描;输出顺序不确定,最后统一排序保证确定性。
pub fn project_files(root: &Path) -> Vec<PathBuf> {
    let (tx, rx) = std::sync::mpsc::channel::<PathBuf>();
    WalkBuilder::new(root)
        .require_git(false)
        .filter_entry(|e| {
            // 目录(或文件)名为 node_modules 时不再深入
            e.file_name() != "node_modules"
        })
        .build_parallel()
        .run(|| {
            let tx = tx.clone();
            Box::new(move |entry| {
                if let Ok(e) = entry {
                    if e.file_type().is_some_and(|t| t.is_file()) {
                        let _ = tx.send(e.into_path());
                    }
                }
                ignore::WalkState::Continue
            })
        });
    drop(tx);
    let mut files: Vec<PathBuf> = rx
        .into_iter()
        .filter_map(|p| p.strip_prefix(root).ok().map(PathBuf::from))
        .collect();
    files.sort();
    files
}

/// 全文搜索的遍历范围(search_project_text):尊重 .gitignore / .ignore
/// (与 VSCode 默认搜索一致),但保留隐藏文件(.env / .gitignore 本身可搜),
/// 并跳过 node_modules 与 .git 内部——node_modules 全文搜索既慢又几乎全是噪音。
pub fn searchable_files(root: &Path) -> Vec<PathBuf> {
    let (tx, rx) = std::sync::mpsc::channel::<PathBuf>();
    WalkBuilder::new(root)
        .require_git(false)
        .hidden(false)
        .filter_entry(|e| e.file_name() != ".git" && e.file_name() != "node_modules")
        .build_parallel()
        .run(|| {
            let tx = tx.clone();
            Box::new(move |entry| {
                if let Ok(e) = entry {
                    if e.file_type().is_some_and(|t| t.is_file()) {
                        let _ = tx.send(e.into_path());
                    }
                }
                ignore::WalkState::Continue
            })
        });
    drop(tx);
    let mut files: Vec<PathBuf> = rx
        .into_iter()
        .filter_map(|p| p.strip_prefix(root).ok().map(PathBuf::from))
        .collect();
    files.sort();
    files
}

/// 相对路径转 '/' 分隔字符串(Windows 下 '\' 归一化)。
/// 统一辅助在 crate::path_util,此处保留旧名兼容既有调用
pub fn to_slash(path: &Path) -> String {
    crate::path_util::to_forward_slash(path)
}

/// 单层目录子项(dir_entries):相对 root 的路径 + 是否目录 + 是否被 ignore 排除
pub struct DirEntry {
    pub path: PathBuf,
    pub is_dir: bool,
    pub ignored: bool,
}

/// 列出 rel_dir(相对 root,空路径表示根目录)的直接子项,供文件树逐层懒加载。
///
/// 用「尊重 ignore 规则」与「不看 ignore 规则」两次 max_depth(1) 遍历取差集标 ignored
/// (ignore crate 的 parents 默认开启,直接遍历子目录时祖先 .gitignore 仍生效);
/// 文件与目录都收集——空目录因此可见。浅层遍历成本低,无需并行与缓存。
pub fn dir_entries(root: &Path, rel_dir: &Path) -> Vec<DirEntry> {
    let dir = if rel_dir.as_os_str().is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel_dir)
    };
    // gitignore 的目录规则(如 logs/)只匹配目录本身,靠遍历时剪枝生效;
    // 直接以被排除目录为起点遍历时其内条目不命中任何规则,
    // 需检测目录自身是否被排除,命中则按 git 语义让全部子项继承 ignored。
    // 根层(空 / "." / 父目录无意义)不需要走该检测:其 file_name 返 None
    // 会让 name 变空串,shallow_entries 不可能命中空名,导致整根子项被错标 ignored
    let dir_ignored = !is_root_level(rel_dir) && {
        let parent = rel_dir.parent().unwrap_or_else(|| Path::new(""));
        let parent_dir = if parent.as_os_str().is_empty() {
            root.to_path_buf()
        } else {
            root.join(parent)
        };
        // file_name 返 None(Path::new(".") 等)时按非排除处理,
        // 调用方不该传这类路径,但保险起见不再错判 ignored
        let name = rel_dir.file_name().unwrap_or_default();
        if name.is_empty() {
            false
        } else {
            !shallow_entries(&parent_dir, true)
                .iter()
                .any(|(p, _)| p.as_os_str() == name)
        }
    };
    let unignored: std::collections::HashSet<_> = shallow_entries(&dir, true)
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    let mut entries: Vec<DirEntry> = shallow_entries(&dir, false)
        .into_iter()
        .map(|(rel, is_dir)| {
            let path = if is_root_level(rel_dir) {
                rel.clone()
            } else {
                rel_dir.join(&rel)
            };
            DirEntry {
                ignored: dir_ignored || !unignored.contains(&rel),
                is_dir,
                path,
            }
        })
        .collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
}

/// rel_dir 表示工作区根层:空串或 `.`(后者经 `as_os_str().is_empty()` 也按空处理之外,
/// 调用方可能传 `Path::new(".")` 而逃过 is_empty 判定)
fn is_root_level(rel_dir: &Path) -> bool {
    rel_dir.as_os_str().is_empty() || rel_dir == Path::new(".")
}

/// max_depth(1) 收集 dir 的直接子项,返回相对 dir 的路径与是否目录。
/// respect_ignore 区分两次遍历:true 尊重 .gitignore/.ignore(含隐藏文件),
/// false 全量(不看任何 ignore 规则);两者都只跳过 .git 目录
fn shallow_entries(dir: &Path, respect_ignore: bool) -> Vec<(PathBuf, bool)> {
    let mut builder = WalkBuilder::new(dir);
    builder
        .require_git(false)
        .hidden(false)
        .max_depth(Some(1))
        .filter_entry(|e| e.file_name() != ".git");
    if !respect_ignore {
        builder
            .ignore(false)
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .parents(false);
    }
    builder
        .build()
        .filter_map(|entry| entry.ok())
        .filter(|e| e.depth() > 0)
        .filter_map(|e| {
            let ft = e.file_type()?;
            // file_type 不跟随符号链接:指向目录的软链(如 pnpm 的 node_modules)
            // 按目标类型归类,悬空链接不收集;std::fs::metadata 会跟随链接
            let is_dir = if ft.is_symlink() {
                match std::fs::metadata(e.path()) {
                    Ok(meta) if meta.is_dir() => true,
                    Ok(meta) if meta.is_file() => false,
                    _ => return None,
                }
            } else if ft.is_dir() {
                true
            } else if ft.is_file() {
                false
            } else {
                return None; // socket/设备文件等不收集
            };
            e.path()
                .strip_prefix(dir)
                .ok()
                .map(|p| (PathBuf::from(p), is_dir))
        })
        .collect()
}

/// 文件名搜索(search_project_files):在未被 .gitignore/.ignore 排除的文件里
/// 按相对路径(小写化后,needle 需已小写)做子串匹配,命中 limit 条即停止遍历——
/// 单线程顺序走,大项目避免为搜索框全量扫描;遍历口径:尊重 .gitignore/.ignore
/// (require_git(false) 非 git 项目同样生效)、含隐藏文件、node_modules 是否出现
/// 由项目 ignore 规则决定、跳过 .git
pub fn search_file_paths(root: &Path, needle_lower: &str, limit: usize) -> Vec<PathBuf> {
    let mut matches: Vec<PathBuf> = WalkBuilder::new(root)
        .require_git(false)
        .hidden(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build()
        .filter_map(|entry| entry.ok())
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|e| e.path().strip_prefix(root).ok().map(PathBuf::from))
        .filter(|rel| to_slash(rel).to_lowercase().contains(needle_lower))
        .take(limit)
        .collect();
    matches.sort();
    matches
}

// ── walk 结果缓存(notify 变更即时失效 + TTL 兜底) ─────────────────────────

/// 缓存 TTL 仅作兜底:正常路径下文件监听会在 package.json / yaml / gitignore
/// 变更时即时删掉缓存条目,TTL 只覆盖监听安装失败、事件丢失(缓冲区溢出等)
/// 的场景,故可以从 30s 放宽到 5 分钟,大项目反复进出详情页不再周期性重扫
const WALK_CACHE_TTL: Duration = Duration::from_secs(300);

/// 缓存条目上限:单个大项目的文件清单可达数十 MB,超限直接清空重建,
/// 避免多项目缓存堆积占用内存
const WALK_CACHE_MAX_ENTRIES: usize = 8;

struct CachedWalk {
    files: Arc<Vec<PathBuf>>,
    at: Instant,
}

static WALK_CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedWalk>>> = OnceLock::new();

/// 已缓存根目录的递归文件监听器。回调里只做缓存失效(map.remove),
/// 真正的重扫延迟到下次请求时按需进行;锁顺序:回调只拿 WALK_CACHE,
/// 其余路径先 WALK_CACHE 后 WALK_WATCHERS,无循环等待
static WALK_WATCHERS: OnceLock<Mutex<HashMap<PathBuf, notify::RecommendedWatcher>>> =
    OnceLock::new();

/// 带缓存的 project_files:命中且未过期直接共享同一份结果(Arc,不复制)。
/// 详情页资产扫描等高频只读场景使用;需要保证新鲜的调用方(如测试)用 project_files。
/// key 归一化:同一目录的正反斜杠/尾斜杠写法共享缓存与监听器,不会重复装 watcher
pub fn project_files_cached(root: &Path) -> Arc<Vec<PathBuf>> {
    let root = &crate::path_util::clean(root);
    let cache = WALK_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let map = cache.lock().unwrap();
        if let Some(entry) = map.get(root) {
            if entry.at.elapsed() < WALK_CACHE_TTL {
                return entry.files.clone();
            }
        }
    }
    let files = Arc::new(project_files(root));
    let mut map = cache.lock().unwrap();
    // 插入前顺带清掉已过期条目;仍超限则整体清空(实现简单,重建成本低)
    map.retain(|_, e| e.at.elapsed() < WALK_CACHE_TTL);
    if map.len() >= WALK_CACHE_MAX_ENTRIES {
        map.clear();
    }
    map.insert(
        root.to_path_buf(),
        CachedWalk {
            files: files.clone(),
            at: Instant::now(),
        },
    );
    // 撤掉已不在缓存里的根目录监听,避免 watcher 无限堆积
    WALK_WATCHERS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap()
        .retain(|dir, _| map.contains_key(dir));
    drop(map);
    ensure_watcher(root);
    files
}

/// 项目路径变更(重定向/移动/删除)后清理该路径的缓存条目与文件监听器。
/// 否则旧路径 watcher 要等 TTL 才被回收,期间旧目录的变更还会无效地触发失效回调
pub fn invalidate(root: &Path) {
    let root = &crate::path_util::clean(root);
    if let Some(cache) = WALK_CACHE.get() {
        cache.lock().unwrap().remove(root);
    }
    if let Some(watchers) = WALK_WATCHERS.get() {
        // watcher 随条目移除 drop,递归监听随之注销
        watchers.lock().unwrap().remove(root);
    }
}

/// 为已缓存的根目录安装递归文件监听(幂等)。
/// 监听安装失败(权限、平台不支持等)时静默降级为纯 TTL 失效。
/// 闭包不再持有根目录的 .gitignore/.ignore:规则文件被改后旧规则会持续
/// 错过滤事件,漏掉应当让缓存失效的资产变更(例:`target/` 加入 .gitignore
/// 后,旧规则不会排除它,target 下的 package.json 写入会跳过 invalidate);
/// 改为只按 is_asset_relevant 判定,代价是构建目录(target/ 等)的写入
/// 偶尔会触发一次额外重扫(忽略规则改在重扫时自然生效)
fn ensure_watcher(root: &Path) {
    let watchers = WALK_WATCHERS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = watchers.lock().unwrap();
    if map.contains_key(root) {
        return;
    }
    let root_owned = root.to_path_buf();
    let Ok(mut watcher) = notify::recommended_watcher(move |res| {
        let Ok(event) = res else { return };
        if !event_affects_assets(&event) {
            return;
        }
        if let Some(cache) = WALK_CACHE.get() {
            cache.lock().unwrap().remove(&root_owned);
        }
    }) else {
        eprintln!(
            "[walk] 创建文件监听器失败({}),降级为纯 TTL 失效",
            root.display()
        );
        return;
    };
    match watcher.watch(root, notify::RecursiveMode::Recursive) {
        Ok(()) => {
            map.insert(root.to_path_buf(), watcher);
        }
        Err(e) => {
            eprintln!(
                "[walk] 监听安装失败({}): {e},降级为纯 TTL 失效",
                root.display()
            );
        }
    }
}

/// 路径是否参与资产提取:仅 package.json、yaml 与 ignore 规则文件
/// (gitignore 变更会改变遍历集合本身);node_modules 遍历时被跳过,其内部变更同样无关
fn is_asset_relevant(path: &Path) -> bool {
    if path.components().any(|c| c.as_os_str() == "node_modules") {
        return false;
    }
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if name == "package.json" || name == ".gitignore" || name == ".ignore" {
        return true;
    }
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("yml") || e.eq_ignore_ascii_case("yaml"))
}

/// 文件事件是否可能改变扫描结果:无法判断时(paths 为空,如事件缓冲区溢出)
/// 保守按失效处理;否则只要任一相关路径命中即认为应 invalidate
fn event_affects_assets(event: &notify::Event) -> bool {
    if event.paths.is_empty() {
        return true;
    }
    event.paths.iter().any(|p| is_asset_relevant(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "repomeow-walk-{tag}-{}-{}",
            std::process::id(),
            crate::time_util::now_ts_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn respects_gitignore_and_skips_node_modules() {
        let dir = temp_dir("gitignore");
        fs::write(dir.join("keep.yml"), "a: 1").unwrap();
        fs::create_dir_all(dir.join("logs")).unwrap();
        fs::write(dir.join("logs/app.yml"), "a: 1").unwrap();
        fs::write(dir.join(".gitignore"), "logs/\n").unwrap();

        let files = project_files(&dir);
        let names: Vec<String> = files.iter().map(|p| to_slash(p)).collect();
        assert!(names.contains(&"keep.yml".to_string()));
        assert!(!names.iter().any(|n| n.starts_with("logs/")));

        // node_modules 即使未被 gitignore 也跳过
        fs::remove_file(dir.join(".gitignore")).unwrap();
        fs::create_dir_all(dir.join("node_modules/dep")).unwrap();
        fs::write(dir.join("node_modules/dep/package.json"), "{}").unwrap();
        let files = project_files(&dir);
        assert!(!files
            .iter()
            .any(|p| to_slash(p).starts_with("node_modules/")));
        // .gitignore 文件本身是隐藏文件,不出现在结果里
        assert!(!files.iter().any(|p| to_slash(p) == ".gitignore"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn skips_hidden_dirs() {
        let dir = temp_dir("hidden");
        fs::create_dir_all(dir.join(".github/workflows")).unwrap();
        fs::write(dir.join(".github/workflows/ci.yml"), "on: push").unwrap();
        fs::write(dir.join("app.yml"), "a: 1").unwrap();

        let files = project_files(&dir);
        let names: Vec<String> = files.iter().map(|p| to_slash(p)).collect();
        assert_eq!(names, vec!["app.yml"]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dir_entries_lists_single_level_with_dirs_and_ignored() {
        let dir = temp_dir("dirents");
        fs::write(dir.join(".gitignore"), "logs/\n").unwrap();
        fs::create_dir_all(dir.join("src/nested")).unwrap();
        fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(dir.join("src/nested/deep.txt"), "x").unwrap();
        fs::create_dir_all(dir.join("empty")).unwrap();
        fs::create_dir_all(dir.join("logs")).unwrap();
        fs::write(dir.join("logs/app.log"), "x").unwrap();
        fs::write(dir.join(".env"), "A=1").unwrap();
        fs::create_dir_all(dir.join(".git/objects")).unwrap();
        fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main").unwrap();

        // 根层:只一层(不出现 src/main.rs)、含目录与空目录、跳过 .git
        let entries = dir_entries(&dir, Path::new(""));
        let by_path: std::collections::HashMap<String, &DirEntry> =
            entries.iter().map(|e| (to_slash(&e.path), e)).collect();
        for expected in ["src", "empty", "logs", ".env", ".gitignore"] {
            assert!(by_path.contains_key(expected), "缺少 {expected}");
        }
        assert!(!by_path.keys().any(|p| p.contains('/')), "只应有一层");
        assert!(!by_path
            .keys()
            .any(|p| p == ".git" || p.starts_with(".git/")));
        assert!(by_path["src"].is_dir && by_path["empty"].is_dir);
        assert!(!by_path[".env"].is_dir);
        // 被 .gitignore 排除的目录整体标 ignored,其余不标
        assert!(by_path["logs"].ignored);
        assert!(!by_path["src"].ignored && !by_path[".env"].ignored);

        // 子目录层:路径带前缀;祖先 .gitignore 仍生效(遍历 logs 时自身规则不影响,
        // 换 src 验证嵌套忽略)
        fs::write(dir.join("src/.gitignore"), "nested/\n").unwrap();
        let entries = dir_entries(&dir, Path::new("src"));
        let by_path: std::collections::HashMap<String, &DirEntry> =
            entries.iter().map(|e| (to_slash(&e.path), e)).collect();
        assert!(by_path.contains_key("src/main.rs"));
        assert!(by_path.contains_key("src/nested"));
        assert!(
            by_path["src/nested"].ignored,
            "子目录自己的 .gitignore 应生效"
        );

        // 父目录被排除时全部子项继承 ignored(git 语义:规则靠剪枝生效,
        // 直接以被排除目录为起点遍历时子项自身不命中规则)
        let entries = dir_entries(&dir, Path::new("logs"));
        assert_eq!(entries.len(), 1);
        assert!(entries[0].ignored, "被排除目录内的条目应继承 ignored");

        // 带路径的规则(build/output.txt):直接遍历 build 时祖先 .gitignore 仍按完整相对路径匹配
        fs::write(dir.join(".gitignore"), "logs/\nbuild/output.txt\n").unwrap();
        fs::create_dir_all(dir.join("build")).unwrap();
        fs::write(dir.join("build/output.txt"), "x").unwrap();
        fs::write(dir.join("build/keep.txt"), "x").unwrap();
        let entries = dir_entries(&dir, Path::new("build"));
        let by_path: std::collections::HashMap<String, &DirEntry> =
            entries.iter().map(|e| (to_slash(&e.path), e)).collect();
        assert!(by_path["build/output.txt"].ignored);
        assert!(!by_path["build/keep.txt"].ignored);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dir_entries_follows_symlinks_to_dir_and_file() {
        let dir = temp_dir("dirents-symlink");
        fs::create_dir_all(dir.join("real_dir")).unwrap();
        fs::write(dir.join("real_dir/inside.txt"), "x").unwrap();
        fs::write(dir.join("real.txt"), "x").unwrap();

        let link_dir = dir.join("linked_dir");
        let link_file = dir.join("linked.txt");
        let dangling = dir.join("dangling");
        #[cfg(unix)]
        let results = [
            std::os::unix::fs::symlink(dir.join("real_dir"), &link_dir),
            std::os::unix::fs::symlink(dir.join("real.txt"), &link_file),
            std::os::unix::fs::symlink(dir.join("missing"), &dangling),
        ];
        #[cfg(windows)]
        let results = [
            std::os::windows::fs::symlink_dir(dir.join("real_dir"), &link_dir),
            std::os::windows::fs::symlink_file(dir.join("real.txt"), &link_file),
            std::os::windows::fs::symlink_dir(dir.join("missing"), &dangling),
        ];
        if results.iter().any(|r| r.is_err()) {
            // Windows 无开发者模式/权限时无法创建符号链接,跳过
            let _ = fs::remove_dir_all(&dir);
            return;
        }

        // 软链按目标类型归类:指向目录的是目录、指向文件的是文件,悬空链接不出现
        let entries = dir_entries(&dir, Path::new(""));
        let by_path: std::collections::HashMap<String, &DirEntry> =
            entries.iter().map(|e| (to_slash(&e.path), e)).collect();
        assert!(by_path["linked_dir"].is_dir, "指向目录的软链应为目录");
        assert!(!by_path["linked.txt"].is_dir, "指向文件的软链应为文件");
        assert!(!by_path.contains_key("dangling"), "悬空软链不应出现");

        // 展开软链目录能列出目标内容(pnpm node_modules 场景)
        let entries = dir_entries(&dir, Path::new("linked_dir"));
        assert!(
            entries
                .iter()
                .any(|e| e.path == Path::new("linked_dir/inside.txt")),
            "软链目录的子项应可列出"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_file_paths_matches_case_insensitive_and_respects_gitignore() {
        let dir = temp_dir("searchpaths");
        fs::write(dir.join("ReadMe.md"), "# x").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/README.md"), "# y").unwrap();
        fs::write(dir.join("other.txt"), "x").unwrap();
        fs::create_dir_all(dir.join("logs")).unwrap();
        fs::write(dir.join("logs/readme.log"), "x").unwrap();
        fs::write(dir.join(".gitignore"), "logs/\n").unwrap();
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join(".git/readme"), "x").unwrap();

        // 大小写不敏感子串匹配;被 gitignore 排除的与 .git 内部不参与
        let matches = search_file_paths(&dir, "readme", 50);
        let names: Vec<String> = matches.iter().map(|p| to_slash(p)).collect();
        assert_eq!(names, vec!["ReadMe.md", "src/README.md"]);

        // limit 提前截断
        let matches = search_file_paths(&dir, "readme", 1);
        assert_eq!(matches.len(), 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn asset_relevance_matches_scan_inputs() {
        assert!(is_asset_relevant(Path::new("/p/package.json")));
        assert!(is_asset_relevant(Path::new("/p/a/b/docker-compose.yml")));
        assert!(is_asset_relevant(Path::new("/p/x.YAML")));
        // ignore 规则变更会改变遍历集合本身,也算相关
        assert!(is_asset_relevant(Path::new("/p/.gitignore")));
        assert!(is_asset_relevant(Path::new("/p/sub/.ignore")));
        assert!(!is_asset_relevant(Path::new("/p/README.md")));
        assert!(!is_asset_relevant(Path::new("/p/src/main.rs")));
        // node_modules 被遍历跳过,内部 package.json 变更无关
        assert!(!is_asset_relevant(Path::new(
            "/p/node_modules/dep/package.json"
        )));
    }

    #[test]
    fn watcher_invalidates_cache_on_yaml_change() {
        let dir = temp_dir("watch");
        fs::write(dir.join("app.yml"), "a: 1").unwrap();
        let first = project_files_cached(&dir);
        assert_eq!(first.len(), 1);

        // project_files_cached 返回时监听已安装,之后新增 yaml 应让缓存失效
        fs::write(dir.join("b.yml"), "b: 2").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if project_files_cached(&dir).len() == 2 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "文件监听未在超时内让 walk 缓存失效"
            );
            std::thread::sleep(Duration::from_millis(50));
        }

        let _ = fs::remove_dir_all(&dir);
    }
}
