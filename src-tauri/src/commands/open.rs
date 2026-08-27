use std::collections::HashMap;
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
use tauri::{AppHandle, State};

use crate::db::{self, Db};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::EditorKind;

/// 执行命令所用的终端 shell(对应前端 settings.json 的 `terminal` 键,仅 Windows 生效)。
/// Cmd 是默认与兜底值:设置缺失/非法、Git Bash 未安装时都回退到这里。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ShellKind {
    Cmd,
    PowerShell,
    GitBash,
}

impl ShellKind {
    fn from_setting(value: Option<String>) -> Self {
        match value.as_deref() {
            Some("powershell") => Self::PowerShell,
            Some("gitbash") => Self::GitBash,
            _ => Self::Cmd,
        }
    }

    /// 多行命令摊平时的顺序分隔符:cmd 用 ` & `,其余 shell 用 `; `
    #[cfg(windows)]
    fn separator(self) -> &'static str {
        match self {
            Self::Cmd => " & ",
            Self::PowerShell | Self::GitBash => "; ",
        }
    }
}

/// 从 settings.json 读取终端选择(与 closeAction 同一读取通道,执行时才读,天然拿到最新值)
#[cfg(windows)]
pub(crate) fn resolve_shell(app: &AppHandle) -> ShellKind {
    ShellKind::from_setting(crate::tray::read_setting_string(app, "terminal"))
}

/// 非 Windows 平台无终端选择,固定返回占位值(spawn_terminal 会忽略)
#[cfg(not(windows))]
pub(crate) fn resolve_shell(_app: &AppHandle) -> ShellKind {
    ShellKind::Cmd
}

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

/// spawn_terminal 一次执行所需的 shell 解析结果(种类之外的附属信息)
#[cfg(windows)]
#[derive(Clone, Copy)]
struct ShellTools<'a> {
    /// git bash 的 bash.exe 全路径(GitBash 分支使用)
    bash: Option<&'a str>,
    /// PowerShell 可执行文件名:PATH 上有 pwsh 时优先(PowerShell 7 才支持 && / ||),
    /// 否则回退系统自带的 Windows PowerShell 5.1
    ps: &'a str,
}

/// 在系统终端打开目录,可选执行命令(跑完不关窗口),终端类型由设置项 `terminal` 决定。
/// 优先用 Windows Terminal(wt.exe);未安装或启动失败时退回 start 拉起对应 shell。
#[cfg(windows)]
pub fn spawn_terminal(
    path: &str,
    title: &str,
    command: Option<&str>,
    shell: ShellKind,
) -> AppResult<()> {
    // Git Bash 未安装时回退 cmd:被执行的命令往往是 npm 这类通用命令,降级执行优于报错阻断
    let (shell, bash) = match shell {
        ShellKind::GitBash => match find_git_bash() {
            Some(bash) => (ShellKind::GitBash, Some(bash)),
            None => {
                eprintln!("[open] 未找到 Git for Windows 的 bash.exe,回退到 cmd 执行");
                (ShellKind::Cmd, None)
            }
        },
        other => (other, None),
    };
    let tools = ShellTools {
        bash: bash.as_deref(),
        ps: if shell == ShellKind::PowerShell {
            find_powershell()
        } else {
            "powershell"
        },
    };
    let command = flatten_multiline(command, shell.separator());
    let command = command.as_deref();
    if let Some(wt) = find_wt() {
        // wt 是 GUI 子系统进程,自己创建窗口,不存在句柄透传问题,直接启动即可
        let spawned = Command::new(wt)
            .args(build_wt_args(path, title, command, shell, tools))
            .spawn();
        if spawned.is_ok() {
            return Ok(());
        }
        // 启动失败(如 AppX 执行别名被用户禁用)则继续走下面的 start 兜底
    }
    // 结构:cmd /C start "<title>" <shell 启动串>,外层 cmd 用 CREATE_NO_WINDOW 隐藏。
    //
    // 为什么不能直接 CREATE_NEW_CONSOLE 起目标 shell:Rust 的 Command 会把父进程
    // (Tauri 应用)的标准句柄透传给子进程——即使子进程拿到了新控制台,它的
    // stdout/stderr 仍指向父进程的句柄(dev 终端或管道),命令照常执行但新窗口里
    // 看不到任何输出。而 start 拉起目标进程时不透传句柄,目标 shell 会拿到全新控制台
    // 的输入/输出句柄,输出正常显示。(已用 marker 实验证实两种方式的句柄流向)
    //
    // 引号解析(已实测,cmd 分支):
    // - 外层 cmd /C 的命令串首字符是 's',不触发首尾引号剥离;
    // - `&&` 位于 cmd /K 的引号串内,不会在外层被顶层切分,整串交给 start;
    // - start 把第一个引号串当窗口标题,其余原样交给新进程;
    // - 内层 cmd /K 首字符是引号,剥掉首尾引号后得到 `cd /d "<path>" && <command>`,
    //   在同一窗口内依次执行,跑完窗口保留。
    // powershell / git bash 分支由 start 的 /D 参数指定工作目录,命令经
    // -EncodedCommand(base64) / 双引号包裹的 -c 负载传递。
    let cmdline = build_start_cmdline(path, title, command, shell, tools);
    hidden(Command::new("cmd")).raw_arg(&cmdline).spawn()?;
    Ok(())
}

