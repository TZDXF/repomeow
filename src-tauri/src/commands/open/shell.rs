use serde::Serialize;
use tauri::{AppHandle};
use crate::error::{AppResult};
use super::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ShellKind {
    Cmd,
    PowerShell,
    GitBash,
}

impl ShellKind {
    pub(super) fn from_setting(value: Option<String>) -> Self {
        match value.as_deref() {
            Some("powershell") => Self::PowerShell,
            Some("gitbash") => Self::GitBash,
            _ => Self::Cmd,
        }
    }

    /// 多行命令摊平时的顺序分隔符:cmd 用 ` & `,其余 shell 用 `; `
    #[cfg(windows)]
    pub(super) fn separator(self) -> &'static str {
        match self {
            Self::Cmd => " & ",
            Self::PowerShell | Self::GitBash => "; ",
        }
    }
}

/// 前端设置页展示的三种命令解释器可用性。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TerminalShellCapabilities {
    cmd: bool,
    powershell: bool,
    gitbash: bool,
}

/// 当前平台的命令终端能力；字段名固定与前端契约保持一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalCapabilities {
    is_windows: bool,
    windows_terminal: bool,
    shells: TerminalShellCapabilities,
}

/// 构造可序列化的能力结果。非 Windows 平台强制把全部能力置为 false，避免调用方
/// 误把构建机或测试注入的探测结果当成可用终端。
pub(super) fn build_terminal_capabilities(
    is_windows: bool,
    windows_terminal: bool,
    cmd: bool,
    powershell: bool,
    gitbash: bool,
) -> TerminalCapabilities {
    TerminalCapabilities {
        is_windows,
        windows_terminal: is_windows && windows_terminal,
        shells: TerminalShellCapabilities {
            cmd: is_windows && cmd,
            powershell: is_windows && powershell,
            gitbash: is_windows && gitbash,
        },
    }
}

/// 请求的 shell 不可用时统一回退 Cmd。保持为纯函数，便于覆盖“提前回退”语义；
/// 调用方须先完成对应 shell 的实际探测。
#[cfg(any(windows, test))]
pub(super) fn fallback_to_cmd_if_unavailable(requested: ShellKind, available: bool) -> ShellKind {
    if available {
        requested
    } else {
        ShellKind::Cmd
    }
}

/// 从 settings.json 读取终端选择(与 closeAction 同一读取通道,执行时才读,天然拿到最新值)，
/// 并在命令被包装成 shell 专属语法之前完成可用性回退。
#[cfg(windows)]
pub(crate) fn resolve_shell(app: &AppHandle) -> ShellKind {
    let requested = ShellKind::from_setting(crate::tray::read_setting_string(app, "terminal"));
    let available = match requested {
        ShellKind::Cmd => true,
        ShellKind::PowerShell => find_powershell().is_some(),
        ShellKind::GitBash => find_git_bash().is_some(),
    };
    let resolved = fallback_to_cmd_if_unavailable(requested, available);
    if resolved != requested {
        eprintln!("[open] 配置的命令终端不可用，回退到 cmd 执行");
    }
    resolved
}

/// 非 Windows 平台无终端选择,固定返回占位值(spawn_terminal 会忽略)
#[cfg(not(windows))]
pub(crate) fn resolve_shell(_app: &AppHandle) -> ShellKind {
    ShellKind::Cmd
}

/// 实时探测设置页需要的终端能力，不写入持久缓存。
#[tauri::command]
pub fn detect_terminal_capabilities() -> AppResult<TerminalCapabilities> {
    #[cfg(windows)]
    let capabilities = build_terminal_capabilities(
        true,
        find_wt().is_some(),
        cmd_available(),
        find_powershell().is_some(),
        find_git_bash().is_some(),
    );
    #[cfg(not(windows))]
    let capabilities = build_terminal_capabilities(false, false, false, false, false);

    Ok(capabilities)
}

