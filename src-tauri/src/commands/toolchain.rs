//! 设置页「工具链」面板:常用开发 CLI 的检测与安装/更新/卸载/版本切换。
//!
//! 检测(detect_toolchains):where/which 找 PATH 上的可执行文件,跑 `--version`
//! 解析版本,版本管理器(nvm/fnm/vp/dotnet/uv)额外列出可切换的版本;
//! python 不单列——uv 行承载 python 版本管理(uv python install 装卸,
//! --default 建 python/python3 全局别名),uv 自身安装走官方脚本(irm/curl)。
//! 操作(toolchain_op):按 平台+安装来源 解析出命令串,直接 spawn_terminal
//! 在系统终端新窗口执行——安装/升级可能需要 UAC、网络与进度展示,交给终端
//! 是与应用「命令在终端跑」一致的选择,失败信息也直接可见。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use regex::Regex;

use crate::commands::open::{hidden, spawn_terminal};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::{
    ToolchainCaps, ToolchainKind, ToolchainRemoteVersion, ToolchainStatus, ToolchainVersion,
};

/// 子进程探测超时:坏掉的工具不应拖住整个检测流程(与 java.rs 的 JDK 探测同理)
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// `gh auth status` 的探测超时:它会联网校验 token,给比版本探测更长的时限
const AUTH_PROBE_TIMEOUT: Duration = Duration::from_secs(8);

// ── 工具注册表 ──────────────────────────────────────────────────────────────

/// 内置工具登记项;TOOLS 顺序即设置页各分组内的展示顺序
struct ToolSpec {
    /// CLI 名(探测与 toolchain_op 的 tool 参数)
    id: &'static str,
    kind: ToolchainKind,
    /// 版本探测依次尝试的参数组:老版本工具可能只认其中一种(如 nvm-windows 的 `version`)
    version_args: &'static [&'static str],
}

const TOOLS: &[ToolSpec] = &[
    ToolSpec {
        id: "rustup",
        kind: ToolchainKind::Rust,
        version_args: &["--version"],
    },
    ToolSpec {
        id: "rustc",
        kind: ToolchainKind::Rust,
        version_args: &["--version"],
    },
    ToolSpec {
        id: "cargo",
        kind: ToolchainKind::Rust,
        version_args: &["--version"],
    },
    ToolSpec {
        id: "uv",
        kind: ToolchainKind::Python,
        version_args: &["--version"],
    },
    ToolSpec {
        id: "nvm",
        kind: ToolchainKind::Node,
        version_args: &["--version", "version"],
    },
    ToolSpec {
        id: "fnm",
        kind: ToolchainKind::Node,
        version_args: &["--version"],
    },
    ToolSpec {
        id: "vp",
        kind: ToolchainKind::Node,
        version_args: &["--version", "-v"],
    },
    ToolSpec {
        id: "dotnet",
        kind: ToolchainKind::Dotnet,
        version_args: &["--version"],
    },
    ToolSpec {
        id: "git",
        kind: ToolchainKind::Git,
        version_args: &["--version"],
    },
    ToolSpec {
        id: "gh",
        kind: ToolchainKind::Git,
        version_args: &["--version"],
    },
];

// ── 检测 ────────────────────────────────────────────────────────────────────

/// 检测全部内置工具(detect_toolchains 的阻塞实现)。探测失败的工具返回
/// found=false 而非报错——「未安装」本身就是合法检测结果。
fn detect_toolchains_blocking() -> Vec<ToolchainStatus> {
    let mut statuses: Vec<ToolchainStatus> = TOOLS.iter().map(detect_one).collect();

    // nvm 是否以二进制形式在 PATH 上(windows 的 nvm-windows);
    // unix 的 nvm 是 shell 函数,拉不了远端列表,要在兜底填充前捕获
    let nvm_binary = statuses.iter().any(|s| s.id == "nvm" && s.found);

    // unix 的 nvm 是被 source 的 shell 函数,PATH 上没有二进制:
    // 以 ~/.nvm 目录存在为准,版本列表来自其 versions/node 子目录
    #[cfg(not(windows))]
    {
        if let Some(status) = statuses.iter_mut().find(|s| s.id == "nvm") {
            if !status.found {
                fill_unix_nvm(status);
            }
        }
    }

    // 版本管理器附加探测(仅对已找到的工具)
    for status in &mut statuses {
        if !status.found {
            continue;
        }
        let Some(path) = status.path.as_ref() else {
            continue;
        };
        let exe = PathBuf::from(path);
        status.versions = match status.id.as_str() {
            "nvm" => run_with_timeout(&exe, &["list"])
                .filter(|(ok, _)| *ok)
                .map(|(_, out)| parse_nvm_list(&out))
                .unwrap_or_default(),
            "fnm" => run_with_timeout(&exe, &["list"])
                .filter(|(ok, _)| *ok)
                .map(|(_, out)| parse_token_versions(&out))
                .unwrap_or_default(),
            "vp" => run_with_timeout(&exe, &["env", "list"])
                .filter(|(ok, _)| *ok)
                .map(|(_, out)| parse_vp_env_list(&out))
                .unwrap_or_default(),
            "dotnet" => run_with_timeout(&exe, &["--list-sdks"])
                .filter(|(ok, _)| *ok)
                .map(|(_, out)| parse_dotnet_sdks(&out))
                .unwrap_or_default(),
            // uv 行承载 python 版本管理:列出 uv 托管的解释器并标记当前全局版本
            "uv" => uv_python_versions(&exe),
            _ => Vec::new(),
        };
    }

    // gh 额外探测当前登录账号(`gh auth status` 会联网校验 token,给更长超时)
    if let Some(status) = statuses.iter_mut().find(|s| s.id == "gh" && s.found) {
        if let Some(exe) = status.path.as_ref().map(PathBuf::from) {
            status.account = run_with_timeout_in(&exe, &["auth", "status"], AUTH_PROBE_TIMEOUT)
                .filter(|(ok, _)| *ok)
                .and_then(|(_, out)| parse_gh_auth_status(&out));
        }
    }

    let rustup_found = statuses.iter().any(|s| s.id == "rustup" && s.found);
    for status in &mut statuses {
        status.caps = caps_for(
            &status.id,
            status.found,
            status.source.as_deref(),
            rustup_found,
        );
        // nvm 的远端列表只有 windows(nvm-windows 的 `nvm list available`)能拉
        if status.id == "nvm" {
            status.caps.can_list_remote = nvm_binary;
        }
    }
    statuses
}

/// 探测单个工具:PATH 命中 → 来源判定 → 版本探测
fn detect_one(spec: &ToolSpec) -> ToolchainStatus {
    let hit = cli_hits_on_path(spec.id).into_iter().next();
    let mut status = ToolchainStatus {
        id: spec.id.to_string(),
        kind: spec.kind,
        found: hit.is_some(),
        version: None,
        path: hit.as_ref().map(|p| display_path(p)),
        source: hit.as_ref().map(|p| classify_source(p)),
        versions: Vec::new(),
        account: None,
        caps: ToolchainCaps {
            can_install: false,
            can_update: false,
            can_uninstall: false,
            can_switch: false,
            can_list_remote: false,
        },
    };
    if let Some(exe) = &hit {
        status.version = probe_version(exe, spec.version_args);
    }
    status
}

