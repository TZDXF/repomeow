//! 「更新」按钮的自动检测:按各工具的更新通道(winget / rustup / GitHub Release / brew)
//! 查询远端版本,判定 可更新 / 已最新 / 无法判定(Unknown 时前端退化为点击即更新)。

use std::path::Path;
use std::time::Duration;

use crate::models::{ToolchainUpdateInfo, ToolchainUpdateState};

use super::dotnet;
use super::process::{classify_source, cli_hits_on_path, run_with_timeout_in};
use super::version::extract_semver;

/// 联网探测(winget / GitHub / brew / rustup check)允许比本地探测更长的超时
const REMOTE_CHECK_TIMEOUT: Duration = Duration::from_secs(20);

const UNKNOWN: (ToolchainUpdateState, Option<String>) = (ToolchainUpdateState::Unknown, None);

pub(super) fn check_toolchain_update_blocking(tool: &str) -> ToolchainUpdateInfo {
    let (state, latest) = detect_update_state(tool);
    ToolchainUpdateInfo {
        tool: tool.to_string(),
        state,
        latest,
    }
}

fn detect_update_state(tool: &str) -> (ToolchainUpdateState, Option<String>) {
    let Some(exe) = cli_hits_on_path(tool).into_iter().next() else {
        return UNKNOWN;
    };
    let source = classify_source(&exe);
    match tool {
        // rustup 系:rustup check 同时报告 rustup 自身与全部工具链(rustc/cargo 随之更新)
        "rustup" | "rustc" | "cargo" => rustup_check(tool),
        // uv:包管理器安装的走包管理器检测,独立安装(安装脚本)的用自带 dry-run 预检
        "uv" if source == "winget" => winget_check("astral-sh.uv"),
        "uv" if source == "brew" => brew_check(&["outdated", "--quiet", "uv"]),
        "uv" => uv_self_check(&exe),
        // vp 暂无只读查询通道(vp upgrade 即升级),无法预判
        "vp" => UNKNOWN,
        _ => {
            if cfg!(windows) {
                winget_id(tool).map_or(UNKNOWN, |id| winget_check(&id))
            } else if source == "brew" {
                match tool {
                    "nvm" | "fnm" | "git" | "gh" => {
                        brew_check(&["outdated", "--quiet", tool])
                    }
                    "dotnet" => brew_check(&["outdated", "--quiet", "--cask", "dotnet-sdk"]),
                    _ => UNKNOWN,
                }
            } else {
                UNKNOWN
            }
        }
    }
}

/// winget 安装的 pwsh/git/gh/nvm/fnm/dotnet,winget id 与 toolchain_op 的更新命令保持一致
fn winget_id(tool: &str) -> Option<String> {
    let id = match tool {
        "pwsh" => "Microsoft.PowerShell".to_string(),
        "git" => "Git.Git".to_string(),
        "gh" => "GitHub.cli".to_string(),
        "nvm" => "CoreyButler.NVMforWindows".to_string(),
        "fnm" => "Schniz.fnm".to_string(),
        // dotnet 更新针对已装的最高大版本 SDK,与 dotnet::resolve 的 update 一致
        "dotnet" => format!("Microsoft.DotNet.SDK.{}", dotnet::highest_major()?),
        _ => return None,
    };
    Some(id)
}

/// winget list 的输出含「可用」列即代表可更新;表头随系统语言变化,改为按数据行解析
fn winget_check(winget_id: &str) -> (ToolchainUpdateState, Option<String>) {
    let args = [
        "list",
        "--id",
        winget_id,
        "-e",
        "--disable-interactivity",
        "--accept-source-agreements",
    ];
    let Some((true, output)) = run_with_timeout_in(Path::new("winget"), &args, REMOTE_CHECK_TIMEOUT)
    else {
        return UNKNOWN;
    };
    parse_winget_list_row(&output, winget_id)
}

/// winget list 数据行:<名称> <ID> <当前版本> [可用版本] [源];ID 之后第二个版本号即「可用」
pub(super) fn parse_winget_list_row(output: &str, winget_id: &str) -> (ToolchainUpdateState, Option<String>) {
    for line in output.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let Some(pos) = tokens.iter().position(|token| *token == winget_id) else {
            continue;
        };
        let versions: Vec<&str> = tokens[pos + 1..]
            .iter()
            .copied()
            .filter(|token| is_plain_version(token))
            .collect();
        return match versions.as_slice() {
            [] => UNKNOWN,
            [_current] => (ToolchainUpdateState::UpToDate, None),
            [_, latest, ..] => (
                ToolchainUpdateState::UpdateAvailable,
                Some((*latest).to_string()),
            ),
        };
    }
    UNKNOWN
}

