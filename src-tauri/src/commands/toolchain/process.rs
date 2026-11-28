#[cfg(unix)]
use std::io::Read;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::ExitStatus;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use std::sync::RwLock;

use crate::commands::open::hidden;

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// 登录 shell 探测的超时上限:用户 rc 文件里有阻塞式交互时不能让检测卡死。
#[cfg(target_os = "macos")]
const LOGIN_SHELL_TIMEOUT: Duration = Duration::from_secs(5);

/// 登录 shell 探针结果的标记:输出被包在标记之间,避免 motd / 插件输出混进 PATH 解析。
#[cfg(any(target_os = "macos", test))]
const PATH_BEGIN: &str = "REPOMEOW_PATH_BEGIN:";
#[cfg(any(target_os = "macos", test))]
const PATH_END: &str = ":REPOMEOW_PATH_END";

/// 探针把 PATH 写进这个环境变量指向的临时文件:stdout/stderr 一律丢弃,
/// 既避开 rc 文件回显(`set -x`)污染解析,也让子进程退出后不存在「读管道到 EOF」
/// 的无界等待(后台孙进程持有管道不再影响返回时间)。
#[cfg(target_os = "macos")]
const PROBE_FILE_ENV: &str = "REPOMEOW_PATH_PROBE_FILE";

/// POSIX 系登录 shell(zsh / bash / sh):把 `$PATH` 写进临时文件。
#[cfg(target_os = "macos")]
const POSIX_PATH_COMMAND: &str =
    "printf 'REPOMEOW_PATH_BEGIN:%s:REPOMEOW_PATH_END' \"$PATH\" > \"$REPOMEOW_PATH_PROBE_FILE\"";

/// fish 的 `$PATH` 是列表,`printf '%s\n' $PATH` 一行一个元素,由 Rust 侧用 `:` 拼回 PATH;
/// 直接 `"$PATH"` 在 fish 下会按空格连接,会把多段粘连成一个含空格的伪目录。
#[cfg(target_os = "macos")]
const FISH_PATH_COMMAND: &str = "printf '%s\\n' $PATH > \"$REPOMEOW_PATH_PROBE_FILE\"";

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
        // 版本探测与工具自身的子进程(node → npm 等)沿用同一套候选目录,
        // 否则「找到了却拿不到版本」。候选目录只由继承 PATH 与登录 shell 结果组成;
        // 登录 shell 探针自身不走 probe_command,所以这里不会递归触发新探测。
        if let Some(path) = probe_child_path() {
            command.env("PATH", path);
        }
    }
    command
}

/// PATH 上的可执行文件命中(找不到返回空)。
/// 按 `windows` / `unix` 各给一份实现:两者的候选目录来源完全不同(Tauri 只支持
/// windows / linux / macos,后两者都是 `unix`,所以这份拆分覆盖全部构建目标),
/// 比「`not(windows)` 里再套 unix 专用 API」更容易核对条件编译的恒等式——
/// `is_executable_file` 依赖 `std::os::unix::fs::PermissionsExt`,只能是 `unix`。
#[cfg(windows)]
pub(super) fn cli_hits_on_path(cli: &str) -> Vec<PathBuf> {
    where_hits(cli)
}