/// where/which 命中的首个路径(找不到或执行失败返回空)
fn cli_hits_on_path(cli: &str) -> Vec<PathBuf> {
    #[cfg(windows)]
    let probe = hidden(Command::new("where")).arg(cli).output();
    #[cfg(not(windows))]
    let probe = Command::new("which").arg(cli).output();
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

/// 展示用路径:剥掉 canonicalize 带来的 \\?\ 前缀(与 java.rs 同法)
fn display_path(path: &Path) -> String {
    let s = path.to_string_lossy().into_owned();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

/// 按可执行文件路径推断安装来源(决定更新/卸载走哪条命令)
fn classify_source(path: &Path) -> String {
    let s = path.to_string_lossy().to_lowercase();
    if s.contains("winget") {
        "winget".to_string()
    } else if s.contains(r"\.cargo") || s.contains("/.cargo") {
        "rustup".to_string()
    } else if s.contains("/opt/homebrew/")
        || s.contains("/usr/local/cellar")
        || s.contains("/home/linuxbrew")
    {
        "brew".to_string()
    } else {
        "standalone".to_string()
    }
}

/// 依次尝试各参数组跑 `<exe> <args>` 解析版本:
/// 任一参数组输出里能提取 x.y 形态的版本串即采用;全部跑完仍没有时,
/// 若存在「退出码 0 但无版本串」的输出则回退其首行(自定义工具的横幅等),
/// 连命令都跑不起来才返回 None(仍算 found,前端显示「已安装」)。
fn probe_version(exe: &Path, attempts: &[&str]) -> Option<String> {
    let mut last_ok_output = String::new();
    for args in attempts {
        if let Some((true, out)) = run_with_timeout(exe, &[args]) {
            if let Some(v) = extract_semver(&out) {
                return Some(v);
            }
            last_ok_output = out;
        }
    }
    first_nonempty_line(&last_ok_output)
}

/// 带超时跑命令并合并 stdout+stderr,超时/启动失败返回 None
fn run_with_timeout(exe: &Path, args: &[&str]) -> Option<(bool, String)> {
    run_with_timeout_in(exe, args, PROBE_TIMEOUT)
}

/// 同上,自定义超时(`gh auth status` 这类要联网校验的探测用更长时限)
fn run_with_timeout_in(exe: &Path, args: &[&str], timeout: Duration) -> Option<(bool, String)> {
    let mut child = hidden(Command::new(exe))
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
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
    let out = child.wait_with_output().ok()?;
    Some((
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    ))
}

/// 从文本中提取首个 `数字.数字` 形态的版本串:
/// `git version 2.44.0.windows.1` → "2.44.0"、`rustc 1.77.0 (...)` → "1.77.0"
fn extract_semver(text: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\d+(\.\d+)+").unwrap());
    re.find(text).map(|m| m.as_str().to_string())
}

fn first_nonempty_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

// ── 版本列表解析 ────────────────────────────────────────────────────────────

/// nvm-windows `nvm list` 行如 `  * 22.11.0 (Currently using 64-bit executable)`;
/// 只认版本形态的行,滤掉可能的表头与提示文字
fn parse_nvm_list(text: &str) -> Vec<ToolchainVersion> {
    text.lines()
        .filter_map(|line| {
            let current = line.trim_start().starts_with('*');
            let rest = line.trim().trim_start_matches('*').trim_start();
            let name = rest.split_whitespace().next()?;
            version_token(name).map(|name| ToolchainVersion { name, current })
        })
        .collect()
}

/// fnm `fnm list`(实测:`*` 只标在当前行,当前行还带 default 别名):
/// ```text
/// * v18.20.4 default
///   v20.11.1
/// ```
fn parse_token_versions(text: &str) -> Vec<ToolchainVersion> {
    text.lines()
        .filter_map(|line| {
            let current =
                line.starts_with('*') || line.contains("default") || line.contains("current");
            let rest = line.trim_start().trim_start_matches('*').trim_start();
            let token = rest.split_whitespace().next()?;
            version_token(token).map(|name| ToolchainVersion { name, current })
        })
        .collect()
}

/// vp `vp env list`(实测:每行以 `*` 作项目符号,不代表当前!当前版本行尾带
/// `current` 字样且混有 ANSI 颜色码;末尾还有 note 提示行,被版本形态过滤):
/// ```text
/// * v24.15.0
/// * v24.19.0 current
/// note: Run `vp env clean` to free disk space...
/// ```
fn parse_vp_env_list(text: &str) -> Vec<ToolchainVersion> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\x1b\[[0-9;]*m").unwrap());
    re.replace_all(text, "")
        .lines()
        .filter_map(|line| {
            let current = line.contains("current");
            let rest = line.trim_start().trim_start_matches('*').trim_start();
            let token = rest.split_whitespace().next()?;
            version_token(token).map(|name| ToolchainVersion { name, current })
        })
        .collect()
}

/// token 规整为版本名:`v22.11.0` → "22.11.0";非版本形态返回 None
fn version_token(token: &str) -> Option<String> {
    let bare = token.trim_start_matches('v');
    bare.starts_with(|c: char| c.is_ascii_digit())
        .then(|| bare.to_string())
}

/// `dotnet --list-sdks` 行如 `8.0.204 [C:\Program Files\dotnet\sdk]`(无「当前」概念)
fn parse_dotnet_sdks(text: &str) -> Vec<ToolchainVersion> {
    text.lines()
        .filter_map(|line| {
            let name = line.split_whitespace().next()?;
            version_token(name).map(|name| ToolchainVersion {
                name,
                current: false,
            })
        })
        .collect()
}

/// uv 管理的 Python 版本:`uv python list --only-installed` 列出,
/// `uv python find` 输出的路径定位当前全局版本(匹配不上则全部不标)
fn uv_python_versions(uv: &Path) -> Vec<ToolchainVersion> {
    let Some((true, out)) = run_with_timeout(uv, &["python", "list", "--only-installed"]) else {
        return Vec::new();
    };
    let current = run_with_timeout(uv, &["python", "find"])
        .filter(|(ok, _)| *ok)
        .map(|(_, out)| out)
        .and_then(|out| python_version_from_path(&out));
    parse_uv_python_list(&out)
        .into_iter()
        .map(|name| {
            // uv python find 可能给出目录级前缀("3.12")或完整版本("3.12.7"),双向前缀匹配
            let matched = current.as_deref().is_some_and(|c| {
                name == c
                    || name.starts_with(&format!("{c}."))
                    || c.starts_with(&format!("{name}."))
            });
            ToolchainVersion {
                current: matched,
                name,
            }
        })
        .collect()
}

/// `uv python list --only-installed` 行如
/// `cpython-3.12.7-windows-x86_64-none    C:\...\python.exe`:
/// 取首 token(cpython-/pypy- 前缀)里的版本号;同版本多变体(freethreaded 等)按版本去重
fn parse_uv_python_list(text: &str) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    text.lines()
        .filter_map(|line| {
            let token = line.split_whitespace().next()?;
            let is_python = token.starts_with("cpython-") || token.starts_with("pypy-");
            let version = extract_semver(token)?;
            is_python
                .then_some(version)
                .filter(|v| seen.insert(v.clone()))
        })
        .collect()
}

/// `uv python list --only-downloads` 行如
/// `cpython-3.14.4+freethreaded-windows-x86_64-none    <download available>`:
/// 取首 token(cpython-/pypy- 前缀)的版本段——保留 3.15.0a8 这类预发布后缀,
/// 安装时按原样请求;freethreaded 等变体与普通版同号,按版本去重
fn parse_uv_python_remote(text: &str) -> Vec<ToolchainRemoteVersion> {
    let mut seen: HashSet<String> = HashSet::new();
    text.lines()
        .filter_map(|line| {
            let token = line.split_whitespace().next()?;
            let rest = token
                .strip_prefix("cpython-")
                .or_else(|| token.strip_prefix("pypy-"))?;
            let version = rest.split(['-', '+']).next()?.to_string();
            seen.insert(version.clone())
                .then_some(ToolchainRemoteVersion {
                    name: version,
                    tag: None,
                })
        })
        .collect()
}

/// 从 `uv python find` 输出的解释器路径提取版本:
/// uv 托管 `...\uv\python\cpython-3.12.7-windows-x86_64-none\python.exe`;
/// Windows 系统 `...\Programs\Python\Python312\python.exe`;unix `/usr/bin/python3.11`
fn python_version_from_path(text: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?:cpython|pypy)-(\d+\.\d+\.\d+)|[Pp]ython3?(\d{2})[\\/]|python3\.(\d+)")
            .unwrap()
    });
    let caps = re.captures(text)?;
    if let Some(m) = caps.get(1) {
        return Some(m.as_str().to_string());
    }
    if let Some(m) = caps.get(2) {
        // Windows 目录命名 Python312 → "3.12"(3? 已吃掉主版本 3)
        return Some(format!("3.{}", m.as_str()));
    }
    caps.get(3).map(|m| format!("3.{}", m.as_str()))
}

