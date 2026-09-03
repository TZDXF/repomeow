use crate::db;
use super::editor::*;
use super::shell::*;
use super::terminal::*;
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
    let s = build_start_cmdline(
        r"D:\code\foo bar",
        "Terminal",
        None,
        ShellKind::Cmd,
        cmd_tools(),
    );
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
    let args = build_wt_args(
        r"D:\code\foo bar",
        "Terminal",
        None,
        ShellKind::Cmd,
        cmd_tools(),
    );
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
fn wt_args_gitbash_encodes_command_to_hide_semicolons_from_wt() {
    use base64::Engine as _;
    let bash = r"C:\Program Files\Git\bin\bash.exe";
    let tools = ShellTools {
        bash: Some(bash),
        ps: "powershell",
    };
    let args = build_wt_args(
        r"D:\code\foo",
        "t",
        Some("npm run dev"),
        ShellKind::GitBash,
        tools,
    );
    assert_eq!(args[4..8], [bash, "--login", "-i", "-c"]);
    // wt 可见的命令行中不允许出现 `;` 与内层双引号(wt 会把 `;` 当子命令
    // 分隔符拆成多个标签页,且其分词器不识别 `\"` 转义)
    let c_arg = &args[8];
    assert!(!c_arg.contains(';'));
    assert!(!c_arg.contains('"'));
    // 负载是 base64 编码的「用户命令 + 保活 exec bash」,由 bash 解码执行
    let b64 = c_arg
        .strip_prefix("source <(echo ")
        .and_then(|s| s.strip_suffix(" | base64 -d)"))
        .unwrap();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .unwrap();
    assert_eq!(
        String::from_utf8(decoded).unwrap(),
        "npm run dev; exec bash"
    );
    // 不带命令时仅开交互式 bash,无需编码
    let args = build_wt_args(r"D:\p", "t", None, ShellKind::GitBash, tools);
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
fn terminal_capabilities_serialize_with_frontend_contract() {
    let capabilities = build_terminal_capabilities(true, false, true, false, true);
    assert_eq!(
        serde_json::to_value(capabilities).unwrap(),
        serde_json::json!({
            "isWindows": true,
            "windowsTerminal": false,
            "shells": {
                "cmd": true,
                "powershell": false,
                "gitbash": true,
            },
        })
    );
}

#[test]
fn non_windows_terminal_capabilities_are_all_false() {
    // 即使探测输入为 true，平台不支持时也不向前端暴露任何 Windows 能力。
    let capabilities = build_terminal_capabilities(false, true, true, true, true);
    assert_eq!(
        serde_json::to_value(capabilities).unwrap(),
        serde_json::json!({
            "isWindows": false,
            "windowsTerminal": false,
            "shells": {
                "cmd": false,
                "powershell": false,
                "gitbash": false,
            },
        })
    );
}

#[test]
fn unavailable_requested_shell_falls_back_to_cmd() {
    assert_eq!(
        fallback_to_cmd_if_unavailable(ShellKind::PowerShell, false),
        ShellKind::Cmd
    );
    assert_eq!(
        fallback_to_cmd_if_unavailable(ShellKind::GitBash, false),
        ShellKind::Cmd
    );
    assert_eq!(
        fallback_to_cmd_if_unavailable(ShellKind::PowerShell, true),
        ShellKind::PowerShell
    );
    assert_eq!(
        fallback_to_cmd_if_unavailable(ShellKind::GitBash, true),
        ShellKind::GitBash
    );
    // Cmd 没有其他可回退的解释器，即使探测异常也保持 Cmd。
    assert_eq!(
        fallback_to_cmd_if_unavailable(ShellKind::Cmd, false),
        ShellKind::Cmd
    );
}

#[test]
fn generic_winget_wt_link_is_not_treated_as_windows_terminal() {
    assert!(is_winget_link(
        r"C:\Users\me\AppData\Local\Microsoft\WinGet\Links\wt.exe"
    ));
    assert!(!is_winget_link(
        r"C:\Users\me\AppData\Local\Microsoft\WindowsApps\wt.exe"
    ));
    assert!(!is_winget_link(r"C:\tools\scoop\shims\wt.exe"));
}

#[cfg(windows)]
#[test]
fn powershell_selection_prefers_pwsh_and_requires_an_available_binary() {
    assert_eq!(select_powershell(true, true), Some("pwsh"));
    assert_eq!(select_powershell(true, false), Some("pwsh"));
    assert_eq!(select_powershell(false, true), Some("powershell"));
    assert_eq!(select_powershell(false, false), None);
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
    assert_eq!(ShellKind::from_setting(Some("zsh".into())), ShellKind::Cmd);
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
