//! 设置页「工具链」面板:常用开发 CLI 的检测与安装/更新/卸载/版本切换。
//!
//! 检测(detect_toolchains):where/which 找 PATH 上的可执行文件,跑 `--version`
//! 解析版本,版本管理器(rustup/nvm/fnm/vp/dotnet)额外列出可切换的版本;
//! Python 的版本管理挂在 uv 上(uv python install --default 下载并设为全局)。
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
use crate::models::{ToolchainCaps, ToolchainKind, ToolchainStatus, ToolchainVersion};

/// 子进程探测超时:坏掉的工具不应拖住整个检测流程(与 java.rs 的 JDK 探测同理)
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

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
        id: "python",
        kind: ToolchainKind::Python,
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
            "rustup" => run_with_timeout(&exe, &["toolchain", "list"])
                .filter(|(ok, _)| *ok)
                .map(|(_, out)| parse_rustup_toolchains(&out))
                .unwrap_or_default(),
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
                .map(|(_, out)| parse_token_versions(&out))
                .unwrap_or_default(),
            "dotnet" => run_with_timeout(&exe, &["--list-sdks"])
                .filter(|(ok, _)| *ok)
                .map(|(_, out)| parse_dotnet_sdks(&out))
                .unwrap_or_default(),
            _ => Vec::new(),
        };
    }

    // python 的版本管理挂在 uv 上:即便 PATH 上还没有 python(uv --default 别名未建),
    // 也能先经 uv 下载,故不走上面「仅已找到」的循环;uv 未装时 python 行退化为
    // 纯展示(版本+路径),版本区由 caps.can_switch=false 隐藏
    let uv_exe = statuses
        .iter()
        .find(|s| s.id == "uv" && s.found)
        .and_then(|s| s.path.clone())
        .map(PathBuf::from);
    if let (Some(status), Some(uv)) = (
        statuses.iter_mut().find(|s| s.id == "python"),
        uv_exe.as_deref(),
    ) {
        status.versions = uv_python_versions(uv);
    }

    let rustup_found = statuses.iter().any(|s| s.id == "rustup" && s.found);
    let ctx = FamilyCtx {
        rustup_found,
        uv_found: uv_exe.is_some(),
    };
    for status in &mut statuses {
        status.caps = caps_for(&status.id, status.found, status.source.as_deref(), &ctx);
    }
    statuses
}