/// 从 `gh auth status` 输出提取当前登录用户名:
/// ```text
/// github.com
///   ✓ Logged in to github.com account octocat (keyring)
///   - Active account: true
/// ```
/// 多账号时 Active account: true 跟在其账号行的下一行,配对取 active;
/// 没有任何 active 标记时退回首个;未登录返回 None
fn parse_gh_auth_status(text: &str) -> Option<String> {
    let mut first: Option<String> = None;
    let mut last: Option<String> = None;
    let mut active: Option<String> = None;
    for line in text.lines() {
        if let Some(rest) = line.split("account ").nth(1) {
            // 用户名后的 "(keyring)" 等标注以空格分隔,取首个 token
            if let Some(name) = rest.split_whitespace().next() {
                first.get_or_insert_with(|| name.to_string());
                last = Some(name.to_string());
            }
        } else if line.contains("Active account: true") {
            // 标记行总在对应账号行之后,取最近一条
            if let Some(name) = last.take() {
                active = Some(name);
            }
        }
    }
    active.or(first)
}

/// unix nvm 兜底:~/.nvm 存在即视为已安装,版本 = versions/node 子目录,
/// 当前版本尽力读 alias/default 别名文件(仅数字形态可对上,`lts/*` 等放弃)
#[cfg(not(windows))]
fn fill_unix_nvm(status: &mut ToolchainStatus) {
    let Some(home) = user_home_path() else {
        return;
    };
    let root = home.join(".nvm");
    if !root.is_dir() {
        return;
    }
    let mut names: Vec<String> = child_dir_names(&root.join("versions").join("node"))
        .into_iter()
        .filter_map(|n| {
            let bare = n.trim_start_matches('v').to_string();
            bare.starts_with(|c: char| c.is_ascii_digit())
                .then_some(bare)
        })
        .collect();
    names.sort_by(|a, b| natural_version_cmp(a, b));
    let alias = std::fs::read_to_string(root.join("alias").join("default"))
        .unwrap_or_default()
        .trim()
        .trim_start_matches('v')
        .to_string();
    // 别名可能是完整版本或仅主版本("18"),后者对到该主版本下最新的一个
    let current_name = if let Some(exact) = names.iter().find(|n| **n == alias) {
        Some(exact.clone())
    } else if let Some(prefix) = names
        .iter()
        .filter(|n| n.starts_with(&format!("{alias}.")))
        .max()
    {
        Some(prefix.clone())
    } else {
        None
    };
    status.found = true;
    status.version = None;
    status.path = Some(display_path(&root));
    status.source = Some("standalone".to_string());
    status.versions = names
        .into_iter()
        .map(|name| ToolchainVersion {
            current: Some(name) == current_name,
            name,
        })
        .collect();
}

#[cfg(not(windows))]
fn child_dir_names(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// 简易版本排序:逐段按数字比较,段数不同时段多者视为更新("22.11" > "22.9"、"22.11.1" > "22.11")
fn natural_version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |s: &str| -> Vec<u64> { s.split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    parse(a).cmp(&parse(b))
}

fn user_home_path() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

// ── 操作能力判定(与 resolve_op 共用,保证按钮可见性与命令可执行一致) ──────

/// nvm / fnm / dotnet / git / gh 这类「可经包管理器升级卸载」的工具:
/// Windows 默认可(winget 按 ARP 登记匹配,安装器型包不落在 WinGet 目录也能升级);
/// macOS 仅 brew 来源;Linux 无统一包管理器,不提供。
fn manageable(source: Option<&str>) -> bool {
    if cfg!(windows) {
        true
    } else if cfg!(target_os = "macos") {
        source == Some("brew")
    } else {
        false
    }
}

/// rustup 的在场情况:决定被管理工具(rustc/cargo)的安装入口是否开放
fn caps_for(id: &str, found: bool, source: Option<&str>, rustup_found: bool) -> ToolchainCaps {
    match id {
        // rustup / vp 自带升级与卸载子命令,与安装来源无关;远端可装列表只有 vp 有;
        // rustup 不做版本切换展示(工具链由 rustup update 统一维护),vp 支持
        "rustup" | "vp" => ToolchainCaps {
            can_install: !found,
            can_update: found,
            can_uninstall: found,
            can_switch: id == "vp" && found,
            can_list_remote: id == "vp" && found,
        },
        // rustc/cargo 随 rustup 工具链走,自身无独立装卸
        "rustc" | "cargo" => ToolchainCaps {
            can_install: !found && !rustup_found,
            can_update: found && rustup_found,
            can_uninstall: false,
            can_switch: false,
            can_list_remote: false,
        },
        // uv 行承载 python 版本管理:版本区(装/卸/切)与远端可装列表随 uv 在场开放
        "uv" => ToolchainCaps {
            can_install: !found,
            can_update: found,
            can_uninstall: found,
            can_switch: found,
            can_list_remote: found,
        },
        // nvm/fnm:版本管理;dotnet/git/gh:无
        "nvm" | "fnm" => {
            let manageable = found && manageable(source);
            ToolchainCaps {
                can_install: !found,
                can_update: manageable,
                can_uninstall: manageable,
                can_switch: found,
                // fnm 有 `fnm list-remote`;nvm 由 detect 循环按二进制在场修正
                can_list_remote: id == "fnm" && found,
            }
        }
        _ => {
            let manageable = found && manageable(source);
            ToolchainCaps {
                can_install: !found,
                can_update: manageable,
                can_uninstall: manageable,
                can_switch: false,
                can_list_remote: false,
            }
        }
    }
}

// ── 操作命令解析 ────────────────────────────────────────────────────────────

fn winget(action: &str, id: &str) -> String {
    format!("winget {action} --id {id} -e")
}

fn unsupported(tool: &str, op: &str) -> AppError {
    AppError::coded(ErrorCode::ToolchainOpUnsupported, format!("{tool} {op}"))
}

/// 版本/工具链参数只放行 [A-Za-z0-9._/-]:值会原样拼进终端命令,防注入
fn sanitize_version<'a>(version: &'a str) -> AppResult<&'a str> {
    let ok = !version.is_empty()
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'));
    if ok {
        Ok(version)
    } else {
        Err(AppError::coded(
            ErrorCode::ToolchainVersionInvalid,
            version.to_string(),
        ))
    }
}

