use std::collections::HashMap;
use std::process::Command;

use tauri::State;

use crate::db::{self, Db};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::EditorKind;

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

/// 在系统终端打开目录,可选执行命令(跑完不关窗口)。
/// 优先用 Windows Terminal(wt.exe);未安装或启动失败时退回 cmd。
#[cfg(windows)]
pub fn spawn_terminal(path: &str, title: &str, command: Option<&str>) -> AppResult<()> {
    let command = flatten_multiline(command, " & ");
    let command = command.as_deref();
    if let Some(wt) = find_wt() {
        // wt 是 GUI 子系统进程,自己创建窗口,不存在句柄透传问题,直接启动即可
        let spawned = Command::new(wt)
            .args(build_wt_args(path, title, command))
            .spawn();
        if spawned.is_ok() {
            return Ok(());
        }
        // 启动失败(如 AppX 执行别名被用户禁用)则继续走下面的 cmd 兜底
    }
    // 结构:cmd /C start "<title>" cmd /K "<inner>",外层 cmd 用 CREATE_NO_WINDOW 隐藏。
    //
    // 为什么不能直接 CREATE_NEW_CONSOLE 起 cmd /K:Rust 的 Command 会把父进程
    // (Tauri 应用)的标准句柄透传给子进程——即使子进程拿到了新控制台,它的
    // stdout/stderr 仍指向父进程的句柄(dev 终端或管道),命令照常执行但新窗口里
    // 看不到任何输出。而 start 拉起目标进程时不透传句柄,cmd /K 会拿到全新控制台
    // 的输入/输出句柄,输出正常显示。(已用 marker 实验证实两种方式的句柄流向)
    //
    // 引号解析(已实测):
    // - 外层 cmd /C 的命令串首字符是 's',不触发首尾引号剥离;
    // - `&&` 位于 cmd /K 的引号串内,不会在外层被顶层切分,整串交给 start;
    // - start 把第一个引号串当窗口标题,其余原样交给新进程;
    // - 内层 cmd /K 首字符是引号,剥掉首尾引号后得到 `cd /d "<path>" && <command>`,
    //   在同一窗口内依次执行,跑完窗口保留。
    let cmdline = build_start_cmdline(path, title, command);
    use std::os::windows::process::CommandExt;
    hidden(Command::new("cmd")).raw_arg(&cmdline).spawn()?;
    Ok(())
}

/// 构造外层 cmd 的命令串:`/C start "<title>" cmd /K "<inner>"`
#[cfg(windows)]
fn build_start_cmdline(path: &str, title: &str, command: Option<&str>) -> String {
    // title 是展示文本,剥掉 cmd 元字符,避免打乱引号配对;path 剥掉双引号
    let title = sanitize_cmd_text(title);
    let path = path.replace('"', "");
    let inner = match command {
        Some(c) => format!("cd /d \"{path}\" && {c}"),
        None => format!("cd /d \"{path}\""),
    };
    format!("/C start \"{title}\" cmd /K \"{inner}\"")
}

/// 多行命令摊平成单行:cmd /k 与 AppleScript 的 do script 都无法携带换行
/// (换行即终结当前命令串,只有第一行会被执行)。
/// 逐行 trim、丢弃空行后用顺序分隔符连接(cmd 用 ` & `、sh 用 `; `),
/// 语义等同在终端逐行回车:前一条失败不阻断后续(故不用 `&&`)。
/// 单行命令原样借用返回,不产生分配。
#[cfg(any(windows, target_os = "macos"))]
fn flatten_multiline<'a>(command: Option<&'a str>, sep: &str) -> Option<std::borrow::Cow<'a, str>> {
    use std::borrow::Cow;
    let c = command?;
    if !c.contains(['\n', '\r']) {
        return Some(Cow::Borrowed(c));
    }
    let joined = c
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(sep);
    Some(Cow::Owned(joined))
}

