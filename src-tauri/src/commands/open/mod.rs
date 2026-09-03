use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::models::EditorKind;

/// 执行命令所用的终端 shell(对应前端 settings.json 的 `terminal` 键,仅 Windows 生效)。

mod editor;
mod shell;
mod terminal;
#[cfg(test)]
mod tests;

pub use editor::*;
pub use shell::*;
pub use terminal::*;
pub(crate) use editor::open_explorer;
pub(crate) use shell::{resolve_shell, ShellKind};
pub(crate) use terminal::find_wt;

/// detect_editors 结果在 settings 表中的缓存 key(JSON: { "<kind>": bool })
const EDITORS_SETTING_KEY: &str = "editors_available";

/// 命令类编辑器登记表:(kind, 前端 id, CLI 命令名)。
/// 可用性只用 where/which 查 PATH,不扫描任何安装目录;
/// explorer / terminal 不在表内(平台特判,无需探测)。
const EDITOR_CLI_TABLE: &[(EditorKind, &str, &str)] = &[
    (EditorKind::Vscode, "vscode", "code"),
    (EditorKind::Cursor, "cursor", "cursor"),
    (EditorKind::Windsurf, "windsurf", "windsurf"),
    (EditorKind::Trae, "trae", "trae"),
    (EditorKind::Vscodium, "vscodium", "codium"),
    (EditorKind::Zed, "zed", "zed"),
    (EditorKind::Sublime, "sublime", "subl"),
    (EditorKind::Idea, "idea", "idea"),
    (EditorKind::Webstorm, "webstorm", "webstorm"),
    (EditorKind::Goland, "goland", "goland"),
    (EditorKind::Pycharm, "pycharm", "pycharm"),
    (EditorKind::Clion, "clion", "clion"),
    (EditorKind::Rustrover, "rustrover", "rustrover"),
];

pub(crate) fn cli_command(kind: EditorKind) -> Option<&'static str> {
    EDITOR_CLI_TABLE
        .iter()
        .find(|(k, _, _)| *k == kind)
        .map(|(_, _, cli)| *cli)
}

/// Windows 下隐藏中间进程的控制台黑窗(最终弹出的终端窗口不受影响)
pub(crate) fn hidden(#[allow(unused_mut)] mut cmd: Command) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