/// 把 (tool, op, version, source) 解析为要在终端执行的命令串。
/// source 为操作时刻重新探测的安装来源(轻量,仅 where/which 不跑版本命令),
/// 保证与检测时刻判定一致。
fn resolve_op(
    tool: &str,
    op: &str,
    version: Option<&str>,
    source: Option<&str>,
) -> AppResult<String> {
    let version = match version.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) => Some(sanitize_version(v)?),
        None => None,
    };
    let need_version = || version.ok_or_else(|| unsupported(tool, op));
    match tool {
        "rustup" => match op {
            "install" => Ok(if cfg!(windows) {
                winget("install", "Rustlang.Rustup")
            } else if cfg!(target_os = "macos") {
                "brew install rustup".to_string()
            } else {
                r#"curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"#.to_string()
            }),
            "update" => Ok("rustup update".to_string()),
            "uninstall" => Ok("rustup self uninstall -y".to_string()),
            _ => Err(unsupported(tool, op)),
        },
        // rustc/cargo 的安装与更新都经 rustup(装 rustup / 更新工具链)
        "rustc" | "cargo" => match op {
            "install" => Ok(if cfg!(windows) {
                winget("install", "Rustlang.Rustup")
            } else if cfg!(target_os = "macos") {
                "brew install rustup".to_string()
            } else {
                r#"curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"#.to_string()
            }),
            "update" => Ok("rustup update".to_string()),
            _ => Err(unsupported(tool, op)),
        },
        // uv 安装走官方脚本(uv 文档推荐方式):脚本装的 uv 在 ~/.local/bin,
        // 可 uv self update;版本级操作即 python 版本管理(--default 额外创建
        // python/python3 可执行别名,即「设为全局」)
        "uv" => match op {
            "install" => Ok(if cfg!(windows) {
                r#"powershell -ExecutionPolicy ByPass -c "irm https://astral.sh/uv/install.ps1 | iex""#
                    .to_string()
            } else {
                "curl -LsSf https://astral.sh/uv/install.sh | sh".to_string()
            }),
            // 包管理来源(winget/brew)的 uv 被 self update 拒绝,走对应包管理升级
            "update" => Ok(if source == Some("winget") {
                winget("upgrade", "astral-sh.uv")
            } else if source == Some("brew") {
                "brew upgrade uv".to_string()
            } else {
                "uv self update".to_string()
            }),
            "uninstall" => Ok(match source {
                Some("winget") => winget("uninstall", "astral-sh.uv"),
                Some("brew") => "brew uninstall uv".to_string(),
                // cargo 装的 uv 在 ~/.cargo/bin,官方脚本的清理路径删不到
                Some("rustup") => "cargo uninstall uv".to_string(),
                // 官方脚本安装无包管理记录:清缓存并删除二进制(uv 文档的卸载步骤)
                _ if cfg!(windows) => {
                    r#"uv cache clean & del /f "%USERPROFILE%\.local\bin\uv.exe" "%USERPROFILE%\.local\bin\uvx.exe""#
                        .to_string()
                }
                _ => "uv cache clean; rm -f \"$HOME/.local/bin/uv\" \"$HOME/.local/bin/uvx\""
                    .to_string(),
            }),
            "use" => Ok(format!("uv python install {} --default", need_version()?)),
            "install_version" => Ok(format!("uv python install {}", need_version()?)),
            "uninstall_version" => Ok(format!("uv python uninstall {}", need_version()?)),
            _ => Err(unsupported(tool, op)),
        },
        "nvm" => match op {
            "install" => Ok(if cfg!(windows) {
                winget("install", "CoreyButler.NVMforWindows")
            } else if cfg!(target_os = "macos") {
                "brew install nvm".to_string()
            } else {
                "curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.3/install.sh | bash"
                    .to_string()
            }),
            "update" | "uninstall" => {
                if cfg!(windows) {
                    Ok(winget(
                        if op == "update" {
                            "upgrade"
                        } else {
                            "uninstall"
                        },
                        "CoreyButler.NVMforWindows",
                    ))
                } else if cfg!(target_os = "macos") && source == Some("brew") {
                    Ok(format!(
                        "brew {} nvm",
                        if op == "update" {
                            "upgrade"
                        } else {
                            "uninstall"
                        }
                    ))
                } else {
                    Err(unsupported(tool, op))
                }
            }
            // nvm-windows 的 use 改全局符号链接;unix 的 use 只影响当前 shell,
            // 全局默认要写 default 别名
            "use" => Ok(if cfg!(windows) {
                format!("nvm use {}", need_version()?)
            } else {
                format!("nvm alias default {}", need_version()?)
            }),
            "install_version" => Ok(format!("nvm install {}", need_version()?)),
            "uninstall_version" => Ok(format!("nvm uninstall {}", need_version()?)),
            _ => Err(unsupported(tool, op)),
        },
        "fnm" => match op {
            "install" => Ok(if cfg!(windows) {
                winget("install", "Schniz.fnm")
            } else if cfg!(target_os = "macos") {
                "brew install fnm".to_string()
            } else {
                "curl -fsSL https://fnm.vercel.app/install | bash".to_string()
            }),
            "update" | "uninstall" => {
                if cfg!(windows) {
                    Ok(winget(
                        if op == "update" {
                            "upgrade"
                        } else {
                            "uninstall"
                        },
                        "Schniz.fnm",
                    ))
                } else if cfg!(target_os = "macos") && source == Some("brew") {
                    Ok(format!(
                        "brew {} fnm",
                        if op == "update" {
                            "upgrade"
                        } else {
                            "uninstall"
                        }
                    ))
                } else {
                    Err(unsupported(tool, op))
                }
            }
            // fnm 的 use 只影响当前 shell,全局默认走 default 别名
            "use" => Ok(format!("fnm default {}", need_version()?)),
            "install_version" => Ok(format!("fnm install {}", need_version()?)),
            "uninstall_version" => Ok(format!("fnm uninstall {}", need_version()?)),
            _ => Err(unsupported(tool, op)),
        },
        "vp" => match op {
            "install" => Ok(if cfg!(windows) {
                // 显式调 powershell:默认终端 profile 可能是 cmd,irm/iex 不可用
                r#"powershell -NoProfile -Command "irm https://vite.plus/ps1 | iex""#.to_string()
            } else {
                "curl -fsSL https://vite.plus | bash".to_string()
            }),
            "update" => Ok("vp upgrade".to_string()),
            "uninstall" => Ok("vp implode".to_string()),
            // vp env use 仅当前会话,全局默认走 env default
            "use" => Ok(format!("vp env default {}", need_version()?)),
            "install_version" => Ok(format!("vp env install {}", need_version()?)),
            "uninstall_version" => Ok(format!("vp env uninstall {}", need_version()?)),
            _ => Err(unsupported(tool, op)),
        },
        "dotnet" => match op {
            // version 为目标大版本("8"/"9"/"10"),前端用下拉收敛
            "install" => {
                let major = need_version()?;
                Ok(if cfg!(windows) {
                    winget("install", &format!("Microsoft.DotNet.SDK.{major}"))
                } else if cfg!(target_os = "macos") {
                    "brew install --cask dotnet-sdk".to_string()
                } else {
                    format!("curl -sSL https://dot.net/v1/dotnet-install.sh | bash -s -- --channel {major}")
                })
            }
            // 对最高已装大版本升级/卸载(SDK 多版本并存,winget id 按大版本区分)
            "update" | "uninstall" => {
                let action = if op == "update" {
                    "upgrade"
                } else {
                    "uninstall"
                };
                if cfg!(windows) {
                    let major = dotnet_highest_major().ok_or_else(|| unsupported(tool, op))?;
                    Ok(winget(action, &format!("Microsoft.DotNet.SDK.{major}")))
                } else if cfg!(target_os = "macos") && source == Some("brew") {
                    Ok(format!("brew {action} --cask dotnet-sdk"))
                } else {
                    Err(unsupported(tool, op))
                }
            }
            // dotnet 无全局版本切换概念(按项目走 global.json)
            _ => Err(unsupported(tool, op)),
        },
        "git" => match op {
            "install" => Ok(if cfg!(windows) {
                winget("install", "Git.Git")
            } else if cfg!(target_os = "macos") {
                "brew install git".to_string()
            } else {
                "sudo apt-get install -y git".to_string()
            }),
            "update" | "uninstall" => {
                if cfg!(windows) {
                    Ok(winget(
                        if op == "update" {
                            "upgrade"
                        } else {
                            "uninstall"
                        },
                        "Git.Git",
                    ))
                } else if cfg!(target_os = "macos") && source == Some("brew") {
                    Ok(format!(
                        "brew {} git",
                        if op == "update" {
                            "upgrade"
                        } else {
                            "uninstall"
                        }
                    ))
                } else {
                    Err(unsupported(tool, op))
                }
            }
            _ => Err(unsupported(tool, op)),
        },
        "gh" => match op {
            "install" => Ok(if cfg!(windows) {
                winget("install", "GitHub.cli")
            } else if cfg!(target_os = "macos") {
                "brew install gh".to_string()
            } else {
                "sudo apt-get install -y gh".to_string()
            }),
            // 未登录时的登录指引:交互式流程在终端里完成
            "login" => Ok("gh auth login".to_string()),
            "update" | "uninstall" => {
                if cfg!(windows) {
                    Ok(winget(
                        if op == "update" {
                            "upgrade"
                        } else {
                            "uninstall"
                        },
                        "GitHub.cli",
                    ))
                } else if cfg!(target_os = "macos") && source == Some("brew") {
                    Ok(format!(
                        "brew {} gh",
                        if op == "update" {
                            "upgrade"
                        } else {
                            "uninstall"
                        }
                    ))
                } else {
                    Err(unsupported(tool, op))
                }
            }
            _ => Err(unsupported(tool, op)),
        },
        _ => Err(unsupported(tool, op)),
    }
}