/// 剥掉会破坏 cmd 命令行解析的元字符(仅用于 title 这类展示文本;
/// 用户命令原样透传,允许包含 && | 等 shell 操作符)
#[cfg(windows)]
fn sanitize_cmd_text(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '"' | '&' | '|' | '<' | '>' | '^'))
        .collect()
}

/// 定位 wt.exe:先查 AppX 执行别名(Store / 官网安装都会注册),
/// 再用 where 搜 PATH(覆盖 scoop / choco / 绿色版等安装方式);找不到返回 None
#[cfg(windows)]
pub(crate) fn find_wt() -> Option<String> {
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let alias = format!(r"{local}\Microsoft\WindowsApps\wt.exe");
        if std::path::Path::new(&alias).exists() {
            return Some(alias);
        }
    }
    let probe = hidden(Command::new("where")).arg("wt").output();
    match probe {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

/// 构造 wt 参数:`wt --title "<title>" -d "<path>" [cmd /k "<command>"]`
/// 不带命令时由 wt 打开默认配置文件的 shell;带命令时套 cmd /k,跑完窗口保留
#[cfg(windows)]
fn build_wt_args(path: &str, title: &str, command: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "--title".into(),
        sanitize_cmd_text(title),
        "-d".into(),
        path.replace('"', ""),
    ];
    if let Some(c) = command {
        args.extend(["cmd".into(), "/k".into(), c.into()]);
    }
    args
}

#[cfg(target_os = "macos")]
pub fn spawn_terminal(path: &str, _title: &str, command: Option<&str>) -> AppResult<()> {
    let command = flatten_multiline(command, "; ");
    let command = command.as_deref();
    let inner = match command {
        Some(c) => format!(
            "cd '{}' && {}",
            path.replace('\'', "'\\''"),
            c.replace('"', "\\\"")
        ),
        None => format!("cd '{}'", path.replace('\'', "'\\''")),
    };
    let script = format!("tell application \"Terminal\" to do script \"{inner}\"");
    Command::new("osascript").args(["-e", &script]).spawn()?;
    Ok(())
}

#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn spawn_terminal(_path: &str, _title: &str, _command: Option<&str>) -> AppResult<()> {
    Err(AppError::coded(ErrorCode::TerminalNotSupported, ""))
}

/// 通过编辑器 CLI 打开目录(命令需在 PATH 中)
fn open_editor(cli: &str, path: &str) -> AppResult<()> {
    #[cfg(windows)]
    hidden(Command::new("cmd"))
        .args(["/C", cli, path])
        .spawn()?;
    #[cfg(not(windows))]
    Command::new(cli).arg(path).spawn()?;
    Ok(())
}

pub(crate) fn open_explorer(path: &str) -> AppResult<()> {
    #[cfg(windows)]
    Command::new("explorer").arg(path).spawn()?;
    #[cfg(target_os = "macos")]
    Command::new("open").arg(path).spawn()?;
    #[cfg(all(not(windows), not(target_os = "macos")))]
    return Err(AppError::coded(ErrorCode::FileManagerNotSupported, ""));
    Ok(())
}

/// 纯 PATH 探测:Windows 用 where,其他平台用 which。
/// 不用 `<cli> --version` —— JetBrains 系启动器收到 --version 可能直接拉起 GUI。
fn command_on_path(cli: &str) -> bool {
    #[cfg(windows)]
    let probe = hidden(Command::new("where")).arg(cli).output();
    #[cfg(not(windows))]
    let probe = Command::new("which").arg(cli).output();
    matches!(probe, Ok(out) if out.status.success())
}

#[tauri::command]
pub fn open_with(path: String, kind: EditorKind) -> AppResult<()> {
    if !std::path::Path::new(&path).is_dir() {
        return Err(AppError::coded(ErrorCode::InvalidPath, path));
    }
    match kind {
        EditorKind::Explorer => open_explorer(&path),
        EditorKind::Terminal => spawn_terminal(&path, "Terminal", None),
        other => match cli_command(other) {
            Some(cli) => open_editor(cli, &path),
            None => Err(AppError::coded(ErrorCode::OpenMethodUnknown, format!("{other:?}"))),
        },
    }
}

