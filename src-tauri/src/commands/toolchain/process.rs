use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use std::sync::RwLock;

use crate::commands::open::hidden;

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// 登录 shell 探测的超时上限:用户 rc 文件里有阻塞式交互时不能让检测卡死。
#[cfg(target_os = "macos")]
const LOGIN_SHELL_TIMEOUT: Duration = Duration::from_secs(5);

/// 登录 shell 探针结果的标记:输出被包在标记之间,避免 motd / rc 文件输出 /
/// 作业控制警告混进 PATH 解析。
#[cfg(any(target_os = "macos", test))]
const PATH_BEGIN: &str = "REPOMEOW_PATH_BEGIN:";
#[cfg(any(target_os = "macos", test))]
const PATH_END: &str = ":REPOMEOW_PATH_END";
/// 在登录 shell 里取真实 PATH 的命令。
#[cfg(target_os = "macos")]
const SHELL_PATH_COMMAND: &str = "printf 'REPOMEOW_PATH_BEGIN:%s:REPOMEOW_PATH_END' \"$PATH\"";

/// 进程内缓存的登录 shell PATH。
/// - 外层 `None` 表示尚未探测,`Some(None)` 表示探测过但失败(失败也缓存,
///   否则一次探测失败会让 12 个工具各跑一遍登录 shell);
/// - 每次 `detect_toolchains` 开头由 `refresh_login_shell_path` 重解析一次,
///   让「重新检测」反映最新的 shell 配置;
/// - 探测子进程自身不读这份缓存,避免递归初始化。
#[cfg(target_os = "macos")]
static LOGIN_SHELL_PATH: RwLock<Option<Option<String>>> = RwLock::new(None);

fn probe_command(exe: &Path) -> Command {
    let mut command = hidden(Command::new(exe));
    #[cfg(windows)]
    super::windows_env::apply(&mut command);
    #[cfg(target_os = "macos")]
    {
        // 版本探测与工具自身的子进程(node → npm 等)沿用登录 shell 的 PATH,
        // 否则「找到了却拿不到版本」。
        if let Some(path) = cached_login_shell_path() {
            command.env("PATH", path);
        }
    }
    command
}

/// PATH 上的可执行文件命中(找不到返回空)。
pub(super) fn cli_hits_on_path(cli: &str) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        where_hits(cli)
    }
    #[cfg(not(windows))]
    {
        path_hits_in(cli, &probe_search_dirs())
    }
}