/// 纯数字+点的版本号(2.51.0 / 10.0.100);用于排除 winget 源列等杂项
fn is_plain_version(token: &str) -> bool {
    let token = token.strip_prefix('v').unwrap_or(token);
    token.contains('.')
        && token.starts_with(|c: char| c.is_ascii_digit())
        && token.chars().all(|c| c.is_ascii_digit() || c == '.')
}

fn rustup_check(tool: &str) -> (ToolchainUpdateState, Option<String>) {
    let Some(exe) = cli_hits_on_path("rustup").into_iter().next() else {
        return UNKNOWN;
    };
    let Some((true, output)) = run_with_timeout_in(&exe, &["check"], REMOTE_CHECK_TIMEOUT) else {
        return UNKNOWN;
    };
    parse_rustup_check(&output, tool)
}

/// `rustup check` 输出:`<工具链|rustup> - up to date(:| :) x` 或 `... - update available: a -> b`;
/// 大小写与冒号间距随 rustup 版本不一(旧版首字母大写),统一小写匹配;
/// rustup 行看 rustup 自身,rustc/cargo 看工具链行(任一可更新即提示)
pub(super) fn parse_rustup_check(output: &str, tool: &str) -> (ToolchainUpdateState, Option<String>) {
    let mut up_to_date = false;
    for line in output.lines() {
        let Some((name, rest)) = line.split_once(" - ") else {
            continue;
        };
        if (tool == "rustup") != (name.trim() == "rustup") {
            continue;
        }
        let lower = rest.to_lowercase();
        if lower.contains("update available") {
            let latest = rest
                .split_once("->")
                .and_then(|(_, tail)| tail.split_whitespace().next())
                .map(str::to_string);
            return (ToolchainUpdateState::UpdateAvailable, latest);
        }
        if lower.contains("up to date") {
            up_to_date = true;
        }
    }
    if up_to_date {
        (ToolchainUpdateState::UpToDate, None)
    } else {
        UNKNOWN
    }
}

/// 独立安装的 uv:`uv self update --dry-run` 与真实更新命令同源(自带 receipt 校验、
/// UV_GITHUB_TOKEN 等处理),比直接调 GitHub API 更不易被限流
fn uv_self_check(exe: &Path) -> (ToolchainUpdateState, Option<String>) {
    let args = ["self", "update", "--dry-run"];
    let Some((true, output)) = run_with_timeout_in(exe, &args, REMOTE_CHECK_TIMEOUT) else {
        return UNKNOWN;
    };
    parse_uv_self_update_dry_run(&output)
}

/// dry-run 输出:可更新 "Would update uv from a to b";
/// 已最新 "You're already on version vx of uv (the latest version)"
pub(super) fn parse_uv_self_update_dry_run(output: &str) -> (ToolchainUpdateState, Option<String>) {
    let lower = output.to_lowercase();
    if lower.contains("would update") {
        let latest = output.rsplit(" to ").next().and_then(extract_semver);
        return (ToolchainUpdateState::UpdateAvailable, latest);
    }
    if lower.contains("already on version") || lower.contains("the latest version") {
        return (ToolchainUpdateState::UpToDate, None);
    }
    UNKNOWN
}

/// brew 通道:`brew outdated --quiet` 有输出即代表可更新(不解析目标版本)
fn brew_check(args: &[&str]) -> (ToolchainUpdateState, Option<String>) {
    #[cfg(windows)]
    {
        let _ = args;
        UNKNOWN
    }
    #[cfg(not(windows))]
    {
        let Some((true, output)) = run_with_timeout_in(Path::new("brew"), args, REMOTE_CHECK_TIMEOUT)
        else {
            return UNKNOWN;
        };
        if output.trim().is_empty() {
            (ToolchainUpdateState::UpToDate, None)
        } else {
            (ToolchainUpdateState::UpdateAvailable, None)
        }
    }
}