/// 构造外层 cmd 的命令串(非 cmd 的 shell 也借 start 拉起,工作目录用 /D 指定):
/// cmd:        /C start "<title>" cmd /K "cd /d "<path>" && <command>"
/// powershell: /C start "<title>" /D "<path>" <ps> -NoExit -EncodedCommand <b64>
/// git bash:   /C start "<title>" /D "<path>" "<bash>" --login -i -c "<command>; exec bash"
#[cfg(windows)]
fn build_start_cmdline(
    path: &str,
    title: &str,
    command: Option<&str>,
    shell: ShellKind,
    tools: ShellTools<'_>,
) -> String {
    // title 是展示文本,剥掉 cmd 元字符,避免打乱引号配对;path 剥掉双引号
    let title = sanitize_cmd_text(title);
    let path = path.replace('"', "");
    match shell {
        ShellKind::Cmd => {
            let inner = match command {
                Some(c) => format!("cd /d \"{path}\" && {c}"),
                None => format!("cd /d \"{path}\""),
            };
            format!("/C start \"{title}\" cmd /K \"{inner}\"")
        }
        ShellKind::PowerShell => {
            let tail = match command {
                Some(c) => format!("-NoExit -EncodedCommand {}", encode_powershell_command(c)),
                None => "-NoExit".to_string(),
            };
            format!("/C start \"{title}\" /D \"{path}\" {} {tail}", tools.ps)
        }
        ShellKind::GitBash => {
            let bash = tools.bash.unwrap_or("bash");
            // -c 负载整体包双引号:外层 cmd 会保护引号内的 & | < > 等元字符;
            // 负载内嵌 `"` 会破坏配对,属已知限制(与 cmd 分支原样透传的约定同级)
            let tail = match command {
                Some(c) => format!("--login -i -c \"{c}; exec bash\""),
                None => "--login -i".to_string(),
            };
            format!("/C start \"{title}\" /D \"{path}\" \"{bash}\" {tail}")
        }
    }
}

/// 多行命令摊平成单行:cmd /k 与 AppleScript 的 do script 都无法携带换行
/// (换行即终结当前命令串,只有第一行会被执行)。
/// 先合并 bash 风格 `\` 续行(仅当 `\` 前是空白字符——避免把行尾恰好是
/// Windows 路径如 `C:\tools\` 的既有命令误并行),逐行 trim、丢弃空行后用
/// 顺序分隔符连接(cmd 用 ` & `、其余 shell 用 `; `),语义等同在终端逐行
/// 回车:前一条失败不阻断后续(故不用 `&&`)。
/// 单行命令原样借用返回,不产生分配。
#[cfg(any(windows, target_os = "macos"))]
fn flatten_multiline<'a>(command: Option<&'a str>, sep: &str) -> Option<std::borrow::Cow<'a, str>> {
    use std::borrow::Cow;
    let c = command?;
    if !c.contains(['\n', '\r']) {
        return Some(Cow::Borrowed(c));
    }
    let mut logical: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in c.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue; // 空行一律丢弃(续行中间的空行同样忽略)
        }
        if !current.is_empty() {
            current.push(' ');
        }
        if is_continuation(line) {
            current.push_str(line[..line.len() - 1].trim_end());
        } else {
            current.push_str(line);
            logical.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        logical.push(current);
    }
    Some(Cow::Owned(logical.join(sep)))
}