/// Windows:where.exe 继承的 PATH 已由 windows_env 从注册表刷新。
#[cfg(windows)]
fn where_hits(cli: &str) -> Vec<PathBuf> {
    let probe = probe_command(Path::new("where.exe")).arg(cli).output();
    let out = match probe {
        Ok(out) if out.status.success() => out,
        _ => return Vec::new(),
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Unix:直接扫候选目录,不依赖 `which`(GUI 进程的最小 PATH 里未必有它,
/// 而且这里需要可测的纯路径匹配)。
#[cfg(not(windows))]
pub(super) fn path_hits_in(cli: &str, dirs: &[PathBuf]) -> Vec<PathBuf> {
    dirs.iter()
        .map(|dir| dir.join(cli))
        .filter(|candidate| is_executable_file(candidate))
        .collect()
}

#[cfg(not(windows))]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// macOS 候选搜索目录:登录 shell 的真实 PATH 优先,其次进程继承的 PATH;
/// 只有登录 shell 探测失败时才补常见包管理器目录(兜底)。探测成功时不再额外扫描,
/// 保证结果与用户在终端里 `which` 得到的一致(不把「装了但没配进 PATH」的工具误报为可用)。
#[cfg(target_os = "macos")]
fn probe_search_dirs() -> Vec<PathBuf> {
    let shell_path = cached_login_shell_path();
    let mut values: Vec<String> = Vec::new();
    if let Some(shell_path) = &shell_path {
        values.push(shell_path.clone());
    }
    if let Some(inherited) = std::env::var_os("PATH") {
        values.push(inherited.to_string_lossy().into_owned());
    }
    let mut dirs = merge_path_values(&values);
    if shell_path.is_none() {
        for dir in fallback_probe_dirs(user_home_path().as_deref()) {
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
    }
    dirs
}

/// 其他 Unix(Linux):保持原有语义,只看进程继承的 PATH(等价于原来的 `which`),
/// 不额外跑登录 shell,也不追加兜底目录,避免行为倒退。
#[cfg(all(unix, not(target_os = "macos")))]
fn probe_search_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .filter(|value| !value.is_empty())
        .map(|value| merge_path_values(&[value.to_string_lossy().into_owned()]))
        .unwrap_or_default()
}

/// 把若干个 PATH 值按先后展开成目录列表,空段丢弃,重复目录只留首次出现。
#[cfg(not(windows))]
pub(super) fn merge_path_values(values: &[String]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for value in values {
        for dir in std::env::split_paths(value) {
            if dir.as_os_str().is_empty() || dirs.contains(&dir) {
                continue;
            }
            dirs.push(dir);
        }
    }
    dirs
}

/// 登录 shell 探测失败时的兜底目录(macOS):Homebrew(Apple Silicon / Intel)与
/// cargo / uv / nvm / volta / fnm 的常见安装位置。
#[cfg(target_os = "macos")]
pub(super) fn fallback_probe_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/local/sbin"),
    ];
    if let Some(home) = home {
        for sub in [
            ".cargo/bin",
            ".local/bin",
            ".nvm/current/bin",
            ".volta/bin",
            ".fnm/aliases/default/bin",
        ] {
            dirs.push(home.join(sub));
        }
    }
    dirs
}

/// 从登录 shell 的输出里取出 PATH 值;标记缺失或内容为空返回 None。
/// 生产环境只在 macOS 用(其他 Unix 不跑登录 shell),`test` 下各平台都编译以便单测覆盖解析逻辑。
#[cfg(any(target_os = "macos", test))]
pub(super) fn parse_shell_path_output(output: &str) -> Option<String> {
    // rfind:rc 文件把命令本身打印出来时,取最后一次(即真实结果)
    let start = output.rfind(PATH_BEGIN)? + PATH_BEGIN.len();
    let rest = &output[start..];
    let end = rest.find(PATH_END)?;
    let value = rest[..end].trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// 读缓存的登录 shell PATH;从未探测过则先探测一次。
#[cfg(target_os = "macos")]
fn cached_login_shell_path() -> Option<String> {
    let cached = LOGIN_SHELL_PATH.read().ok().and_then(|guard| guard.clone());
    match cached {
        Some(value) => value,
        None => {
            refresh_login_shell_path();
            LOGIN_SHELL_PATH
                .read()
                .ok()
                .and_then(|guard| guard.clone())
                .flatten()
        }
    }
}

/// 重新探测登录 shell 的真实 PATH(每次工具链检测开头调用)。
/// macOS 从 Finder/Dock 启动时 GUI 进程只有最小 PATH(`/usr/bin:/bin:/usr/sbin:/sbin`),
/// Homebrew / cargo / nvm 等目录都不在内;探测失败(rc 报错、超时、shell 不存在)
/// 缓存为 `None`,调用方退回继承的 PATH + 兜底目录。
#[cfg(target_os = "macos")]
pub(super) fn refresh_login_shell_path() {
    let value = login_shell_path();
    if let Ok(mut guard) = LOGIN_SHELL_PATH.write() {
        *guard = Some(value);
    }
}

#[cfg(target_os = "macos")]
fn login_shell_path() -> Option<String> {
    // 用户 SHELL 未设置时用系统默认的 zsh;其他 shell(bash/fish)同样返回该进程的 PATH
    let shell = std::env::var_os("SHELL")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"));
    // 刻意不走 probe_command / run_with_timeout_in:它们要读 LOGIN_SHELL_PATH,
    // 而这正是本函数在填充的值,复用会递归初始化。
    let child = hidden(Command::new(&shell))
        .args(["-ilc", SHELL_PATH_COMMAND])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let output = wait_with_timeout(child, LOGIN_SHELL_TIMEOUT)?;
    parse_shell_path_output(&merged_output(&output))
}

/// 展示用路径:剥掉 canonicalize 带来的 `\\?\` 前缀。
pub(super) fn display_path(path: &Path) -> String {
    let value = path.to_string_lossy().into_owned();
    value.strip_prefix(r"\\?\").unwrap_or(&value).to_string()
}

/// 按可执行文件路径推断安装来源。
pub(super) fn classify_source(path: &Path) -> String {
    let value = path.to_string_lossy().to_lowercase();
    if value.contains("winget") {
        "winget".to_string()
    } else if value.contains(r"\.cargo") || value.contains("/.cargo") {
        "rustup".to_string()
    } else if value.contains("/opt/homebrew/")
        || value.contains("/usr/local/cellar")
        || value.contains("/home/linuxbrew")
        || value.contains("/.linuxbrew/")
    {
        "brew".to_string()
    } else if value.contains("/.nvm/") {
        "nvm".to_string()
    } else if value.contains("/.fnm/") || value.contains("/fnm_multishells/") {
        "fnm".to_string()
    } else if value.contains("/.volta/") {
        "volta".to_string()
    } else {
        "standalone".to_string()
    }
}

/// 带默认超时跑命令并合并 stdout+stderr。
pub(super) fn run_with_timeout(exe: &Path, args: &[&str]) -> Option<(bool, String)> {
    run_with_timeout_in(exe, args, PROBE_TIMEOUT)
}

/// 同上，允许为联网探测指定更长超时。
pub(super) fn run_with_timeout_in(
    exe: &Path,
    args: &[&str],
    timeout: Duration,
) -> Option<(bool, String)> {
    let child = probe_command(exe)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let output = wait_with_timeout(child, timeout)?;
    Some((output.status.success(), merged_output(&output)))
}

/// 轮询等待子进程结束,超时则杀掉并返回 None(不留僵尸进程)。
fn wait_with_timeout(mut child: Child, timeout: Duration) -> Option<Output> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
    child.wait_with_output().ok()
}

fn merged_output(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

pub(super) fn user_home_path() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}
