use std::collections::HashMap;
use std::process::Command;
use tauri::{AppHandle, State};
use crate::db::{self, Db};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::EditorKind;
use super::*;
/// 通过编辑器 CLI 打开目录(命令需在 PATH 中)

pub(super) fn open_editor(cli: &str, path: &str) -> AppResult<()> {
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

/// 渲染用户配置的打开命令。{path} 经环境变量(REPOMEOW_OPEN_PATH)传入 shell,
/// 文本中只保留对应的引用占位符,避免直接拼接用户路径;
/// {line} 走文本内插(参数类型 u32,无注入面)。
/// 未使用 {path} 时在命令末尾追加引用后的路径占位符,保留简洁配置方式。
pub(super) fn render_custom_open_command(command: &str, line: Option<u32>) -> String {
    let path_var = if cfg!(windows) {
        "\"%REPOMEOW_OPEN_PATH%\""
    } else {
        "\"$REPOMEOW_OPEN_PATH\""
    };
    let line = line
        .filter(|n| *n > 0)
        .map_or_else(String::new, |n| n.to_string());
    let has_path = command.contains("{path}");
    let rendered = command.replace("{path}", path_var).replace("{line}", &line);
    if has_path {
        rendered
    } else {
        format!("{rendered} {path_var}")
    }
}

#[cfg(windows)]
pub(super) fn custom_open_cmdline(rendered: &str) -> String {
    format!("/C {rendered}")
}

/// 用用户配置的 shell 命令打开文件或目录。命令模板支持 {path} 与 {line};
/// 不含 {path} 时自动在末尾追加目标路径(以 %REPOMEOW_OPEN_PATH% 占位符引用,
/// 实际路径在子进程 env 中)。
/// 入口用 path_util::clean 归一化(去尾斜杠、统一分隔符),与 STATUS_CACHE / WALK_CACHE
/// 等共享 key 规范,避免 ad-hoc 路径写法绕过 exists 校验。
#[tauri::command]
pub fn open_with_custom_command(path: String, command: String, line: Option<u32>) -> AppResult<()> {
    // 路径归一化后再校验存在性;空串、纯空白、未归一化形态都进同一闸口
    let normalized = crate::path_util::clean_str(&path);
    if normalized.is_empty() || !std::path::Path::new(&normalized).exists() {
        return Err(AppError::coded(ErrorCode::InvalidPath, path));
    }
    let command = command.trim();
    if command.is_empty() {
        return Err(AppError::coded(ErrorCode::CustomOpenCommandRequired, ""));
    }
    let rendered = render_custom_open_command(command, line);
    #[cfg(windows)]
    {
        // `args(["/C", &rendered])` 会把整条 `code "路径"` 再包成一个参数,
        // cmd 随后按自己的首尾引号规则重解释,导致裸 CLI 命令收不到路径。
        // raw_arg 保留 `/C <完整命令>` 的原始命令尾部,与用户在 cmd 中手动输入一致。
        hidden(Command::new("cmd"))
            .raw_arg(custom_open_cmdline(&rendered))
            .env("REPOMEOW_OPEN_PATH", &normalized)
            .spawn()?;
    }
    #[cfg(not(windows))]
    {
        Command::new("sh")
            .args(["-c", &rendered])
            .env("REPOMEOW_OPEN_PATH", &normalized)
            .spawn()?;
    }
    Ok(())
}

/// 纯 PATH 探测:Windows 用 where,其他平台用 which。
/// 不用 `<cli> --version` —— JetBrains 系启动器收到 --version 可能直接拉起 GUI。
pub(super) fn command_on_path(cli: &str) -> bool {
    #[cfg(windows)]
    let probe = hidden(Command::new("where")).arg(cli).output();
    #[cfg(not(windows))]
    let probe = Command::new("which").arg(cli).output();
    matches!(probe, Ok(out) if out.status.success())
}

#[tauri::command]
pub fn open_with(app: AppHandle, path: String, kind: EditorKind) -> AppResult<()> {
    if !std::path::Path::new(&path).is_dir() {
        return Err(AppError::coded(ErrorCode::InvalidPath, path));
    }
    match kind {
        EditorKind::Explorer => open_explorer(&path),
        EditorKind::Terminal => spawn_terminal(&path, "Terminal", None, resolve_shell(&app)),
        other => match cli_command(other) {
            Some(cli) => open_editor(cli, &path),
            None => Err(AppError::coded(
                ErrorCode::OpenMethodUnknown,
                format!("{other:?}"),
            )),
        },
    }
}

/// 用指定编辑器打开文件(可选行号定位)或目录,提交详情"在 IDE 打开"用。
/// 与 open_with 的区别:允许文件路径;行号语法按各编辑器 CLI 约定构造,
/// 不支持行号的编辑器降级为仅打开文件;explorer 选中文件、terminal 在父目录开终端
#[tauri::command]
pub fn open_in_editor(
    app: AppHandle,
    path: String,
    kind: EditorKind,
    line: Option<u32>,
) -> AppResult<()> {
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
            spawn_terminal(dir, "Terminal", None, resolve_shell(&app))
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
            None => Err(AppError::coded(
                ErrorCode::OpenMethodUnknown,
                format!("{other:?}"),
            )),
        },
    }
}

/// 编辑器 CLI 的打开参数:带行号时按各 CLI 语法拼接,无行号时仅传路径。
/// VSCode 系用 `-g file:line`(-g 复用已有窗口);Zed / Sublime 直接 `file:line`;
/// JetBrains 系启动器用 `--line <n> <path>`
pub(super) fn editor_open_args(kind: EditorKind, path: &str, line: Option<u32>) -> Vec<String> {
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
pub(super) fn open_explorer_reveal(path: &str) -> AppResult<()> {
    // explorer /select, 对正斜杠路径处理不佳,统一为反斜杠(前端拼接 git 相对路径用的是 "/")
    Command::new("explorer")
        .arg(format!(
            "/select,{}",
            crate::path_util::to_native_separator(path)
        ))
        .spawn()?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub(super) fn open_explorer_reveal(path: &str) -> AppResult<()> {
    Command::new("open").arg("-R").arg(path).spawn()?;
    Ok(())
}

#[cfg(all(not(windows), not(target_os = "macos")))]
pub(super) fn open_explorer_reveal(path: &str) -> AppResult<()> {
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