/// 用指定编辑器打开文件(可选行号定位)或目录,提交详情"在 IDE 打开"用。
/// 与 open_with 的区别:允许文件路径;行号语法按各编辑器 CLI 约定构造,
/// 不支持行号的编辑器降级为仅打开文件;explorer 选中文件、terminal 在父目录开终端
#[tauri::command]
pub fn open_in_editor(path: String, kind: EditorKind, line: Option<u32>) -> AppResult<()> {
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err(AppError::coded(ErrorCode::InvalidPath, path));
    }
    match kind {
        EditorKind::Explorer => {
            if p.is_file() {
                open_explorer_reveal(&path)
            } else {
                open_explorer(&path)
            }
        }
        EditorKind::Terminal => {
            let dir = if p.is_dir() {
                path.as_str()
            } else {
                p.parent().and_then(|d| d.to_str()).unwrap_or(&path)
            };
            spawn_terminal(dir, "Terminal", None)
        }
        other => match cli_command(other) {
            Some(cli) => {
                let args = editor_open_args(other, &path, line);
                #[cfg(windows)]
                {
                    let mut full = vec!["/C".to_string(), cli.to_string()];
                    full.extend(args);
                    hidden(Command::new("cmd")).args(&full).spawn()?;
                }
                #[cfg(not(windows))]
                Command::new(cli).args(&args).spawn()?;
                Ok(())
            }
            None => Err(AppError::coded(ErrorCode::OpenMethodUnknown, format!("{other:?}"))),
        },
    }
}

/// 编辑器 CLI 的打开参数:带行号时按各 CLI 语法拼接,无行号时仅传路径。
/// VSCode 系用 `-g file:line`(-g 复用已有窗口);Zed / Sublime 直接 `file:line`;
/// JetBrains 系启动器用 `--line <n> <path>`
fn editor_open_args(kind: EditorKind, path: &str, line: Option<u32>) -> Vec<String> {
    match line {
        Some(n) if n > 0 => match kind {
            EditorKind::Vscode
            | EditorKind::Cursor
            | EditorKind::Windsurf
            | EditorKind::Trae
            | EditorKind::Vscodium => vec!["-g".into(), format!("{path}:{n}")],
            EditorKind::Zed | EditorKind::Sublime => vec![format!("{path}:{n}")],
            _ => vec!["--line".into(), n.to_string(), path.into()],
        },
        _ => vec![path.into()],
    }
}