/// 操作时刻读取 `dotnet --list-sdks` 取最高已装大版本
fn dotnet_highest_major() -> Option<String> {
    let exe = cli_hits_on_path("dotnet").into_iter().next()?;
    let Some((true, out)) = run_with_timeout(&exe, &["--list-sdks"]) else {
        return None;
    };
    parse_dotnet_sdks(&out)
        .iter()
        .filter_map(|v| v.name.split('.').next().and_then(|m| m.parse::<u32>().ok()))
        .max()
        .map(|major| major.to_string())
}

// ── Tauri 命令包装 ──────────────────────────────────────────────────────────

/// 检测全部内置工具链工具(设置页「工具链」面板)。串行子进程探测总时长
/// 可能到秒级,放入 spawn_blocking 避免阻塞主线程。
#[tauri::command]
pub async fn detect_toolchains() -> AppResult<Vec<ToolchainStatus>> {
    tokio::task::spawn_blocking(detect_toolchains_blocking)
        .await
        .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))
}

/// 获取版本管理器可安装的远端版本列表(「添加版本」选择器的数据源),按版本降序:
/// nvm-windows 解析 `nvm list available` 表格(列头自带 LTS/UNSTABLE 语义);
/// fnm/vp 走各自的 list-remote(vp 的 LTS 行带 `(代号)` 后缀);
/// uv 行的远端列表是可下载的 python 版本:`uv python list --only-downloads`。
/// 走网络,耗时秒级。
#[tauri::command]
pub async fn list_toolchain_versions(tool: String) -> AppResult<Vec<ToolchainRemoteVersion>> {
    tokio::task::spawn_blocking(move || list_toolchain_versions_blocking(&tool))
        .await
        .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))?
}

fn list_toolchain_versions_blocking(tool: &str) -> AppResult<Vec<ToolchainRemoteVersion>> {
    let unsupported_err = || {
        Err(AppError::coded(
            ErrorCode::ToolchainOpUnsupported,
            format!("{tool} list_remote"),
        ))
    };
    let run = |exe: Option<PathBuf>, args: &[&str]| exe.and_then(|e| run_with_timeout(&e, args));
    let parsed = match tool {
        "uv" => run(
            cli_hits_on_path("uv").into_iter().next(),
            &["python", "list", "--only-downloads"],
        ),
        // unix nvm 是 shell 函数拉不了;windows 的 nvm list available 是表格
        "nvm" => {
            #[cfg(windows)]
            let exe = cli_hits_on_path("nvm").into_iter().next();
            #[cfg(not(windows))]
            let exe: Option<PathBuf> = None;
            run(exe, &["list", "available"])
        }
        "fnm" => run(cli_hits_on_path("fnm").into_iter().next(), &["list-remote"]),
        "vp" => run(
            cli_hits_on_path("vp").into_iter().next(),
            &["env", "list-remote"],
        ),
        _ => None,
    };
    let Some((true, out)) = parsed else {
        return unsupported_err();
    };
    let mut versions = match tool {
        "nvm" => parse_nvm_available_table(&out),
        "vp" => parse_vp_remote(&out),
        // uv 的行带 cpython-/pypy- 前缀,通用 token 解析不认,走专用解析
        "uv" => parse_uv_python_remote(&out),
        // fnm 的列表无 LTS 标注,不加 tag
        _ => parse_remote_tokens(&out)
            .into_iter()
            .map(|name| ToolchainRemoteVersion { name, tag: None })
            .collect(),
    };
    versions.sort_by(|a, b| natural_version_cmp(&b.name, &a.name));
    Ok(versions)
}