/// 判断一行是否以 `\` 续行符结尾:`\` 前必须是空白字符(或整行只有 `\`),
/// 以免把行尾是 Windows 路径(`C:\tools\`)的命令误当续行
#[cfg(any(windows, target_os = "macos"))]
fn is_continuation(line: &str) -> bool {
    line.strip_suffix('\\')
        .is_some_and(|head| head.is_empty() || head.ends_with(char::is_whitespace))
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

/// 构造 wt 参数:`wt --title "<title>" -d "<path>" [<shell> <args...>]`
/// 不带命令时 cmd 分支由 wt 打开默认配置文件的 shell,其余分支显式启动所选 shell;
/// 带命令时按 shell 包装(cmd /k、powershell -NoExit -EncodedCommand、bash -c '...; exec bash'),
/// 跑完窗口保留
#[cfg(windows)]
fn build_wt_args(
    path: &str,
    title: &str,
    command: Option<&str>,
    shell: ShellKind,
    tools: ShellTools<'_>,
) -> Vec<String> {
    let mut args = vec![
        "--title".into(),
        sanitize_cmd_text(title),
        "-d".into(),
        path.replace('"', ""),
    ];
    match shell {
        ShellKind::Cmd => {
            if let Some(c) = command {
                args.extend(["cmd".into(), "/k".into(), c.into()]);
            }
        }
        ShellKind::PowerShell => {
            args.push(tools.ps.into());
            if let Some(c) = command {
                args.extend([
                    "-NoExit".into(),
                    "-EncodedCommand".into(),
                    encode_powershell_command(c),
                ]);
            }
        }
        ShellKind::GitBash => {
            args.push(tools.bash.unwrap_or("bash").into());
            match command {
                Some(c) => args.extend([
                    "--login".into(),
                    "-i".into(),
                    "-c".into(),
                    format!("{c}; exec bash"),
                ]),
                None => args.extend(["--login".into(), "-i".into()]),
            }
        }
    }
    args
}

/// PowerShell 命令编码:脚本整体转 UTF-16LE 后 base64,配合 -EncodedCommand 使用,
/// 彻底规避命令文本在外层 cmd / wt 与 powershell 之间的引号转义问题。
/// 脚本开头附一句 Write-Host 回显原命令文本(对齐 cmd /K 的回显行为,便于排查)。
#[cfg(windows)]
fn encode_powershell_command(command: &str) -> String {
    use base64::Engine as _;
    let echoed = command.replace('\'', "''");
    let script = format!("Write-Host '{echoed}'; {command}");
    let utf16_le: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(utf16_le)
}

/// PowerShell 可执行文件名:优先 pwsh(PowerShell 7+,支持 && / || 短路运算符),
/// 不在 PATH 时回退系统自带的 Windows PowerShell 5.1(powershell.exe, && 会直接语法错误)
#[cfg(windows)]
fn find_powershell() -> &'static str {
    let probe = hidden(Command::new("where")).arg("pwsh").output();
    match probe {
        Ok(out) if out.status.success() => "pwsh",
        _ => "powershell",
    }
}

