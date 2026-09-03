use std::process::Command;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use crate::error::AppResult;
use super::*;

/// spawn_terminal 一次执行所需的 shell 解析结果(种类之外的附属信息)
#[cfg(windows)]
#[derive(Clone, Copy)]
pub(super) struct ShellTools<'a> {
    /// git bash 的 bash.exe 全路径(GitBash 分支使用)
    pub(super) bash: Option<&'a str>,
    /// 已探测到的 PowerShell 可执行文件名:PATH 上有 pwsh 时优先
    /// (PowerShell 7 才支持 && / ||),否则使用 Windows PowerShell 5.1
    pub(super) ps: &'a str,
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
    // 即使调用方已提前探测，启动前仍防御性复查，覆盖运行期间卸载或直接传入
    // ShellKind 的调用点。被执行的命令往往是 npm 这类通用命令，降级优于阻断。
    let (shell, bash, ps) = match shell {
        ShellKind::GitBash => match find_git_bash() {
            Some(bash) => (ShellKind::GitBash, Some(bash), "powershell"),
            None => {
                eprintln!("[open] 未找到 Git for Windows 的 bash.exe,回退到 cmd 执行");
                (ShellKind::Cmd, None, "powershell")
            }
        },
        ShellKind::PowerShell => match find_powershell() {
            Some(ps) => (ShellKind::PowerShell, None, ps),
            None => {
                eprintln!("[open] 未找到 pwsh.exe 或 powershell.exe,回退到 cmd 执行");
                (ShellKind::Cmd, None, "powershell")
            }
        },
        ShellKind::Cmd => (ShellKind::Cmd, None, "powershell"),
    };
    let tools = ShellTools {
        bash: bash.as_deref(),
        ps,
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
pub(super) fn build_start_cmdline(
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
pub(super) fn flatten_multiline<'a>(command: Option<&'a str>, sep: &str) -> Option<std::borrow::Cow<'a, str>> {
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
pub(super) fn is_continuation(line: &str) -> bool {
    line.strip_suffix('\\')
        .is_some_and(|head| head.is_empty() || head.ends_with(char::is_whitespace))
}

/// 剥掉会破坏 cmd 命令行解析的元字符(仅用于 title 这类展示文本;
/// 用户命令原样透传,允许包含 && | 等 shell 操作符)
#[cfg(windows)]
pub(super) fn sanitize_cmd_text(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '"' | '&' | '|' | '<' | '>' | '^'))
        .collect()
}

/// 定位 wt.exe:先查 AppX 执行别名(Store / 官网安装都会注册),
/// 再用 where 搜 PATH(覆盖 scoop / choco / 绿色版等安装方式)。WinGet Links 中的
/// `wt.exe` 可能是 Worktrunk 等同名工具,不能据此认定为 Windows Terminal。
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
            .map(str::trim)
            .find(|path| !path.is_empty() && !is_winget_link(path))
            .map(str::to_string),
        _ => None,
    }
}

/// WinGet 的通用链接目录只表示“有一个包声明了该命令名”,无法区分 Windows Terminal
/// 与 Worktrunk 等同名 `wt.exe`;官方 MSIX 版会走上面的 WindowsApps 执行别名。
#[cfg(any(windows, test))]
pub(super) fn is_winget_link(path: &str) -> bool {
    path.replace('/', "\\")
        .to_ascii_lowercase()
        .contains(r"\microsoft\winget\links\")
}

/// 构造 wt 参数:`wt --title "<title>" -d "<path>" [<shell> <args...>]`
/// 不带命令时 cmd 分支由 wt 打开默认配置文件的 shell,其余分支显式启动所选 shell;
/// 带命令时按 shell 包装(cmd /k、powershell -NoExit -EncodedCommand、
/// bash -c 经 encode_bash_command 编码),跑完窗口保留
#[cfg(windows)]
pub(super) fn build_wt_args(
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
                    encode_bash_command(c),
                ]),
                None => args.extend(["--login".into(), "-i".into()]),
            }
        }
    }
    args
}

/// Git Bash 命令在 wt 命令行下的安全包装:wt 把命令行中的 `;` 一律视为子命令
/// 分隔符(引号内也不豁免),且其分词器不识别 `\"` 转义——`-c` 负载中内嵌的
/// 双引号会把参数切碎。与 PowerShell 的 -EncodedCommand 同理,把完整负载
/// (用户命令 + 保活的 exec bash)base64 编码,由 bash 启动后经进程替代解码执行:
/// `source <(echo <b64> | base64 -d)`。base64 字母表不含 wt 的任何特殊字符,
/// 负载内不出现 `;` 与双引号;source 直接按脚本语法解析解码结果,不经分词/glob,
/// 命令里的字符串字面量原样保留。
#[cfg(windows)]
pub(super) fn encode_bash_command(command: &str) -> String {
    use base64::Engine as _;
    let payload = format!("{command}; exec bash");
    let b64 = base64::engine::general_purpose::STANDARD.encode(payload);
    format!("source <(echo {b64} | base64 -d)")
}

/// PowerShell 命令编码:脚本整体转 UTF-16LE 后 base64,配合 -EncodedCommand 使用,
/// 彻底规避命令文本在外层 cmd / wt 与 powershell 之间的引号转义问题。
/// 脚本开头附一句 Write-Host 回显原命令文本(对齐 cmd /K 的回显行为,便于排查)。
#[cfg(windows)]
pub(super) fn encode_powershell_command(command: &str) -> String {
    use base64::Engine as _;
    let echoed = command.replace('\'', "''");
    let script = format!("Write-Host '{echoed}'; {command}");
    let utf16_le: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(utf16_le)
}

/// 按探测结果选择 PowerShell 可执行文件名。保持为纯函数供单元测试覆盖优先级和
/// “两者都不可用”的分支。
#[cfg(windows)]
pub(super) fn select_powershell(
    pwsh_available: bool,
    windows_powershell_available: bool,
) -> Option<&'static str> {
    if pwsh_available {
        Some("pwsh")
    } else if windows_powershell_available {
        Some("powershell")
    } else {
        None
    }
}

/// PowerShell 可执行文件名:优先 pwsh(PowerShell 7+,支持 && / || 短路运算符),
/// 再确认系统 Windows PowerShell 5.1 是否确实可执行；两者都不可用时返回 None。
#[cfg(windows)]
pub(super) fn find_powershell() -> Option<&'static str> {
    let pwsh_available = command_on_path("pwsh");
    let windows_powershell_available = !pwsh_available && command_on_path("powershell");
    select_powershell(pwsh_available, windows_powershell_available)
}

/// cmd 是 Windows 启动兜底，同时仍按实际 ComSpec / PATH 给设置页返回可用性标记。
#[cfg(windows)]
pub(super) fn cmd_available() -> bool {
    std::env::var_os("ComSpec").is_some_and(|path| std::path::Path::new(&path).is_file())
        || command_on_path("cmd")
}

/// 定位 Git for Windows 的 bash.exe:先从 `where git` 的结果推导
/// (<Git>\cmd\git.exe -> <Git>\bin\bash.exe),再探测常见安装目录。
/// 不用裸 `where bash` —— 那会命中 WSL 的 C:\Windows\System32\bash.exe
#[cfg(windows)]
pub(super) fn find_git_bash() -> Option<String> {
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
pub fn spawn_terminal(
    path: &str,
    _title: &str,
    command: Option<&str>,
    _shell: ShellKind,
) -> AppResult<()> {
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