/// 在文件管理器中选中文件(而非仅打开所在目录)
#[cfg(windows)]
fn open_explorer_reveal(path: &str) -> AppResult<()> {
    // explorer /select, 对正斜杠路径处理不佳,统一为反斜杠(前端拼接 git 相对路径用的是 "/")
    Command::new("explorer")
        .arg(format!("/select,{}", crate::path_util::to_native_separator(path)))
        .spawn()?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn open_explorer_reveal(path: &str) -> AppResult<()> {
    Command::new("open").arg("-R").arg(path).spawn()?;
    Ok(())
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn open_explorer_reveal(path: &str) -> AppResult<()> {
    // 无通用"选中文件"语义,退化为打开所在目录
    let dir = std::path::Path::new(path)
        .parent()
        .and_then(|d| d.to_str())
        .unwrap_or(path);
    open_explorer(dir)
}

/// 探测所有命令类编辑器的 CLI 是否在 PATH 中,结果以 JSON 缓存进 settings(仅首次真实探测)
#[tauri::command]
pub fn detect_editors(db: State<'_, Db>) -> AppResult<HashMap<String, bool>> {
    let conn = db.0.lock().unwrap();
    if let Some(cached) = db::get_setting(&conn, EDITORS_SETTING_KEY)? {
        if let Ok(map) = serde_json::from_str::<HashMap<String, bool>>(&cached) {
            return Ok(map);
        }
    }
    let map: HashMap<String, bool> = EDITOR_CLI_TABLE
        .iter()
        .map(|(_, id, cli)| (id.to_string(), command_on_path(cli)))
        .collect();
    let json = serde_json::to_string(&map).unwrap_or_else(|_| "{}".into());
    db::set_setting(&conn, EDITORS_SETTING_KEY, &json)?;
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn settings_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        assert_eq!(db::get_setting(&conn, "k").unwrap(), None);
        db::set_setting(&conn, "k", "v1").unwrap();
        db::set_setting(&conn, "k", "v2").unwrap();
        assert_eq!(db::get_setting(&conn, "k").unwrap().as_deref(), Some("v2"));
    }

    #[test]
    fn editor_kind_deserialize() {
        // 枚举序列化 id 必须与登记表中的前端 id 一致
        for (kind, id, _) in EDITOR_CLI_TABLE {
            let json = format!("\"{id}\"");
            let parsed: EditorKind = serde_json::from_str(&json).unwrap();
            assert_eq!(&parsed, kind);
        }
        let kind: EditorKind = serde_json::from_str("\"explorer\"").unwrap();
        assert!(matches!(kind, EditorKind::Explorer));
        let kind: EditorKind = serde_json::from_str("\"terminal\"").unwrap();
        assert!(matches!(kind, EditorKind::Terminal));
        assert!(serde_json::from_str::<EditorKind>("\"word\"").is_err());
    }

    #[test]
    fn cli_command_lookup() {
        assert_eq!(cli_command(EditorKind::Vscode), Some("code"));
        assert_eq!(cli_command(EditorKind::Sublime), Some("subl"));
        assert_eq!(cli_command(EditorKind::Rustrover), Some("rustrover"));
        assert_eq!(cli_command(EditorKind::Explorer), None);
        assert_eq!(cli_command(EditorKind::Terminal), None);
    }

    #[test]
    fn editor_open_args_line_syntax_per_kind() {
        // VSCode 系:-g file:line
        assert_eq!(
            editor_open_args(EditorKind::Vscode, r"D:\p\f.ts", Some(12)),
            vec!["-g".to_string(), r"D:\p\f.ts:12".to_string()]
        );
        assert_eq!(
            editor_open_args(EditorKind::Cursor, "/p/f.ts", Some(3)),
            vec!["-g".to_string(), "/p/f.ts:3".to_string()]
        );
        // Zed / Sublime:file:line
        assert_eq!(
            editor_open_args(EditorKind::Zed, "/p/f.ts", Some(3)),
            vec!["/p/f.ts:3".to_string()]
        );
        // JetBrains 系:--line n file
        assert_eq!(
            editor_open_args(EditorKind::Idea, "/p/f.ts", Some(3)),
            vec!["--line".to_string(), "3".to_string(), "/p/f.ts".to_string()]
        );
        // 无行号 / 行号 0:仅传路径
        assert_eq!(
            editor_open_args(EditorKind::Vscode, "/p/f.ts", None),
            vec!["/p/f.ts".to_string()]
        );
        assert_eq!(
            editor_open_args(EditorKind::Vscode, "/p/f.ts", Some(0)),
            vec!["/p/f.ts".to_string()]
        );
    }

    #[test]
    fn command_on_path_does_not_panic() {
        let _available = command_on_path("definitely-not-a-real-editor-cli");
    }

    /// 回归测试:终端必须通过 隐藏外层 cmd + start 启动。
    /// 直接 CREATE_NEW_CONSOLE 起 cmd 时,Rust 会把父进程的标准句柄透传给子进程,
    /// 命令输出写到父进程的 dev 终端/管道,新窗口里什么都看不到;
    /// start 拉起的进程才会拿到全新控制台的输入/输出句柄。
    /// 同时 `&&` 必须位于 cmd /K 的引号串内,避免在外层被顶层切分。
    #[cfg(windows)]
    #[test]
    fn start_cmdline_with_command() {
        let s = build_start_cmdline(r"D:\code\foo", "Project: my-app", Some("npm run dev"));
        assert_eq!(
            s,
            r#"/C start "Project: my-app" cmd /K "cd /d "D:\code\foo" && npm run dev""#
        );
    }

    #[cfg(windows)]
    #[test]
    fn start_cmdline_without_command() {
        let s = build_start_cmdline(r"D:\code\foo bar", "Terminal", None);
        assert_eq!(s, r#"/C start "Terminal" cmd /K "cd /d "D:\code\foo bar"""#);
    }

    #[cfg(windows)]
    #[test]
    fn start_cmdline_sanitizes_display_text_but_keeps_command() {
        // path / title 中的引号与 cmd 元字符必须剥掉,避免打乱引号配对
        let s = build_start_cmdline(r#"D:\weird"path"#, r#"a&b"c"#, Some("echo hi"));
        assert_eq!(
            s,
            r#"/C start "abc" cmd /K "cd /d "D:\weirdpath" && echo hi""#
        );
        // 用户命令原样透传,允许 shell 操作符
        let s = build_start_cmdline(r"D:\p", "t", Some("cargo build && cargo run"));
        assert!(s.ends_with("&& cargo build && cargo run\""));
    }

    #[cfg(windows)]
    #[test]
    fn wt_args_without_command() {
        let args = build_wt_args(r"D:\code\foo bar", "Terminal", None);
        assert_eq!(args, ["--title", "Terminal", "-d", r"D:\code\foo bar"]);
    }

    #[cfg(windows)]
    #[test]
    fn wt_args_with_command_keeps_window() {
        // 带命令时必须套 cmd /k,保证跑完窗口保留
        let args = build_wt_args(r"D:\code\foo", "Project: my-app", Some("npm run dev"));
        assert_eq!(
            args,
            [
                "--title",
                "Project: my-app",
                "-d",
                r"D:\code\foo",
                "cmd",
                "/k",
                "npm run dev"
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn wt_args_sanitizes_display_text_but_keeps_command() {
        let args = build_wt_args(
            r#"D:\weird"path"#,
            r#"a&b"c"#,
            Some("cargo build && cargo run"),
        );
        assert_eq!(args[1], "abc");
        assert_eq!(args[3], r"D:\weirdpath");
        assert_eq!(args[6], "cargo build && cargo run");
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn flatten_multiline_keeps_single_line_borrowed() {
        let c = flatten_multiline(Some("npm run dev"), " & ").unwrap();
        assert_eq!(c, "npm run dev");
        assert!(matches!(c, std::borrow::Cow::Borrowed(_)));
        assert!(flatten_multiline(None, " & ").is_none());
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn flatten_multiline_joins_lines_sequentially() {
        // 逐行 trim、丢弃空行,用顺序分隔符连接(前一条失败不阻断后续)
        let c = flatten_multiline(Some("  cargo build\n\n  cargo run  "), " & ").unwrap();
        assert_eq!(c, "cargo build & cargo run");
        // \r\n 同样处理;sh 语义用 `; `
        let c = flatten_multiline(Some("echo a\r\necho b"), "; ").unwrap();
        assert_eq!(c, "echo a; echo b");
    }

    #[cfg(windows)]
    #[test]
    fn start_cmdline_flattens_multiline_command() {
        let s = build_start_cmdline(
            r"D:\p",
            "t",
            flatten_multiline(Some("echo a\necho b"), " & ")
                .as_deref(),
        );
        assert!(s.ends_with("&& echo a & echo b\""));
    }

    #[cfg(windows)]
    #[test]
    fn find_wt_does_not_panic() {
        let _wt = find_wt();
    }
}
