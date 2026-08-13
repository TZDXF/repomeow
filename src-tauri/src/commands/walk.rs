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

/// 递归列出未被 .gitignore / .ignore 排除的文件(相对 root)。
/// 与 all_files 的差异:尊重 ignore 规则(require_git(false),非 git 项目同样生效),
/// 但包含隐藏文件、不额外跳过 node_modules——node_modules 是否出现完全由
/// 项目的 ignore 规则决定。与 all_files 取差集即可标出"被 git 排除"的条目。
pub fn unignored_files(root: &Path) -> Vec<PathBuf> {
    let (tx, rx) = std::sync::mpsc::channel::<PathBuf>();
    WalkBuilder::new(root)
        .require_git(false)
        .hidden(false)
        .filter_entry(|e| e.file_name() != ".git")
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
    rx.into_iter()
        .filter_map(|p| p.strip_prefix(root).ok().map(PathBuf::from))
        .collect()
}

/// 相对路径转 '/' 分隔字符串(Windows 下 '\' 归一化)
pub fn to_slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// 递归列出项目内全部文件(相对 root),供文件预览页展示完整文件树。
///
/// 与 project_files 的差异:不看 .gitignore / .ignore、包含隐藏文件与
/// node_modules——预览页的目标是"项目里有什么就能看到什么"。
/// 唯一跳过的是 .git 目录内部:纯仓库数据没有预览价值,且体量可能极大。
pub fn all_files(root: &Path) -> Vec<PathBuf> {
    let (tx, rx) = std::sync::mpsc::channel::<PathBuf>();
    WalkBuilder::new(root)
        .require_git(false)
        .hidden(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .parents(false)
        .filter_entry(|e| e.file_name() != ".git")
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
pub fn project_files_cached(root: &Path) -> Arc<Vec<PathBuf>> {
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

/// 为已缓存的根目录安装递归文件监听(幂等)。
/// 监听安装失败(权限、平台不支持等)时静默降级为纯 TTL 失效。
fn ensure_watcher(root: &Path) {
    let watchers = WALK_WATCHERS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = watchers.lock().unwrap();
    if map.contains_key(root) {
        return;
    }
    // 根目录的 .gitignore/.ignore 用来过滤构建目录(target/ 等)里的事件,
    // 避免构建产物抖动反复让缓存失效;嵌套 .gitignore 不纳入判断,
    // 漏过滤的代价只是偶发的一次重扫
    let mut gi_builder = ignore::gitignore::GitignoreBuilder::new(root);
    gi_builder.add(root.join(".gitignore"));
    gi_builder.add(root.join(".ignore"));
    // 规则文件本身解析失败时退化为不过滤(多失效几次缓存,无害)
    let gitignore = gi_builder
        .build()
        .unwrap_or_else(|_| ignore::gitignore::Gitignore::empty());
    let root_owned = root.to_path_buf();
    let Ok(mut watcher) = notify::recommended_watcher(move |res| {
        let Ok(event) = res else { return };
        if !event_affects_assets(&event, &gitignore) {
            return;
        }
        if let Some(cache) = WALK_CACHE.get() {
            cache.lock().unwrap().remove(&root_owned);
        }
    }) else {
        return;
    };
    if watcher.watch(root, notify::RecursiveMode::Recursive).is_ok() {
        map.insert(root.to_path_buf(), watcher);
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
/// 保守按失效处理;否则要求至少一条相关路径且未被根 gitignore 排除
/// (排除 target/ 等构建目录的写入抖动)
fn event_affects_assets(event: &notify::Event, gitignore: &ignore::gitignore::Gitignore) -> bool {
    if event.paths.is_empty() {
        return true;
    }
    event.paths.iter().any(|p| {
        is_asset_relevant(p) && !gitignore.matched_path_or_any_parents(p, p.is_dir()).is_ignore()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "repomeow-walk-{tag}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
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
    fn all_files_includes_hidden_and_node_modules_but_skips_git_dir() {
        let dir = temp_dir("all");
        fs::create_dir_all(dir.join("node_modules/dep")).unwrap();
        fs::write(dir.join("node_modules/dep/package.json"), "{}").unwrap();
        fs::create_dir_all(dir.join(".github")).unwrap();
        fs::write(dir.join(".github/ci.yml"), "on: push").unwrap();
        fs::write(dir.join(".env"), "A=1").unwrap();
        fs::write(dir.join(".gitignore"), "ignored/\n").unwrap();
        fs::create_dir_all(dir.join("ignored")).unwrap();
        fs::write(dir.join("ignored/x.txt"), "x").unwrap();
        fs::create_dir_all(dir.join(".git/objects")).unwrap();
        fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main").unwrap();
        fs::write(dir.join("src.rs"), "fn main() {}").unwrap();

        let files = all_files(&dir);
        let names: Vec<String> = files.iter().map(|p| to_slash(p)).collect();
        for expected in [
            "node_modules/dep/package.json",
            ".github/ci.yml",
            ".env",
            ".gitignore",
            "ignored/x.txt",
            "src.rs",
        ] {
            assert!(names.contains(&expected.to_string()), "缺少 {expected}");
        }
        assert!(!names.iter().any(|n| n.starts_with(".git/")));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unignored_files_respects_gitignore_but_keeps_hidden() {
        let dir = temp_dir("unignored");
        fs::write(dir.join(".gitignore"), "logs/\nbuild/output.txt\n").unwrap();
        fs::write(dir.join(".env"), "A=1").unwrap();
        fs::create_dir_all(dir.join("logs")).unwrap();
        fs::write(dir.join("logs/app.log"), "x").unwrap();
        fs::create_dir_all(dir.join("build")).unwrap();
        fs::write(dir.join("build/output.txt"), "x").unwrap();
        fs::write(dir.join("build/keep.txt"), "x").unwrap();
        fs::create_dir_all(dir.join("node_modules/dep")).unwrap();
        fs::write(dir.join("node_modules/dep/package.json"), "{}").unwrap();

        let files = unignored_files(&dir);
        let names: Vec<String> = files.iter().map(|p| to_slash(p)).collect();
        // 隐藏文件保留;未被忽略的 node_modules 内容保留
        assert!(names.contains(&".env".to_string()));
        assert!(names.contains(&".gitignore".to_string()));
        assert!(names.contains(&"node_modules/dep/package.json".to_string()));
        assert!(names.contains(&"build/keep.txt".to_string()));
        // 被 .gitignore 排除的不出现
        assert!(!names.iter().any(|n| n.starts_with("logs/")));
        assert!(!names.contains(&"build/output.txt".to_string()));

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
        assert!(!is_asset_relevant(Path::new("/p/node_modules/dep/package.json")));
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