/// 定位 Git for Windows 的 bash.exe:先从 `where git` 的结果推导
/// (<Git>\cmd\git.exe -> <Git>\bin\bash.exe),再探测常见安装目录。
/// 不用裸 `where bash` —— 那会命中 WSL 的 C:\Windows\System32\bash.exe
#[cfg(windows)]
fn find_git_bash() -> Option<String> {
    if let Ok(out) = hidden(Command::new("where")).arg("git").output() {
        if out.status.success() {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let git = std::path::Path::new(line.trim());
                if let Some(root) = git.parent().and_then(|p| p.parent()) {
                    let bash = root.join("bin").join("bash.exe");
                    if bash.is_file() {
                        return Some(bash.to_string_lossy().to_string());
                    }
                }
            }
        }
    }
    let mut candidates = vec![
        r"C:\Program Files\Git\bin\bash.exe".to_string(),
        r"C:\Program Files (x86)\Git\bin\bash.exe".to_string(),
    ];
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        candidates.push(format!(r"{local}\Programs\Git\bin\bash.exe"));
    }
    candidates
        .into_iter()
        .find(|p| std::path::Path::new(p).is_file())
}

#[cfg(target_os = "macos")]
pub fn spawn_terminal(path: &str, _title: &str, command: Option<&str>, _shell: ShellKind) -> AppResult<()> {
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
pub fn spawn_terminal(
    _path: &str,
    _title: &str,
    _command: Option<&str>,
    _shell: ShellKind,
) -> AppResult<()> {
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

/// 渲染用户配置的打开命令。{path} 经环境变量(REPOMEOW_OPEN_PATH)传入 shell,
/// 文本中只保留对应的引用占位符,避免直接拼接用户路径;
/// {line} 走文本内插(参数类型 u32,无注入面)。
/// 未使用 {path} 时在命令末尾追加引用后的路径占位符,保留简洁配置方式。
fn render_custom_open_command(command: &str, line: Option<u32>) -> String {
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
fn custom_open_cmdline(rendered: &str) -> String {
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
fn command_on_path(cli: &str) -> bool {
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
pub fn open_in_editor(app: AppHandle, path: String, kind: EditorKind, line: Option<u32>) -> AppResult<()> {
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
        .arg(format!(
            "/select,{}",
            crate::path_util::to_native_separator(path)
        ))
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

    #[cfg(windows)]
    fn cmd_tools() -> ShellTools<'static> {
        ShellTools {
            bash: None,
            ps: "powershell",
        }
    }

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
    fn custom_open_command_substitutes_placeholders() {
        let command = render_custom_open_command("my-editor --goto {path}:{line}", Some(42));
        if cfg!(windows) {
            assert_eq!(command, "my-editor --goto \"%REPOMEOW_OPEN_PATH%\":42");
        } else {
            assert_eq!(command, "my-editor --goto \"$REPOMEOW_OPEN_PATH\":42");
        }
    }

    #[test]
    fn custom_open_command_appends_missing_path_and_blanks_missing_line() {
        let command = render_custom_open_command("my-editor --line={line}", None);
        if cfg!(windows) {
            assert_eq!(command, "my-editor --line= \"%REPOMEOW_OPEN_PATH%\"");
        } else {
            assert_eq!(command, "my-editor --line= \"$REPOMEOW_OPEN_PATH\"");
        }
    }

    #[cfg(windows)]
    #[test]
    fn bare_code_command_keeps_appended_path_in_raw_cmdline() {
        let rendered = render_custom_open_command("code", None);
        assert_eq!(rendered, "code \"%REPOMEOW_OPEN_PATH%\"");
        assert_eq!(
            custom_open_cmdline(&rendered),
            "/C code \"%REPOMEOW_OPEN_PATH%\""
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
        let s = build_start_cmdline(
            r"D:\code\foo",
            "Project: my-app",
            Some("npm run dev"),
            ShellKind::Cmd,
            cmd_tools(),
        );
        assert_eq!(
            s,
            r#"/C start "Project: my-app" cmd /K "cd /d "D:\code\foo" && npm run dev""#
        );
    }

    #[cfg(windows)]
    #[test]
    fn start_cmdline_without_command() {
        let s = build_start_cmdline(r"D:\code\foo bar", "Terminal", None, ShellKind::Cmd, cmd_tools());
        assert_eq!(s, r#"/C start "Terminal" cmd /K "cd /d "D:\code\foo bar"""#);
    }

    #[cfg(windows)]
    #[test]
    fn start_cmdline_sanitizes_display_text_but_keeps_command() {
        // path / title 中的引号与 cmd 元字符必须剥掉,避免打乱引号配对
        let s = build_start_cmdline(
            r#"D:\weird"path"#,
            r#"a&b"c"#,
            Some("echo hi"),
            ShellKind::Cmd,
            cmd_tools(),
        );
        assert_eq!(
            s,
            r#"/C start "abc" cmd /K "cd /d "D:\weirdpath" && echo hi""#
        );
        // 用户命令原样透传,允许 shell 操作符
        let s = build_start_cmdline(
            r"D:\p",
            "t",
            Some("cargo build && cargo run"),
            ShellKind::Cmd,
            cmd_tools(),
        );
        assert!(s.ends_with("&& cargo build && cargo run\""));
    }

    #[cfg(windows)]
    #[test]
    fn wt_args_without_command() {
        let args = build_wt_args(r"D:\code\foo bar", "Terminal", None, ShellKind::Cmd, cmd_tools());
        assert_eq!(args, ["--title", "Terminal", "-d", r"D:\code\foo bar"]);
    }

    #[cfg(windows)]
    #[test]
    fn wt_args_with_command_keeps_window() {
        // 带命令时必须套 cmd /k,保证跑完窗口保留
        let args = build_wt_args(
            r"D:\code\foo",
            "Project: my-app",
            Some("npm run dev"),
            ShellKind::Cmd,
            cmd_tools(),
        );
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
            ShellKind::Cmd,
            cmd_tools(),
        );
        assert_eq!(args[1], "abc");
        assert_eq!(args[3], r"D:\weirdpath");
        assert_eq!(args[6], "cargo build && cargo run");
    }

    #[cfg(windows)]
    #[test]
    fn wt_args_powershell_uses_encoded_command() {
        let args = build_wt_args(
            r"D:\code\foo",
            "t",
            Some("npm run dev"),
            ShellKind::PowerShell,
            cmd_tools(),
        );
        assert_eq!(args[4], "powershell");
        assert_eq!(args[5], "-NoExit");
        assert_eq!(args[6], "-EncodedCommand");
        assert_eq!(args[7], encode_powershell_command("npm run dev"));
        // 不带命令时仅开交互式 powershell
        let args = build_wt_args(r"D:\p", "t", None, ShellKind::PowerShell, cmd_tools());
        assert_eq!(args.len(), 5);
        assert_eq!(args[4], "powershell");
    }

    #[cfg(windows)]
    #[test]
    fn wt_args_gitbash_keeps_window_with_exec_bash() {
        let bash = r"C:\Program Files\Git\bin\bash.exe";
        let args = build_wt_args(
            r"D:\code\foo",
            "t",
            Some("npm run dev"),
            ShellKind::GitBash,
            ShellTools {
                bash: Some(bash),
                ps: "powershell",
            },
        );
        assert_eq!(
            args[4..],
            [bash, "--login", "-i", "-c", "npm run dev; exec bash"]
        );
        let args = build_wt_args(r"D:\p", "t", None,
            ShellKind::GitBash,
            ShellTools {
                bash: Some(bash),
                ps: "powershell",
            },
        );
        assert_eq!(args[4..], [bash, "--login", "-i"]);
    }

    #[cfg(windows)]
    #[test]
    fn start_cmdline_powershell_uses_start_d_and_encoded_command() {
        let s = build_start_cmdline(
            r"D:\code\foo",
            "t",
            Some("npm run dev"),
            ShellKind::PowerShell,
            cmd_tools(),
        );
        let expected = format!(
            r#"/C start "t" /D "D:\code\foo" powershell -NoExit -EncodedCommand {}"#,
            encode_powershell_command("npm run dev")
        );
        assert_eq!(s, expected);
        let s = build_start_cmdline(r"D:\p", "t", None, ShellKind::PowerShell, cmd_tools());
        assert_eq!(s, r#"/C start "t" /D "D:\p" powershell -NoExit"#);
    }

    #[cfg(windows)]
    #[test]
    fn start_cmdline_gitbash_wraps_command_in_quoted_c_payload() {
        let bash = r"C:\Program Files\Git\bin\bash.exe";
        let s = build_start_cmdline(
            r"D:\code\foo",
            "t",
            Some("npm run dev & echo done"),
            ShellKind::GitBash,
            ShellTools {
                bash: Some(bash),
                ps: "powershell",
            },
        );
        // -c 负载整体包双引号,外层 cmd 不会切分引号内的 & 元字符
        assert_eq!(
            s,
            r#"/C start "t" /D "D:\code\foo" "C:\Program Files\Git\bin\bash.exe" --login -i -c "npm run dev & echo done; exec bash""#
        );
    }

    #[cfg(windows)]
    #[test]
    fn powershell_encoded_command_roundtrips_utf16le() {
        use base64::Engine as _;
        let encoded = encode_powershell_command("echo 'hi' && echo done");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let script = String::from_utf16(&units).unwrap();
        // 开头回显原命令(单引号翻倍转义),分号后是原命令
        assert_eq!(
            script,
            "Write-Host 'echo ''hi'' && echo done'; echo 'hi' && echo done"
        );
    }

    #[test]
    fn shell_kind_from_setting_falls_back_to_cmd() {
        assert_eq!(
            ShellKind::from_setting(Some("powershell".into())),
            ShellKind::PowerShell
        );
        assert_eq!(
            ShellKind::from_setting(Some("gitbash".into())),
            ShellKind::GitBash
        );
        assert_eq!(ShellKind::from_setting(Some("cmd".into())), ShellKind::Cmd);
        assert_eq!(ShellKind::from_setting(None), ShellKind::Cmd);
        assert_eq!(
            ShellKind::from_setting(Some("zsh".into())),
            ShellKind::Cmd
        );
    }

    #[cfg(windows)]
    #[test]
    fn shell_kind_separator_per_shell() {
        assert_eq!(ShellKind::Cmd.separator(), " & ");
        assert_eq!(ShellKind::PowerShell.separator(), "; ");
        assert_eq!(ShellKind::GitBash.separator(), "; ");
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

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn flatten_multiline_merges_backslash_continuations() {
        // bash 风格续行(docs 常见写法)合并成一条逻辑行
        let c = flatten_multiline(
            Some("docker run -d \\\n  --name app \\\n  -p 8080:8080 nginx"),
            " & ",
        )
        .unwrap();
        assert_eq!(c, "docker run -d --name app -p 8080:8080 nginx");
        // 多组续行 + 独立命令混合,逻辑行间仍用顺序分隔符
        let c = flatten_multiline(Some("echo a \\\n  b\necho c"), " & ").unwrap();
        assert_eq!(c, "echo a b & echo c");
        // 续行中间的空行被忽略;行尾悬空续行不会残留 `\`
        let c = flatten_multiline(Some("echo a \\\n\n  b \\\n"), "; ").unwrap();
        assert_eq!(c, "echo a b");
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn flatten_multiline_keeps_trailing_backslash_paths() {
        // 保守规则:`\` 前不是空白时视为路径行尾,不当续行合并
        let c = flatten_multiline(Some("set P=C:\\tools\\\necho hi"), " & ").unwrap();
        assert_eq!(c, "set P=C:\\tools\\ & echo hi");
        // `\` 紧贴参数(无空白)同样不合并
        let c = flatten_multiline(Some("curl https://a.com/x\\\n  -H ok"), "; ").unwrap();
        assert_eq!(c, "curl https://a.com/x\\; -H ok");
    }

    #[cfg(windows)]
    #[test]
    fn start_cmdline_flattens_multiline_command() {
        let s = build_start_cmdline(
            r"D:\p",
            "t",
            flatten_multiline(Some("echo a\necho b"), " & ").as_deref(),
            ShellKind::Cmd,
            cmd_tools(),
        );
        assert!(s.ends_with("&& echo a & echo b\""));
    }

    #[cfg(windows)]
    #[test]
    fn find_wt_does_not_panic() {
        let _wt = find_wt();
    }
}