/// nvm-windows `nvm list available` 表格(实测):
/// ```text
/// |   CURRENT    |     LTS      |  OLD STABLE  | OLD UNSTABLE |
/// |--------------|--------------|--------------|--------------|
/// |    26.7.0    |   24.19.0    |   0.12.18    |   0.11.16    |
/// ```
/// 列头文字直接作为该列版本的标记原样透传前端。表头行按「全部单元格
/// 是 CURRENT/LTS/STABLE/UNSTABLE 这类措辞」识别,列头措辞随版本有差异。
fn parse_nvm_available_table(text: &str) -> Vec<ToolchainRemoteVersion> {
    let mut column_tags: Vec<String> = Vec::new();
    let mut versions = Vec::new();
    for line in text.lines() {
        let cells: Vec<&str> = line
            .split('|')
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .collect();
        if cells.is_empty() {
            continue;
        }
        if column_tags.is_empty() {
            // 表头行:全部是 CURRENT/LTS/STABLE/UNSTABLE 这类措辞,无版本号
            if cells.iter().all(|c| {
                ["CURRENT", "LTS", "STABLE", "UNSTABLE"]
                    .iter()
                    .any(|k| c.contains(k))
            }) {
                column_tags = cells.iter().map(|c| c.to_string()).collect();
            }
            continue;
        }
        // 数据行:单元格数对不上列头(换页/提示行)时跳过
        if cells.len() != column_tags.len() {
            continue;
        }
        for (cell, tag) in cells.iter().zip(&column_tags) {
            if let Some(name) = version_token(cell) {
                versions.push(ToolchainRemoteVersion {
                    name,
                    tag: Some(tag.clone()),
                });
            }
        }
    }
    versions
}

/// vp `vp env list-remote`(实测):LTS 版本行带 `(代号)` 后缀(如
/// `v18.12.0 (Hydrogen)`),其余为非 LTS 的 Current 线;先剥 ANSI 颜色码
fn parse_vp_remote(text: &str) -> Vec<ToolchainRemoteVersion> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\x1b\[[0-9;]*m").unwrap());
    re.replace_all(text, "")
        .lines()
        .filter_map(|line| {
            let token = line.split_whitespace().next()?;
            let name = version_token(token)?;
            let tag = if line.contains('(') { "LTS" } else { "Current" };
            Some(ToolchainRemoteVersion {
                name,
                tag: Some(tag.to_string()),
            })
        })
        .collect()
}

/// fnm `fnm list-remote` 这类「行首 token 为版本号」的列表
fn parse_remote_tokens(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let token = line.split_whitespace().next()?;
            version_token(token)
        })
        .collect()
}

/// 执行工具链管理操作:解析出命令串后在系统终端新窗口执行(跑完不关),
/// 终端里可见进度与报错(安装可能要 UAC,更新走网络)。
#[tauri::command]
pub fn toolchain_op(tool: String, op: String, version: Option<String>) -> AppResult<()> {
    let tool = tool.trim();
    let op = op.trim();
    if TOOLS.iter().any(|t| t.id == tool) {
        let source = cli_hits_on_path(tool)
            .into_iter()
            .next()
            .as_ref()
            .map(|p| classify_source(p));
        let command = resolve_op(tool, op, version.as_deref(), source.as_deref())?;
        let home = user_home_path()
            .map(|p| display_path(&p))
            .unwrap_or_else(|| ".".to_string());
        return spawn_terminal(&home, &format!("Toolchain: {tool}"), Some(&command));
    }
    Err(unsupported(tool, op))
}