#[cfg(unix)]
pub(super) fn cli_hits_on_path(cli: &str) -> Vec<PathBuf> {
    path_hits_in(cli, &probe_search_dirs())
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
#[cfg(unix)]
pub(super) fn path_hits_in(cli: &str, dirs: &[PathBuf]) -> Vec<PathBuf> {
    dirs.iter()
        .map(|dir| dir.join(cli))
        .filter(|candidate| is_executable_file(candidate))
        .collect()
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    // metadata() 跟随符号链接:nvm/volta/fnm 的 bin 常是 symlink,不能按目录项本身判断
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

/// macOS 上给探测子进程用的 PATH:与 `probe_search_dirs` 同一套候选目录。
#[cfg(target_os = "macos")]
fn probe_child_path() -> Option<String> {
    std::env::join_paths(probe_search_dirs())
        .ok()
        .map(|value| value.to_string_lossy().into_owned())
        .filter(|value| !value.is_empty())
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
#[cfg(unix)]
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
///
/// nvm 不再依赖 `NVM_SYMLINK_CURRENT`(`~/.nvm/current` 只有显式开启该选项才存在,
/// 默认恒空),改为按 `~/.nvm/versions/node/*/bin` 解析真实目录,优先 `alias/default`。
#[cfg(target_os = "macos")]
pub(super) fn fallback_probe_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/local/sbin"),
    ];
    if let Some(home) = home {
        for sub in [
            ".cargo/bin",
            ".local/bin",
            ".volta/bin",
            ".fnm/aliases/default/bin",
        ] {
            dirs.push(home.join(sub));
        }
        dirs.extend(nvm_node_bin_dirs(home));
    }
    // 只留真实存在的目录:兜底列表的路径是静态猜的,不存在的项进了候选也没有意义
    dirs.retain(|dir| dir.is_dir());
    dirs
}

/// nvm 已装版本的 bin 目录:先放 `alias/default` 对应的版本,再按版本号从高到低。
/// 只保留真实存在的目录(路径含空格不影响,交给下层 `path_hits_in` 拼接判断)。
#[cfg(target_os = "macos")]
fn nvm_node_bin_dirs(home: &Path) -> Vec<PathBuf> {
    use super::version::natural_version_cmp;

    let versions = home.join(".nvm").join("versions").join("node");
    let Ok(entries) = std::fs::read_dir(&versions) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| {
            name.trim_start_matches('v')
                .starts_with(|character: char| character.is_ascii_digit())
        })
        .collect();
    names.sort_by(|a, b| natural_version_cmp(a.trim_start_matches('v'), b.trim_start_matches('v')));
    names.reverse();

    let alias = std::fs::read_to_string(home.join(".nvm").join("alias").join("default"))
        .unwrap_or_default()
        .trim()
        .trim_start_matches('v')
        .to_string();
    let mut dirs: Vec<PathBuf> = Vec::new();
    if !alias.is_empty() {
        let matched = names
            .iter()
            .find(|name| name.trim_start_matches('v') == alias)
            .or_else(|| {
                names.iter().find(|name| {
                    name.trim_start_matches('v')
                        .starts_with(&format!("{alias}."))
                })
            });
        if let Some(name) = matched {
            dirs.push(versions.join(name).join("bin"));
        }
    }
    for name in &names {
        let dir = versions.join(name).join("bin");
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs.retain(|dir| dir.is_dir());
    dirs
}

/// 从登录 shell 写出的临时文件内容里取出 PATH 值;标记缺失或内容为空返回 None。
/// 生产环境只在 macOS 用(其他 Unix 不跑登录 shell),`test` 下各平台都编译以便单测覆盖解析逻辑。
#[cfg(any(target_os = "macos", test))]
pub(super) fn parse_shell_path_output(output: &str) -> Option<String> {
    // 出现多组标记说明写进来的不是「一条 printf 结果」:fish / nushell 把 `$PATH` 当列表
    // 逐元素展开就会这样,取其中任意一段都只是 PATH 的一部分,必须整体判失败。
    if output.matches(PATH_BEGIN).count() != 1 {
        return None;
    }
    let start = output.find(PATH_BEGIN)? + PATH_BEGIN.len();
    let rest = &output[start..];
    let end = rest.find(PATH_END)?;
    let value = rest[..end].trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// 探针文件内容 → PATH 值(fish 为一行一个元素的列表形态),再过形态校验。
#[cfg(any(target_os = "macos", test))]
pub(super) fn shell_path_from_file(content: &str, list_form: bool) -> Option<String> {
    let raw = if list_form {
        // 用平台 PATH 分隔符拼回(Unix 是 `:`,Windows 是 `;`),不写死冒号
        let lines: Vec<&str> = content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        std::env::join_paths(lines)
            .ok()?
            .to_string_lossy()
            .into_owned()
    } else {
        parse_shell_path_output(content)?
    };
    is_valid_shell_path(&raw).then_some(raw)
}

/// PATH 形态校验:任何「伪值」都必须判失败,交给调用方走兜底目录。
/// 判失败的情形包括:空值、标记/格式串残留、`split_paths` 后为空、出现相对路径、
/// 或某个含空格的段落并不是真实目录(fish / nushell 把列表按空格连接会把多段
/// 粘连成一个含空格的伪目录)。同时要求至少一段是真实存在的目录。
#[cfg(any(target_os = "macos", test))]
pub(super) fn is_valid_shell_path(value: &str) -> bool {
    if value.contains(PATH_BEGIN) || value.contains(PATH_END) || value.contains("%s") {
        return false;
    }
    let dirs: Vec<PathBuf> = std::env::split_paths(value).collect();
    if dirs.is_empty() {
        return false;
    }
    let mut existing = 0;
    for dir in &dirs {
        if !dir.is_absolute() {
            return false;
        }
        let text = dir.to_string_lossy();
        if text.contains('\n') || text.contains('\r') {
            return false;
        }
        let exists = dir.is_dir();
        if text.contains(' ') && !exists {
            return false;
        }
        if exists {
            existing += 1;
        }
    }
    existing > 0
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
/// Homebrew / cargo / nvm 等目录都不在内;探测失败(rc 报错、超时、shell 不存在、
/// 形态校验不过)缓存为 `None`,调用方退回继承的 PATH + 兜底目录。
#[cfg(target_os = "macos")]
pub(super) fn refresh_login_shell_path() {
    let value = login_shell_path();
    if let Ok(mut guard) = LOGIN_SHELL_PATH.write() {
        *guard = Some(value);
    }
}

#[cfg(target_os = "macos")]
fn login_shell_path() -> Option<String> {
    // 用户 SHELL 未设置时用系统默认的 zsh;bash/sh 走 POSIX 写法,fish 走列表写法
    let shell = std::env::var_os("SHELL")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"));
    probe_login_shell_path(&shell, &[])
}

/// 按 shell 类型跑一次登录 shell 把 PATH 写进临时文件。
///
/// - **有界**:子进程放进独立进程组,`wait_for_exit` 只按超时轮询子进程自身是否退出;
///   超时整组 `KILL`(macOS 上只杀 shell 会留下持有管道的孙进程,而这里根本没有管道);
///   读完文件即返回,不依赖任何管道 EOF,rc 里 `( sleep 600 ) &` 这类后台子孙
///   不再让「重新检测」卡死。
/// - **只认探针自己写的值**:stdout/stderr 全部重定向丢弃,`set -x` / motd / 插件
///   回显都进不了文件,文件内容只可能来自那条 `printf`。再经 `shell_path_from_file`
///   的形态校验,伪值一律判失败走兜底。
/// - `extra_env` 只给测试注入 `ZDOTDIR` 等变量,不读不写全局进程环境。
#[cfg(target_os = "macos")]
pub(super) fn probe_login_shell_path(shell: &Path, extra_env: &[(&str, &str)]) -> Option<String> {
    probe_login_shell_path_with_timeout(shell, extra_env, LOGIN_SHELL_TIMEOUT)
}

/// 同上,超时可指定(单测用它把「卡死」场景压到 1s,不必等满 5s)。
#[cfg(target_os = "macos")]
pub(super) fn probe_login_shell_path_with_timeout(
    shell: &Path,
    extra_env: &[(&str, &str)],
    timeout: Duration,
) -> Option<String> {
    use std::os::unix::process::CommandExt;

    let list_form = shell
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase() == "fish")
        .unwrap_or(false);
    let file = probe_temp_file()?;
    let command = if list_form {
        FISH_PATH_COMMAND
    } else {
        POSIX_PATH_COMMAND
    };
    let mut command_builder = hidden(Command::new(shell));
    command_builder
        .args(["-ilc", command])
        .env(PROBE_FILE_ENV, &file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    for (key, value) in extra_env {
        command_builder.env(key, value);
    }
    let child = command_builder.spawn().ok();
    let outcome = child.and_then(|mut child| {
        let status = wait_for_exit(&mut child, timeout)?;
        status.success().then_some(())
    });
    let content = outcome.and_then(|()| std::fs::read_to_string(&file).ok());
    let _ = std::fs::remove_file(&file);
    content.and_then(|content| shell_path_from_file(&content, list_form))
}

/// 探针用的唯一临时文件路径(不预先创建,由 shell 重定向创建)。
#[cfg(target_os = "macos")]
fn probe_temp_file() -> Option<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or(0);
    let name = format!(
        "repomeow-shell-path-{}-{}-{}",
        std::process::id(),
        nanos,
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let dir = std::env::temp_dir();
    let path = dir.join(name);
    // 预先删掉同名残留,避免读到上一次的旧内容
    let _ = std::fs::remove_file(&path);
    Some(path)
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
///
/// Unix:子进程进独立进程组,stdout/stderr 由伴生线程读到 EOF(不阻塞等待方),
/// 等待退出与收输出都受 `timeout` 约束;超时或收不到输出时整组 `KILL`。
/// 这样即使工具自己拉起持有管道的子孙进程,调用方也不会被拖住。
/// Windows:沿用原有管道实现(where.exe / winget 不拉起长驻子孙,未改以避风险)。
#[cfg(unix)]
pub(super) fn run_with_timeout_in(
    exe: &Path,
    args: &[&str],
    timeout: Duration,
) -> Option<(bool, String)> {
    use std::os::unix::process::CommandExt;
    use std::sync::mpsc::channel;

    let mut child = probe_command(exe)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .ok()?;
    let pid = child.id();
    let (sender, receiver) = channel::<(u8, String)>();
    for (tag, pipe) in [
        (0u8, child.stdout.take().map(PipeReader::Stdout)),
        (1u8, child.stderr.take().map(PipeReader::Stderr)),
    ] {
        let sender = sender.clone();
        if let Some(pipe) = pipe {
            std::thread::spawn(move || {
                let mut buffer = String::new();
                let mut source = pipe.into_reader();
                let _ = source.read_to_string(&mut buffer);
                let _ = sender.send((tag, buffer));
            });
        }
    }
    drop(sender);

    let status = wait_for_exit(&mut child, timeout)?;

    // 收尾窗口单独限一个小值:子进程已退出且管道已关时,伴生线程早已读完,
    // recv 会立刻返回;只有「孙进程仍持有管道」才会走到超时分支(整组杀掉后放弃)。
    // 用 timeout 原值会让最坏耗时翻倍(超时 + 收尾各一份)。
    let deadline = Instant::now() + timeout.min(Duration::from_secs(2));
    let mut collected: [Option<String>; 2] = [None, None];
    for _ in 0..2 {
        let left = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(left.max(Duration::from_millis(10))) {
            Ok((tag, text)) => {
                if let Some(slot) = collected.get_mut(usize::from(tag)) {
                    *slot = Some(text);
                }
            }
            // 孙进程仍持有管道:整组杀掉断掉管道,再给一次机会收尾,拿不到就放弃
            Err(_) => {
                kill_group(pid);
                while let Ok((tag, text)) = receiver.recv_timeout(Duration::from_millis(200)) {
                    if let Some(slot) = collected.get_mut(usize::from(tag)) {
                        *slot = Some(text);
                    }
                }
                break;
            }
        }
    }
    let out = collected[0].take().unwrap_or_default();
    let err = collected[1].take().unwrap_or_default();
    Some((status.success(), format!("{out}{err}")))
}

#[cfg(windows)]
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
    let output = wait_with_output_bounded(child, timeout)?;
    Some((output.status.success(), merged_output(&output)))
}

/// Unix 上前一步 spawn 的管道来源,只为把两种句柄塞进同一个读取闭包。
#[cfg(unix)]
enum PipeReader {
    Stdout(std::process::ChildStdout),
    Stderr(std::process::ChildStderr),
}

#[cfg(unix)]
impl PipeReader {
    fn into_reader(self) -> Box<dyn Read + Send> {
        match self {
            PipeReader::Stdout(pipe) => Box::new(pipe),
            PipeReader::Stderr(pipe) => Box::new(pipe),
        }
    }
}

/// 整组终止:负 PID 即进程组。macOS 上只杀 shell 会留下持有管道的孙进程,
/// 所以超时/收尾都按组杀;`/bin/kill` 不可用(极少数精简发行版)时退回 PATH 上的 `kill`。
#[cfg(unix)]
fn kill_group(pid: u32) {
    let group = format!("-{pid}");
    let Ok(killed) = Command::new("/bin/kill")
        .args(["-s", "KILL", "--", &group])
        .status()
    else {
        let _ = Command::new("kill").args(["-9", &group]).status();
        return;
    };
    if !killed.success() {
        let _ = Command::new("kill").args(["-9", &group]).status();
    }
}

/// 等到子进程退出,超时则整组杀掉并返回 None(不留僵尸进程)。
/// 只在 Unix 用:Windows 的 `run_with_timeout_in` 沿用原有管道实现。
#[cfg(unix)]
fn wait_for_exit(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {
                if Instant::now() > deadline {
                    kill_child_group(child);
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => {
                kill_child_group(child);
                return None;
            }
        }
    }
}

/// Windows:等子进程退出后再收管道(与原有行为一致)。
#[cfg(windows)]
fn wait_with_output_bounded(mut child: Child, timeout: Duration) -> Option<std::process::Output> {
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

/// 终止子进程所在的整个进程组(Windows 分支不用,见 `wait_for_exit` 的平台说明)。
#[cfg(unix)]
fn kill_child_group(child: &mut Child) {
    kill_group(child.id());
}

#[cfg(windows)]
fn merged_output(output: &std::process::Output) -> String {
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