/// 探测单个工具:PATH 命中 → 来源判定 → 版本探测
fn detect_one(spec: &ToolSpec) -> ToolchainStatus {
    // WindowsApps 下的 python.exe 是 Store 存根(执行会拉起商店页),与 java.rs 跳过
    // java 存根同理;其余工具不经 Store 分发,不受影响
    let hit = cli_hits_on_path(spec.id)
        .into_iter()
        .find(|p| !(spec.id == "python" && cfg!(windows) && p.to_string_lossy().contains("WindowsApps")));
    let mut status = ToolchainStatus {
        id: spec.id.to_string(),
        kind: spec.kind,
        found: hit.is_some(),
        version: None,
        path: hit.as_ref().map(|p| display_path(p)),
        source: hit.as_ref().map(|p| classify_source(p)),
        versions: Vec::new(),
        caps: ToolchainCaps {
            can_install: false,
            can_update: false,
            can_uninstall: false,
            can_switch: false,
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
    let mut child = hidden(Command::new(exe))
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + PROBE_TIMEOUT;
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

/// `rustup toolchain list` 行如 `stable-x86_64-pc-windows-msvc (default)`
fn parse_rustup_toolchains(text: &str) -> Vec<ToolchainVersion> {
    text.lines()
        .filter_map(|line| {
            let name = line.split_whitespace().next()?.to_string();
            if name.is_empty() {
                return None;
            }
            Some(ToolchainVersion {
                name,
                current: line.contains("(default)") || line.contains("(active)"),
            })
        })
        .collect()
}

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

/// fnm `fnm list` / vp `vp env list` 这类「行首 token 为版本号」的列表,
/// `*` 前缀或含 default 字样视为当前版本;解析不出时返回空,前端隐藏版本区。
/// 两者的确切输出格式尚无稳定文档,按共性容错解析。
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
                name == c || name.starts_with(&format!("{c}.")) || c.starts_with(&format!("{name}."))
            });
            ToolchainVersion { current: matched, name }
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

/// 从 `uv python find` 输出的解释器路径提取版本:
/// uv 托管 `...\uv\python\cpython-3.12.7-windows-x86_64-none\python.exe`;
/// Windows 系统 `...\Programs\Python\Python312\python.exe`;unix `/usr/bin/python3.11`
fn python_version_from_path(text: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?:cpython|pypy)-(\d+\.\d+\.\d+)|[Pp]ython3?(\d{2})[\\/]|python3\.(\d+)").unwrap()
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
#[cfg(not(windows))]
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

/// 同族管理器的在场情况:决定被管理工具(rustc/cargo 随 rustup、python 随 uv)的能力
struct FamilyCtx {
    rustup_found: bool,
    uv_found: bool,
}

fn caps_for(id: &str, found: bool, source: Option<&str>, ctx: &FamilyCtx) -> ToolchainCaps {
    match id {
        // rustup / vp 自带升级与卸载子命令,与安装来源无关
        "rustup" | "vp" => ToolchainCaps {
            can_install: !found,
            can_update: found,
            can_uninstall: found,
            can_switch: found,
        },
        // rustc/cargo 随 rustup 工具链走,自身无独立装卸
        "rustc" | "cargo" => ToolchainCaps {
            can_install: !found && !ctx.rustup_found,
            can_update: found && ctx.rustup_found,
            can_uninstall: false,
            can_switch: false,
        },
        // python 的装卸与全局默认都经 uv(--default 建 python/python3 别名);
        // 无 uv 时退化为纯展示,系统自装的 python 不提供管理
        "python" => ToolchainCaps {
            can_install: !found && ctx.uv_found,
            can_update: false,
            can_uninstall: false,
            can_switch: ctx.uv_found,
        },
        "uv" => ToolchainCaps {
            can_install: !found,
            can_update: found,
            can_uninstall: found && (cfg!(windows) || source == Some("brew")),
            can_switch: false,
        },
        // nvm/fnm:版本管理;dotnet/git/gh:无
        "nvm" | "fnm" => {
            let manageable = found && manageable(source);
            ToolchainCaps {
                can_install: !found,
                can_update: manageable,
                can_uninstall: manageable,
                can_switch: found,
            }
        }
        _ => {
            let manageable = found && manageable(source);
            ToolchainCaps {
                can_install: !found,
                can_update: manageable,
                can_uninstall: manageable,
                can_switch: false,
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
/// source 为操作时刻重新探测的安装来源(轻量,仅 where/which 不跑版本命令)。
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
            "use" => Ok(format!("rustup default {}", need_version()?)),
            "install_version" => Ok(format!("rustup toolchain install {}", need_version()?)),
            "uninstall_version" => Ok(format!("rustup toolchain uninstall {}", need_version()?)),
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
        "uv" => match op {
            "install" => Ok(if cfg!(windows) {
                winget("install", "astral-sh.uv")
            } else if cfg!(target_os = "macos") {
                "brew install uv".to_string()
            } else {
                "curl -LsSf https://astral.sh/uv/install.sh | sh".to_string()
            }),
            "update" => Ok(if source == Some("winget") {
                winget("upgrade", "astral-sh.uv")
            } else if cfg!(target_os = "macos") && source == Some("brew") {
                "brew upgrade uv".to_string()
            } else {
                "uv self update".to_string()
            }),
            "uninstall" => {
                if cfg!(windows) {
                    Ok(winget("uninstall", "astral-sh.uv"))
                } else if cfg!(target_os = "macos") && source == Some("brew") {
                    Ok("brew uninstall uv".to_string())
                } else {
                    Err(unsupported(tool, op))
                }
            }
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
    use std::collections::HashSet;

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
    fn parses_rustup_toolchain_list() {
        let out = parse_rustup_toolchains(
            "stable-x86_64-pc-windows-msvc (default)\n1.75.0-x86_64-pc-windows-msvc\nnightly-x86_64-pc-windows-msvc (active)\n",
        );
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], ver("stable-x86_64-pc-windows-msvc", true));
        assert_eq!(out[1], ver("1.75.0-x86_64-pc-windows-msvc", false));
        assert!(out[2].current);
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
        // 格式未知,构造合理样例:标记当前 + 普通 + 表头/提示行应被忽略
        let out = parse_token_versions("* 22.11.0 current\n20.18.0\n18.20.4\nsystem\n");
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], ver("22.11.0", true));
        assert_eq!(out[2], ver("18.20.4", false));
        assert!(parse_token_versions("no versions here").is_empty());
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
        // rustup 全套
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
        assert_eq!(
            resolve_op("rustup", "use", Some("stable-x86_64-pc-windows-msvc"), None).unwrap(),
            "rustup default stable-x86_64-pc-windows-msvc"
        );
        // rustc/cargo 只开放安装(=装 rustup)与更新(=rustup update)
        assert_eq!(
            resolve_op("cargo", "update", None, None).unwrap(),
            "rustup update"
        );
        assert!(resolve_op("rustc", "uninstall", None, None).is_err());
        // uv:winget 源走 winget 升级,standalone 走 self update
        assert_eq!(
            resolve_op("uv", "update", None, Some("winget")).unwrap(),
            "winget upgrade --id astral-sh.uv -e"
        );
        assert_eq!(
            resolve_op("uv", "update", None, Some("standalone")).unwrap(),
            "uv self update"
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
    fn registry_ids_are_unique() {
        let mut seen = HashSet::new();
        for spec in TOOLS {
            assert!(seen.insert(spec.id), "重复的工具 id: {}", spec.id);
        }
    }
}