// ── 单元测试 ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ver(name: &str, current: bool) -> ToolchainVersion {
        ToolchainVersion {
            name: name.to_string(),
            current,
        }
    }

    #[test]
    fn extracts_semver_from_real_outputs() {
        assert_eq!(
            extract_semver("git version 2.44.0.windows.1").as_deref(),
            Some("2.44.0")
        );
        assert_eq!(
            extract_semver("rustc 1.77.0 (aedd173a2 2024-03-17)").as_deref(),
            Some("1.77.0")
        );
        assert_eq!(
            extract_semver("rustup 1.27.1 (dd91c1e4b 2024-04-17)").as_deref(),
            Some("1.27.1")
        );
        assert_eq!(
            extract_semver("uv 0.5.11 (7e988cdcd 2024-12-16)").as_deref(),
            Some("0.5.11")
        );
        assert_eq!(
            extract_semver("gh version 2.63.0 (2024-11-27)\nhttps://github.com").as_deref(),
            Some("2.63.0")
        );
        assert_eq!(extract_semver("fnm 1.38.1").as_deref(), Some("1.38.1"));
        assert_eq!(extract_semver("8.0.204").as_deref(), Some("8.0.204"));
        assert_eq!(extract_semver("no digits here"), None);
    }

    #[test]
    fn parses_nvm_windows_list() {
        let out = parse_nvm_list(
            "    18.20.4\n  * 20.11.1 (Currently using 64-bit executable)\nNoVersionsInstalledYet? visit https://github.com/coreybutler/nvm-windows.\n",
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], ver("18.20.4", false));
        assert_eq!(out[1], ver("20.11.1", true));
    }

    #[test]
    fn parses_fnm_list() {
        let out = parse_token_versions("* v18.20.4 default\n  v20.11.1\n  system\n");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], ver("18.20.4", true));
        assert_eq!(out[1], ver("20.11.1", false));
    }

    #[test]
    fn parses_vp_env_list_tolerantly() {
        // 实测格式:每行 `*` 项目符号,当前行尾带 current 且混 ANSI 颜色码,
        // 末尾 note 提示行应被版本形态过滤
        let out = parse_vp_env_list(
            "* v24.15.0\n* v24.16.0\n* v24.17.0\n* v24.18.1\n\u{1b}[94m* v24.19.0 \u{1b}[2mcurrent\u{1b}[0m\u{1b}[39m\n\nnote: Run `vp env clean` to free disk space from unused managed runtimes and package manager caches.\n",
        );
        assert_eq!(out.len(), 5);
        assert_eq!(out[0], ver("24.15.0", false));
        assert_eq!(out[4], ver("24.19.0", true));
        assert!(
            out[..4].iter().all(|v| !v.current),
            "只有 current 行应标记: {out:?}"
        );
        assert!(parse_vp_env_list("no versions here").is_empty());
    }

    #[test]
    fn parses_dotnet_sdks() {
        let out = parse_dotnet_sdks(
            "8.0.204 [C:\\Program Files\\dotnet\\sdk]\n10.0.100 [C:\\Program Files\\dotnet\\sdk]\n",
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], ver("8.0.204", false));
        assert_eq!(out[1], ver("10.0.100", false));
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_nvm_dir_versions_and_default_alias() {
        let dir = std::env::temp_dir().join(format!("repomeow-nvm-{}", std::process::id()));
        let versions = dir.join(".nvm").join("versions").join("node");
        std::fs::create_dir_all(versions.join("v18.20.4")).unwrap();
        std::fs::create_dir_all(versions.join("v20.11.1")).unwrap();
        std::fs::create_dir_all(dir.join(".nvm").join("alias")).unwrap();
        // 别名只写主版本时,应对到该主版本下最新的一个
        std::fs::write(dir.join(".nvm").join("alias").join("default"), "18").unwrap();
        let mut status = ToolchainStatus {
            id: "nvm".to_string(),
            kind: ToolchainKind::Node,
            found: false,
            version: None,
            path: None,
            source: None,
            versions: Vec::new(),
            caps: ToolchainCaps {
                can_install: false,
                can_update: false,
                can_uninstall: false,
                can_switch: false,
                can_list_remote: false,
            },
        };
        std::env::set_var("USERPROFILE", &dir);
        fill_unix_nvm(&mut status);
        std::env::remove_var("USERPROFILE");
        assert!(status.found);
        assert_eq!(status.versions.len(), 2);
        assert_eq!(status.versions[0], ver("18.20.4", true));
        assert!(!status.versions[1].current);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn version_param_is_sanitized() {
        assert!(sanitize_version("22").is_ok());
        assert!(sanitize_version("v22.11.0").is_ok());
        assert!(sanitize_version("stable-x86_64-pc-windows-msvc").is_ok());
        assert!(sanitize_version("lts/hydro").is_ok());
        assert!(sanitize_version("22 && rm -rf ~").is_err());
        assert!(sanitize_version("22; calc").is_err());
        assert!(sanitize_version("").is_err());
        let err = resolve_op("nvm", "use", Some("22 && calc"), None).unwrap_err();
        assert!(err.is_code(ErrorCode::ToolchainVersionInvalid));
    }

    #[test]
    fn unknown_tool_or_op_is_unsupported() {
        let err = resolve_op("npm", "update", None, None).unwrap_err();
        assert!(err.is_code(ErrorCode::ToolchainOpUnsupported));
        let err = resolve_op("dotnet", "use", Some("8"), None).unwrap_err();
        assert!(err.is_code(ErrorCode::ToolchainOpUnsupported));
        let err = resolve_op("nvm", "use", None, None).unwrap_err();
        assert!(err.is_code(ErrorCode::ToolchainOpUnsupported));
    }

    #[cfg(windows)]
    #[test]
    fn resolves_windows_matrix() {
        // rustup 全套;不做版本切换展示(use/装卸工具链不再开放)
        assert_eq!(
            resolve_op("rustup", "install", None, None).unwrap(),
            "winget install --id Rustlang.Rustup -e"
        );
        assert_eq!(
            resolve_op("rustup", "update", None, None).unwrap(),
            "rustup update"
        );
        assert_eq!(
            resolve_op("rustup", "uninstall", None, None).unwrap(),
            "rustup self uninstall -y"
        );
        assert!(resolve_op("rustup", "use", Some("stable"), None).is_err());
        // rustc/cargo 只开放安装(=装 rustup)与更新(=rustup update)
        assert_eq!(
            resolve_op("cargo", "update", None, None).unwrap(),
            "rustup update"
        );
        assert!(resolve_op("rustc", "uninstall", None, None).is_err());
        // uv:安装走官方 PowerShell 脚本;winget 源走 winget 升级/卸载,
        // cargo 源走 cargo uninstall,standalone 走 self update 与缓存清理+删二进制
        assert_eq!(
            resolve_op("uv", "install", None, None).unwrap(),
            r#"powershell -ExecutionPolicy ByPass -c "irm https://astral.sh/uv/install.ps1 | iex""#
        );
        assert_eq!(
            resolve_op("uv", "update", None, Some("winget")).unwrap(),
            "winget upgrade --id astral-sh.uv -e"
        );
        assert_eq!(
            resolve_op("uv", "update", None, Some("standalone")).unwrap(),
            "uv self update"
        );
        assert_eq!(
            resolve_op("uv", "uninstall", None, Some("winget")).unwrap(),
            "winget uninstall --id astral-sh.uv -e"
        );
        assert_eq!(
            resolve_op("uv", "uninstall", None, Some("rustup")).unwrap(),
            "cargo uninstall uv"
        );
        assert_eq!(
            resolve_op("uv", "uninstall", None, Some("standalone")).unwrap(),
            r#"uv cache clean & del /f "%USERPROFILE%\.local\bin\uv.exe" "%USERPROFILE%\.local\bin\uvx.exe""#
        );
        // nvm:windows 上 use 是全局符号链接切换
        assert_eq!(
            resolve_op("nvm", "use", Some("22.11.0"), None).unwrap(),
            "nvm use 22.11.0"
        );
        // fnm/vp 的全局默认走 default 别名
        assert_eq!(
            resolve_op("fnm", "use", Some("20"), None).unwrap(),
            "fnm default 20"
        );
        assert_eq!(
            resolve_op("vp", "use", Some("lts/hydro"), None).unwrap(),
            "vp env default lts/hydro"
        );
        // vp 安装显式走 powershell(默认终端 profile 可能是 cmd)
        assert_eq!(
            resolve_op("vp", "install", None, None).unwrap(),
            r#"powershell -NoProfile -Command "irm https://vite.plus/ps1 | iex""#
        );
        assert_eq!(
            resolve_op("vp", "uninstall", None, None).unwrap(),
            "vp implode"
        );
        // git/gh/dotnet 安装
        assert_eq!(
            resolve_op("git", "install", None, None).unwrap(),
            "winget install --id Git.Git -e"
        );
        assert_eq!(
            resolve_op("dotnet", "install", Some("10"), None).unwrap(),
            "winget install --id Microsoft.DotNet.SDK.10 -e"
        );
    }

    #[test]
    fn caps_match_resolvable_ops() {
        // 运行时不变量:按钮可见(caps)时,以同一来源调 resolve_op 必须能解析出命令;
        // 反向不要求——命令串可能永远可拼(如 `rustup update`),按钮隐藏即不可达。
        // 操作时刻来源与检测时刻一致(found=false 时探不到路径,source=None)。
        let check = |id: &str, caps: ToolchainCaps, source: Option<&str>| {
            for (op, need_version) in [
                ("install", false),
                ("update", false),
                ("uninstall", false),
                ("use", true),
                ("install_version", true),
                ("uninstall_version", true),
            ] {
                let resolvable = resolve_op(id, op, need_version.then_some("22"), source).is_ok();
                let visible = match op {
                    "install" => caps.can_install,
                    "update" => caps.can_update,
                    "uninstall" => caps.can_uninstall,
                    _ => caps.can_switch,
                };
                assert!(
                    !visible || resolvable,
                    "{id} {op}: 按钮可见但命令不可解析(source={source:?})"
                );
            }
        };
        check(
            "uv",
            caps_for("uv", true, Some("standalone"), false),
            Some("standalone"),
        );
        check("uv", caps_for("uv", false, None, false), None);
        check("rustup", caps_for("rustup", true, None, true), None);
        check("rustup", caps_for("rustup", false, None, false), None);
        check(
            "rustc",
            caps_for("rustc", true, Some("rustup"), true),
            Some("rustup"),
        );
        check(
            "nvm",
            caps_for("nvm", true, Some("standalone"), false),
            Some("standalone"),
        );
        check(
            "fnm",
            caps_for("fnm", true, Some("standalone"), false),
            Some("standalone"),
        );
        check(
            "vp",
            caps_for("vp", true, Some("standalone"), false),
            Some("standalone"),
        );
        check(
            "git",
            caps_for("git", true, Some("standalone"), false),
            Some("standalone"),
        );
        check("gh", caps_for("gh", false, None, false), None);
    }

    #[test]
    fn resolves_uv_python_version_ops() {
        // uv 行的版本级操作即 python 版本管理;--default 创建 python/python3 全局别名
        assert_eq!(
            resolve_op("uv", "use", Some("3.12"), None).unwrap(),
            "uv python install 3.12 --default"
        );
        assert_eq!(
            resolve_op("uv", "install_version", Some("3.11.9"), None).unwrap(),
            "uv python install 3.11.9"
        );
        assert_eq!(
            resolve_op("uv", "uninstall_version", Some("3.10"), None).unwrap(),
            "uv python uninstall 3.10"
        );
    }

    #[test]
    fn parses_uv_python_list() {
        let out = parse_uv_python_list(
            "cpython-3.12.7-windows-x86_64-none    C:\\Users\\x\\AppData\\Roaming\\uv\\python\\cpython-3.12.7-windows-x86_64-none\\python.exe\n\
             cpython-3.13.1t-windows-x86_64-none   C:\\Users\\x\\AppData\\Roaming\\uv\\python\\cpython-3.13.1t-windows-x86_64-none\\python.exe\n\
             cpython-3.13.1-windows-x86_64-none    C:\\Users\\x\\AppData\\Roaming\\uv\\python\\cpython-3.13.1-windows-x86_64-none\\python.exe\n\
             cpython-3.10.8-windows-x86_64-none    C:\\Python310\\python.exe\n\
             pypy-3.9.19-windows-x86_64-none       C:\\Users\\x\\AppData\\Roaming\\uv\\python\\pypy-3.9.19-windows-x86_64-none\\pypy.exe\n",
        );
        // freethreaded 变体(3.13.1t)与普通版同版本号,按版本去重
        assert_eq!(
            out,
            vec![
                "3.12.7".to_string(),
                "3.13.1".to_string(),
                "3.10.8".to_string(),
                "3.9.19".to_string(),
            ]
        );
        assert!(parse_uv_python_list("not-a-python-line").is_empty());
    }

    #[test]
    fn parses_uv_python_remote() {
        let out = parse_uv_python_remote(
            "cpython-3.15.0a8-windows-x86_64-none                 <download available>\n\
             cpython-3.14.4+freethreaded-windows-x86_64-none      <download available>\n\
             cpython-3.14.4-windows-x86_64-none                   <download available>\n\
             pypy-3.11.13-windows-x86_64-none                     <download available>\n",
        );
        let names: Vec<&str> = out.iter().map(|v| v.name.as_str()).collect();
        // freethreaded 与普通版同号去重;预发布后缀原样保留(安装按全版本号请求)
        assert_eq!(names, vec!["3.15.0a8", "3.14.4", "3.11.13"]);
        assert!(parse_uv_python_remote("v22.11.0").is_empty());
    }

    #[test]
    fn extracts_python_version_from_find_output() {
        // uv 托管
        assert_eq!(
            python_version_from_path(
                r"C:\Users\x\AppData\Roaming\uv\python\cpython-3.12.7-windows-x86_64-none\python.exe"
            )
            .as_deref(),
            Some("3.12.7")
        );
        // Windows 官方安装器目录
        assert_eq!(
            python_version_from_path(
                r"C:\Users\x\AppData\Local\Programs\Python\Python312\python.exe"
            )
            .as_deref(),
            Some("3.12")
        );
        // unix 命名
        assert_eq!(
            python_version_from_path("/usr/bin/python3.11").as_deref(),
            Some("3.11")
        );
        assert_eq!(python_version_from_path("no version here"), None);
    }

    fn remote(name: &str, tag: Option<&str>) -> ToolchainRemoteVersion {
        ToolchainRemoteVersion {
            name: name.to_string(),
            tag: tag.map(str::to_string),
        }
    }

    #[test]
    fn parses_nvm_available_table() {
        // 实测格式:列头文字直接作为该列版本的标记原样透传
        let out = parse_nvm_available_table(
            "|   CURRENT    |     LTS      |  OLD STABLE  | OLD UNSTABLE |\n\
             |--------------|--------------|--------------|--------------|\n\
             |    26.7.0    |   24.19.0    |   0.12.18    |   0.11.16    |\n\
             |    26.6.0    |   24.18.1    |   0.12.17    |   0.11.15    |\n",
        );
        assert_eq!(out.len(), 8);
        assert_eq!(out[0], remote("26.7.0", Some("CURRENT")));
        assert_eq!(out[1], remote("24.19.0", Some("LTS")));
        assert_eq!(out[2], remote("0.12.18", Some("OLD STABLE")));
        assert_eq!(out[3], remote("0.11.16", Some("OLD UNSTABLE")));
        // 列头措辞随版本有差异(OLD LTS),原样透传不做映射
        let out = parse_nvm_available_table(
            "|   CURRENT    |     LTS      |  OLD STABLE  |  OLD LTS  |\n\
             |    22.11.0   |   20.18.1    |   18.20.5    |  16.20.2  |\n",
        );
        assert_eq!(out[3], remote("16.20.2", Some("OLD LTS")));
        assert!(parse_nvm_available_table("no table here").is_empty());
    }

    #[test]
    fn parses_vp_remote_with_lts_codenames() {
        // 实测格式:LTS 行带 (代号) 后缀,其余为 Current 线;末尾 note 行被过滤
        let out = parse_vp_remote(
            "  v26.7.0\n  v26.6.0\n  v24.19.0 (Krypton)\n\u{1b}[1m\u{1b}[2mnote:\u{1b}[0m\u{1b}[0m Run `vp env clean`...\n",
        );
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], remote("26.7.0", Some("Current")));
        assert_eq!(out[2], remote("24.19.0", Some("LTS")));
    }

    #[test]
    fn parses_remote_token_lists() {
        // fnm list-remote 形态(无 LTS 标注,tag 由调用方置 None)
        let out = parse_remote_tokens("v22.11.0\nv22.10.0\nv20.18.1\n");
        assert_eq!(
            out,
            vec![
                "22.11.0".to_string(),
                "22.10.0".to_string(),
                "20.18.1".to_string()
            ]
        );
        // 非 token 行忽略
        let out = parse_remote_tokens("22.11.0\n20.18.1\nlts\nnode\n");
        assert_eq!(out, vec!["22.11.0".to_string(), "20.18.1".to_string()]);
    }

    #[test]
    fn natural_version_ordering() {
        let mut v = vec!["20.9.0", "22.11.0", "22.10.0", "3.12.7"];
        v.sort_by(|a, b| natural_version_cmp(b, a));
        assert_eq!(v, vec!["22.11.0", "22.10.0", "20.9.0", "3.12.7"]);
    }

    #[test]
    fn parses_gh_auth_status() {
        // 单账号已登录
        assert_eq!(
            parse_gh_auth_status(
                "github.com\n  ✓ Logged in to github.com account octocat (keyring)\n  - Active account: true\n"
            )
            .as_deref(),
            Some("octocat")
        );
        // 多账号:active 标记配对其上一条账号行
        assert_eq!(
            parse_gh_auth_status(
                "github.com\n  ✓ Logged in to github.com account other (keyring)\n  - Active account: false\n  ✓ Logged in to github.com account octocat (keyring)\n  - Active account: true\n"
            )
            .as_deref(),
            Some("octocat")
        );
        // 已登录但没有 active 标记(旧版输出),退回首个
        assert_eq!(
            parse_gh_auth_status("  ✓ Logged in to github.com account octocat (keyring)\n")
                .as_deref(),
            Some("octocat")
        );
        // 未登录
        assert_eq!(
            parse_gh_auth_status("You are not logged into any GitHub hosts.").as_deref(),
            None
        );
    }

    #[test]
    fn registry_ids_are_unique() {
        let mut seen = HashSet::new();
        for spec in TOOLS {
            assert!(seen.insert(spec.id), "重复的工具 id: {}", spec.id);
        }
    }
}
