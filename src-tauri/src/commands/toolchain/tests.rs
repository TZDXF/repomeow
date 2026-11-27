use std::collections::HashSet;

use crate::error::ErrorCode;
use crate::models::{
    ToolchainCaps, ToolchainKind, ToolchainRemoteVersion, ToolchainStatus, ToolchainUpdateState,
    ToolchainVersion,
};

#[cfg(not(windows))]
use super::detect::fill_unix_nvm;
use super::detect::{hidden_on_platform, retain_for_platform, Platform, TOOLS};
use super::operation::{caps_for, resolve_op, sanitize_version};
// `parse_shell_path_output` 的可用条件是 `any(target_os = "macos", test)`:本文件只在
// `#[cfg(test)] mod tests` 里编译,故 lib-test 目标下各平台都有它,导入不挂平台门控
// (挂 `cfg(unix)` 会让 Windows 的 clippy 报 E0425)。
use super::process::parse_shell_path_output;
// `classify_source` 各平台都有,但用它的用例只在 Unix 语义下成立,导入随用例门控,
// 否则 Windows 上这条 import 无人使用(clippy -D warnings 会拦 unused import)。
#[cfg(unix)]
use super::process::classify_source;
use super::update_check::{
    parse_rustup_check, parse_uv_self_update_dry_run, parse_winget_list_row,
};
use super::version::{
    extract_semver, natural_version_cmp, parse_dotnet_sdks, parse_gh_auth_status,
    parse_nvm_available_table, parse_nvm_list, parse_remote_tokens, parse_token_versions,
    parse_uv_python_list, parse_uv_python_remote, parse_vp_env_list, parse_vp_remote,
    python_version_from_path,
};

fn ver(name: &str, current: bool) -> ToolchainVersion {
    ToolchainVersion {
        name: name.to_string(),
        current,
    }
}

fn remote(name: &str, tag: Option<&str>) -> ToolchainRemoteVersion {
    ToolchainRemoteVersion {
        name: name.to_string(),
        tag: tag.map(str::to_string),
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
        extract_semver("uv 0.5.11 (7e988cdcd 2024-12-16)").as_deref(),
        Some("0.5.11")
    );
    assert_eq!(
        extract_semver("rustup 1.27.1 (dd91c1e4b 2024-04-17)").as_deref(),
        Some("1.27.1")
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
    let nvm = parse_nvm_list(
        "    18.20.4\n  * 20.11.1 (Currently using 64-bit executable)\nNoVersionsInstalledYet?\n",
    );
    assert_eq!(nvm, vec![ver("18.20.4", false), ver("20.11.1", true)]);
}

#[test]
fn parses_fnm_list() {
    let fnm = parse_token_versions("* v18.20.4 default\n  v20.11.1\n  system\n");
    assert_eq!(fnm, vec![ver("18.20.4", true), ver("20.11.1", false)]);
}

#[test]
fn parses_vp_env_list_tolerantly() {
    let vp = parse_vp_env_list(
        "* v24.15.0\n\u{1b}[94m* v24.19.0 \u{1b}[2mcurrent\u{1b}[0m\u{1b}[39m\nnote: clean\n",
    );
    assert_eq!(vp, vec![ver("24.15.0", false), ver("24.19.0", true)]);
    assert!(parse_vp_env_list("no versions here").is_empty());
}

#[test]
fn parses_dotnet_sdk_list() {
    let dotnet = parse_dotnet_sdks(
        "8.0.204 [C:\\Program Files\\dotnet\\sdk]\n10.0.100 [C:\\Program Files\\dotnet\\sdk]\n",
    );
    assert_eq!(dotnet, vec![ver("8.0.204", false), ver("10.0.100", false)]);
}

#[cfg(not(windows))]
#[test]
fn unix_nvm_dir_versions_and_default_alias() {
    use crate::models::{ToolchainKind, ToolchainStatus};

    let dir = std::env::temp_dir().join(format!("repomeow-nvm-{}", std::process::id()));
    let versions = dir.join(".nvm").join("versions").join("node");
    std::fs::create_dir_all(versions.join("v18.20.4")).unwrap();
    std::fs::create_dir_all(versions.join("v20.11.1")).unwrap();
    std::fs::create_dir_all(dir.join(".nvm").join("alias")).unwrap();
    std::fs::write(dir.join(".nvm").join("alias").join("default"), "18").unwrap();
    let mut status = ToolchainStatus {
        id: "nvm".to_string(),
        kind: ToolchainKind::Node,
        found: false,
        version: None,
        path: None,
        source: None,
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
    std::env::set_var("USERPROFILE", &dir);
    fill_unix_nvm(&mut status);
    std::env::remove_var("USERPROFILE");
    assert!(status.found);
    assert_eq!(status.versions[0], ver("18.20.4", true));
    assert_eq!(status.versions[1], ver("20.11.1", false));
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
    let error = resolve_op("nvm", "use", Some("22 && calc"), None).unwrap_err();
    assert!(error.is_code(ErrorCode::ToolchainVersionInvalid));
}

#[test]
fn unknown_tool_or_op_is_unsupported() {
    let error = resolve_op("npm", "update", None, None).unwrap_err();
    assert!(error.is_code(ErrorCode::ToolchainOpUnsupported));
    let error = resolve_op("dotnet", "use", Some("8"), None).unwrap_err();
    assert!(error.is_code(ErrorCode::ToolchainOpUnsupported));
    let error = resolve_op("nvm", "use", None, None).unwrap_err();
    assert!(error.is_code(ErrorCode::ToolchainOpUnsupported));
}

#[cfg(windows)]
#[test]
fn resolves_windows_matrix() {
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
    assert_eq!(
        resolve_op("cargo", "update", None, None).unwrap(),
        "rustup update"
    );
    assert!(resolve_op("rustc", "uninstall", None, None).is_err());
    assert_eq!(
        resolve_op("uv", "install", None, None).unwrap(),
        r#"powershell -ExecutionPolicy ByPass -c "irm https://astral.sh/uv/install.ps1 | iex""#
    );
    assert_eq!(
        resolve_op("uv", "update", None, Some("winget")).unwrap(),
        "winget upgrade --id astral-sh.uv -e"
    );
    assert_eq!(
        resolve_op("uv", "uninstall", None, Some("rustup")).unwrap(),
        "cargo uninstall uv"
    );
    assert_eq!(
        resolve_op("nvm", "use", Some("22.11.0"), None).unwrap(),
        "nvm use 22.11.0"
    );
    assert_eq!(
        resolve_op("fnm", "use", Some("20"), None).unwrap(),
        "fnm default 20"
    );
    assert_eq!(
        resolve_op("vp", "use", Some("lts/hydro"), None).unwrap(),
        "vp env default lts/hydro"
    );
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
            assert!(!visible || resolvable, "{id} {op} 与能力声明不一致");
        }
    };
    check(
        "uv",
        caps_for("uv", true, Some("standalone"), false),
        Some("standalone"),
    );
    check("uv", caps_for("uv", false, None, false), None);
    check("rustup", caps_for("rustup", true, None, true), None);
    check(
        "nvm",
        caps_for("nvm", true, Some("standalone"), false),
        Some("standalone"),
    );
    check(
        "git",
        caps_for("git", true, Some("standalone"), false),
        Some("standalone"),
    );
}

#[test]
fn resolves_uv_python_version_ops() {
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
fn parses_uv_python_list_output() {
    let local = parse_uv_python_list(
        "cpython-3.12.7-windows-x86_64-none path\n\
         cpython-3.13.1t-windows-x86_64-none path\n\
         cpython-3.13.1-windows-x86_64-none path\n\
         pypy-3.9.19-windows-x86_64-none path\n",
    );
    assert_eq!(local, vec!["3.12.7", "3.13.1", "3.9.19"]);
    assert!(parse_uv_python_list("not-a-python-line").is_empty());
}

#[test]
fn parses_uv_python_remote_output() {
    let remote_versions = parse_uv_python_remote(
        "cpython-3.15.0a8-windows-x86_64-none <download available>\n\
         cpython-3.14.4+freethreaded-windows-x86_64-none <download available>\n\
         cpython-3.14.4-windows-x86_64-none <download available>\n\
         pypy-3.11.13-windows-x86_64-none <download available>\n",
    );
    assert_eq!(
        remote_versions
            .iter()
            .map(|version| version.name.as_str())
            .collect::<Vec<_>>(),
        vec!["3.15.0a8", "3.14.4", "3.11.13"]
    );
    assert!(parse_uv_python_remote("v22.11.0").is_empty());
}

#[test]
fn extracts_python_version_from_find_output() {
    assert_eq!(
        python_version_from_path(
            r"C:\Users\x\AppData\Roaming\uv\python\cpython-3.12.7-windows-x86_64-none\python.exe"
        )
        .as_deref(),
        Some("3.12.7")
    );
    // `uv python install --default` 会生成次版本号目录(cpython-3.13-…),
    // `uv python find` 可能解析到这里,只有两段版本号
    assert_eq!(
        python_version_from_path(
            r"C:\Users\x\AppData\Roaming\uv\python\cpython-3.13-windows-x86_64-none\python.exe"
        )
        .as_deref(),
        Some("3.13")
    );
    assert_eq!(
        python_version_from_path(r"C:\Users\x\AppData\Local\Programs\Python\Python312\python.exe")
            .as_deref(),
        Some("3.12")
    );
    assert_eq!(
        python_version_from_path("/usr/bin/python3.11").as_deref(),
        Some("3.11")
    );
}

#[test]
fn parses_nvm_available_table_output() {
    let nvm = parse_nvm_available_table(
        "| CURRENT | LTS | OLD STABLE | OLD UNSTABLE |\n\
         |---------|-----|------------|--------------|\n\
         | 26.7.0 | 24.19.0 | 0.12.18 | 0.11.16 |\n",
    );
    assert_eq!(nvm[0], remote("26.7.0", Some("CURRENT")));
    assert_eq!(nvm[1], remote("24.19.0", Some("LTS")));
    let changed_header = parse_nvm_available_table(
        "| CURRENT | LTS | OLD STABLE | OLD LTS |\n\
         | 22.11.0 | 20.18.1 | 18.20.5 | 16.20.2 |\n",
    );
    assert_eq!(changed_header[3], remote("16.20.2", Some("OLD LTS")));
    assert!(parse_nvm_available_table("no table here").is_empty());
}

#[test]
fn parses_vp_remote_with_lts_codenames() {
    let vp = parse_vp_remote("v26.7.0\nv24.19.0 (Krypton)\nnote: clean\n");
    assert_eq!(vp[0], remote("26.7.0", Some("Current")));
    assert_eq!(vp[1], remote("24.19.0", Some("LTS")));
}

#[test]
fn parses_remote_token_lists() {
    assert_eq!(
        parse_remote_tokens("v22.11.0\nv20.18.1\nlts\n"),
        vec!["22.11.0", "20.18.1"]
    );
}

#[test]
fn natural_version_ordering() {
    let mut versions = vec!["20.9.0", "22.11.0", "22.10.0", "3.12.7"];
    versions.sort_by(|a, b| natural_version_cmp(b, a));
    assert_eq!(versions, vec!["22.11.0", "22.10.0", "20.9.0", "3.12.7"]);
}

#[test]
fn parses_gh_auth_status_output() {
    assert_eq!(
        parse_gh_auth_status(
            "github.com\n  ✓ Logged in to github.com account octocat (keyring)\n  - Active account: true\n"
        )
        .as_deref(),
        Some("octocat")
    );
    assert_eq!(
        parse_gh_auth_status("You are not logged into any GitHub hosts."),
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

#[test]
fn powershell_detection_and_operations() {
    assert!(TOOLS.iter().any(|spec| spec.id == "pwsh"));
    assert_eq!(extract_semver("PowerShell 7.6.0").as_deref(), Some("7.6.0"));
    let missing = caps_for("pwsh", false, None, false);
    assert_eq!(missing.can_install, cfg!(windows));
    assert!(!missing.can_update && !missing.can_uninstall);
    let installed = caps_for("pwsh", true, Some("standalone"), false);
    assert!(!installed.can_install && !installed.can_switch && !installed.can_list_remote);
    assert_eq!(installed.can_update, cfg!(windows));
    assert_eq!(installed.can_uninstall, cfg!(windows));
    for (op, action) in [
        ("install", "install"),
        ("update", "upgrade"),
        ("uninstall", "uninstall"),
    ] {
        let result = resolve_op("pwsh", op, None, None);
        if cfg!(windows) {
            assert_eq!(
                result.unwrap(),
                format!("winget {action} --id Microsoft.PowerShell -e")
            );
        } else {
            assert!(result.is_err());
        }
    }
    assert!(resolve_op("pwsh", "use", Some("7.6.0"), None).is_err());
}

#[test]
fn go_detection_and_operations() {
    assert!(TOOLS.iter().any(|spec| spec.id == "go"));
    // `go version` 输出:go version go1.23.2 windows/amd64
    assert_eq!(
        extract_semver("go version go1.23.2 windows/amd64").as_deref(),
        Some("1.23.2")
    );
    let missing = caps_for("go", false, None, false);
    assert!(missing.can_install);
    assert!(!missing.can_update && !missing.can_uninstall);
    let installed = caps_for("go", true, Some("winget"), false);
    assert!(!installed.can_install && !installed.can_switch && !installed.can_list_remote);
    assert!(installed.can_update && installed.can_uninstall);
    for (op, action) in [
        ("install", "install"),
        ("update", "upgrade"),
        ("uninstall", "uninstall"),
    ] {
        let result = resolve_op("go", op, None, Some("winget"));
        if cfg!(windows) {
            assert_eq!(
                result.unwrap(),
                format!("winget {action} --id GoLang.Go -e")
            );
        } else if cfg!(target_os = "macos") || op == "install" {
            assert!(result.is_ok());
        } else {
            assert!(result.is_err());
        }
    }
    assert!(resolve_op("go", "use", Some("1.23.2"), None).is_err());
}
#[test]
fn parses_winget_list_rows() {
    // 含「可用」列(中文表头):可更新,目标版本取 ID 后第二个版本号
    let (state, latest) = parse_winget_list_row(
        "名称                            ID              版本        可用        源\n\
         -----------------------------------------------------------------------\n\
         Git version 2.51.0.2            Git.Git         2.51.0.2    2.52.0      winget\n",
        "Git.Git",
    );
    assert_eq!(state, ToolchainUpdateState::UpdateAvailable);
    assert_eq!(latest.as_deref(), Some("2.52.0"));

    // 无「可用」列(英文表头):已是最新
    let (state, latest) = parse_winget_list_row(
        "Name                ID                      Version Source\n\
         ------------------------------------------------------------\n\
         PowerShell 7.6.0.4  Microsoft.PowerShell    7.6.0.4 winget\n",
        "Microsoft.PowerShell",
    );
    assert_eq!(state, ToolchainUpdateState::UpToDate);
    assert_eq!(latest, None);

    // 无匹配数据行(未安装/查询异常):未知
    let (state, latest) = parse_winget_list_row("No installed package found", "Git.Git");
    assert_eq!(state, ToolchainUpdateState::Unknown);
    assert_eq!(latest, None);
}

#[test]
fn parses_rustup_check_output() {
    // rustup 1.29.x 真实输出:小写 + 冒号间距不一致
    let output = "stable-x86_64-pc-windows-msvc - up to date: 1.98.1 (48a229cea 2026-09-01)\n\
                  rustup - up to date : 1.29.1\n";
    assert_eq!(
        parse_rustup_check(output, "rustup"),
        (ToolchainUpdateState::UpToDate, None)
    );
    assert_eq!(
        parse_rustup_check(output, "rustc"),
        (ToolchainUpdateState::UpToDate, None)
    );

    // 旧版首字母大写格式兼容
    let output = "stable-x86_64-pc-windows-msvc - Up to date : 1.89.0\n\
                  rustup - Up to date : 1.28.2\n";
    assert_eq!(
        parse_rustup_check(output, "rustup"),
        (ToolchainUpdateState::UpToDate, None)
    );

    // 工具链可更新:rustc/cargo 提示更新,rustup 自身不受影响
    let output = "stable-x86_64-pc-windows-msvc - update available: 1.89.0 (2025-08-07) -> 1.90.0 (2025-09-18)\n\
                  rustup - up to date : 1.28.2\n";
    assert_eq!(
        parse_rustup_check(output, "cargo"),
        (
            ToolchainUpdateState::UpdateAvailable,
            Some("1.90.0".to_string())
        )
    );
    assert_eq!(
        parse_rustup_check(output, "rustup"),
        (ToolchainUpdateState::UpToDate, None)
    );

    // rustup 自身可更新
    let output = "rustup - update available: 1.28.2 -> 1.29.0\n";
    assert_eq!(
        parse_rustup_check(output, "rustup"),
        (
            ToolchainUpdateState::UpdateAvailable,
            Some("1.29.0".to_string())
        )
    );
    assert_eq!(
        parse_rustup_check(output, "rustc").0,
        ToolchainUpdateState::Unknown
    );
}
#[test]
fn parses_uv_self_update_dry_run() {
    // 已最新(uv 0.12.x 真实输出)
    assert_eq!(
        parse_uv_self_update_dry_run(
            "info: Checking for updates...\n\
             success: You're already on version v0.12.15 of uv (the latest version).\n"
        ),
        (ToolchainUpdateState::UpToDate, None)
    );

    // 可更新:目标版本取自 " to " 之后
    assert_eq!(
        parse_uv_self_update_dry_run(
            "info: Checking for updates...\n\
             success: Would update uv from v0.12.15 to v0.13.1\n"
        ),
        (
            ToolchainUpdateState::UpdateAvailable,
            Some("0.13.1".to_string())
        )
    );

    // 其余输出(报错/无法判定):未知
    assert_eq!(
        parse_uv_self_update_dry_run("error: network unreachable"),
        (ToolchainUpdateState::Unknown, None)
    );
}

// ---- TZD-157:macOS 登录 shell PATH 与平台过滤 ────────────────────────────

#[test]
fn parses_login_shell_path_output() {
    // 真实 zsh -ilc 输出:前面可能有 motd / 插件输出,只在标记之间取值
    assert_eq!(
        parse_shell_path_output(
            "Last login: Thu Nov 27\n\
             REPOMEOW_PATH_BEGIN:/opt/homebrew/bin:/usr/bin:/bin:REPOMEOW_PATH_END"
        )
        .as_deref(),
        Some("/opt/homebrew/bin:/usr/bin:/bin")
    );
    // 作业控制告警与 PATH 值无关,不影响解析
    assert_eq!(
        parse_shell_path_output(
            "zsh: no job control in this shell\n\
             REPOMEOW_PATH_BEGIN:/usr/local/bin:/usr/bin:REPOMEOW_PATH_END\n"
        )
        .as_deref(),
        Some("/usr/local/bin:/usr/bin")
    );
    // 标记缺失(shell 报错/被 rc 文件覆盖):回退到兜底逻辑
    assert_eq!(parse_shell_path_output("/usr/bin:/bin"), None);
    assert_eq!(parse_shell_path_output(""), None);
    // 空 PATH 不当地址有效值,直接判定失败
    assert_eq!(
        parse_shell_path_output("REPOMEOW_PATH_BEGIN:  :REPOMEOW_PATH_END"),
        None
    );
}

#[cfg(unix)]
#[test]
fn merges_path_values_without_duplicate_or_empty_entries() {
    let merged = super::process::merge_path_values(&[
        "/opt/homebrew/bin:/usr/bin::/usr/bin".to_string(),
        "/usr/bin:/bin".to_string(),
    ]);
    assert_eq!(
        merged,
        vec![
            std::path::PathBuf::from("/opt/homebrew/bin"),
            std::path::PathBuf::from("/usr/bin"),
            std::path::PathBuf::from("/bin"),
        ]
    );
    assert!(super::process::merge_path_values(&[]).is_empty());
}

#[cfg(target_os = "macos")]
#[test]
fn fallback_probe_dirs_cover_homebrew_and_version_managers() {
    // 临时 HOME:只有真实存在的目录才会进兜底列表,故先造出各管理器的 bin 目录
    let home = std::env::temp_dir().join(format!("repomeow-fallback-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    for sub in [
        ".cargo/bin",
        ".local/bin",
        ".volta/bin",
        ".fnm/aliases/default/bin",
        ".nvm/versions/node/v20.11.1/bin",
        ".nvm/versions/node/v22.11.0/bin",
    ] {
        std::fs::create_dir_all(home.join(sub)).unwrap();
    }
    std::fs::create_dir_all(home.join(".nvm").join("alias")).unwrap();
    std::fs::write(home.join(".nvm").join("alias").join("default"), "20\n").unwrap();

    let dirs = super::process::fallback_probe_dirs(Some(&home));
    for expected in [
        std::path::PathBuf::from("/opt/homebrew/bin"),
        std::path::PathBuf::from("/usr/local/bin"),
        home.join(".cargo").join("bin"),
        home.join(".local").join("bin"),
        home.join(".volta").join("bin"),
        home.join(".fnm")
            .join("aliases")
            .join("default")
            .join("bin"),
    ] {
        assert!(
            dirs.contains(&expected),
            "缺少兜底目录 {}",
            expected.display()
        );
    }
    // nvm 按 alias/default 指定的版本优先,其余版本按版本号从高到低;不再依赖恒空的 ~/.nvm/current
    let nvm_dirs: Vec<_> = dirs
        .iter()
        .filter(|dir| dir.to_string_lossy().contains(".nvm/versions/node"))
        .collect();
    assert_eq!(
        nvm_dirs,
        vec![
            &home.join(".nvm/versions/node/v20.11.1/bin"),
            &home.join(".nvm/versions/node/v22.11.0/bin"),
        ],
        "nvm 兜底目录顺序或内容不符"
    );
    for dir in &dirs {
        assert!(dir.is_dir(), "兜底目录不存在: {}", dir.display());
    }
    assert!(!dirs.contains(&home.join(".nvm").join("current").join("bin")));

    // 没有 HOME 时只保留存在的系统级目录,不 panic
    for dir in super::process::fallback_probe_dirs(None) {
        assert!(dir.is_dir(), "兜底目录不存在: {}", dir.display());
    }
    let _ = std::fs::remove_dir_all(&home);
}

#[cfg(unix)]
#[test]
fn path_hits_in_requires_executable_file() {
    use std::os::unix::fs::PermissionsExt;

    let dir = std::env::temp_dir().join(format!("repomeow-path-hits-{}", std::process::id()));
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let executable = bin.join("mytool");
    std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    // 无执行位的文件与目录同名命中都不算
    let plain = bin.join("notool");
    std::fs::write(&plain, "").unwrap();
    std::fs::create_dir_all(bin.join("dirtool")).unwrap();

    let hits = super::process::path_hits_in("mytool", std::slice::from_ref(&bin));
    assert_eq!(hits, vec![executable]);
    assert!(super::process::path_hits_in("notool", std::slice::from_ref(&bin)).is_empty());
    assert!(super::process::path_hits_in("dirtool", std::slice::from_ref(&bin)).is_empty());
    assert!(super::process::path_hits_in("ghost", std::slice::from_ref(&bin)).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn classifies_nvm_fnm_and_volta_install_sources() {
    use std::path::Path;
    for (path, expected) in [
        ("/Users/dev/.nvm/versions/node/v22.11.0/bin/node", "nvm"),
        ("/Users/dev/.fnm/node-versions/v22.11.0/bin/node", "fnm"),
        (
            "/Users/dev/Library/Application Support/fnm_multishells/1_18/bin/node",
            "fnm",
        ),
        ("/Users/dev/.volta/bin/node", "volta"),
        ("/opt/homebrew/bin/node", "brew"),
        ("/Users/dev/.cargo/bin/cargo", "rustup"),
        ("/usr/bin/git", "standalone"),
    ] {
        assert_eq!(classify_source(Path::new(path)), expected, "{path}");
    }
}

#[test]
fn node_is_a_detection_item_without_operations() {
    // Node 作为独立检测项(此前只有 nvm/fnm/vp 侧面体现,brew 版 node 无从检测)
    let node = TOOLS
        .iter()
        .find(|spec| spec.id == "node")
        .expect("TOOLS 缺少 node 检测项");
    assert_eq!(node.kind, ToolchainKind::Node);
    // node 不做装/卸:版本管理交给 nvm / fnm / vp,node::resolve 无 node 分支
    let caps = caps_for("node", true, Some("brew"), true);
    assert!(!caps.can_install && !caps.can_update && !caps.can_uninstall);
    assert!(!caps.can_switch && !caps.can_list_remote);
    assert_eq!(extract_semver("v26.13.1").as_deref(), Some("26.13.1"));
    assert_eq!(extract_semver("v22.11.0\n").as_deref(), Some("22.11.0"));
    for op in ["install", "update", "uninstall", "use"] {
        assert!(
            resolve_op("node", op, Some("22"), Some("brew")).is_err(),
            "node 不应支持 {op}"
        );
    }
}

#[test]
fn macos_hides_uninstalled_dotnet_and_powershell() {
    // 未安装:macOS 整条不展示,macOS 已安装/其他平台保留
    assert!(hidden_on_platform(Platform::Macos, "dotnet", false));
    assert!(hidden_on_platform(Platform::Macos, "pwsh", false));
    assert!(!hidden_on_platform(Platform::Macos, "dotnet", true));
    assert!(!hidden_on_platform(Platform::Macos, "pwsh", true));
    assert!(!hidden_on_platform(Platform::Windows, "dotnet", false));
    assert!(!hidden_on_platform(Platform::Windows, "pwsh", false));
    assert!(!hidden_on_platform(Platform::Linux, "dotnet", false));
    assert!(!hidden_on_platform(Platform::Linux, "pwsh", false));
    // 其余工具在任何平台都不裁剪
    for id in ["node", "rustup", "cargo", "git", "gh", "uv", "go"] {
        assert!(!hidden_on_platform(Platform::Macos, id, false), "{id}");
    }
}

#[test]
fn platform_filter_drops_only_missing_desktop_tools() {
    let mut statuses = fake_statuses(&[("dotnet", false), ("pwsh", false), ("node", true)]);
    retain_for_platform(Platform::Macos, &mut statuses);
    assert_eq!(
        statuses.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        vec!["node"]
    );

    let mut statuses = fake_statuses(&[("dotnet", true), ("pwsh", false), ("node", false)]);
    retain_for_platform(Platform::Macos, &mut statuses);
    // 已安装的 .NET 照常展示(给出版本与能力),未安装的 pwsh 与未安装的 node 保留
    assert_eq!(
        statuses.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        vec!["dotnet", "node"]
    );

    let mut statuses = fake_statuses(&[("dotnet", false), ("pwsh", false), ("go", false)]);
    retain_for_platform(Platform::Windows, &mut statuses);
    assert_eq!(
        statuses.len(),
        3,
        "Windows 上未安装的工具仍显示「未检测到」"
    );
}

fn fake_statuses(entries: &[(&str, bool)]) -> Vec<ToolchainStatus> {
    entries
        .iter()
        .map(|(id, found)| ToolchainStatus {
            id: id.to_string(),
            kind: ToolchainKind::Node,
            found: *found,
            version: None,
            path: None,
            source: None,
            versions: Vec::new(),
            account: None,
            caps: ToolchainCaps {
                can_install: false,
                can_update: false,
                can_uninstall: false,
                can_switch: false,
                can_list_remote: false,
            },
        })
        .collect()
}

// ---- TZD-157 复审 P1/P2:探针有界性与伪值拒绝 ─────────────────────────────

#[cfg(target_os = "macos")]
#[test]
fn shell_probe_is_bounded_when_rc_leaves_background_grandchildren() {
    use std::time::{Duration, Instant};

    // 复现质量工程师的 P1-1:rc 里留后台子孙进程,曾让读管道阶段无限等待
    let home = std::env::temp_dir().join(format!("repomeow-probe-bg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join(".zshrc"), "( sleep 30 ) &\n").unwrap();
    let started = Instant::now();
    let path = super::process::probe_login_shell_path_with_timeout(
        std::path::Path::new("/bin/zsh"),
        &[("ZDOTDIR", home.to_str().unwrap())],
        Duration::from_secs(2),
    );
    let elapsed = started.elapsed();
    let _ = std::fs::remove_dir_all(&home);
    // 探针把 PATH 写进文件、stdout/stderr 丢弃,没有管道可等:正常在秒级返回
    assert!(
        elapsed < Duration::from_secs(5),
        "登录 shell 探针被后台子孙拖住: {:?}",
        elapsed
    );
    // 真实 PATH 拿到时至少要有 /usr/bin 这类目录;拿不到则必须是明确的失败
    if let Some(path) = path {
        assert!(path.contains("/usr/bin"), "探针结果非真实 PATH: {path}");
    }
}

#[cfg(target_os = "macos")]
#[test]
fn shell_probe_ignores_stderr_echo_under_set_x() {
    use std::time::Duration;

    // P1-2:rc 里的 `set -x` 把 printf 命令回显到 stderr。旧实现把 stderr 拼进解析
    // 输入后 rfind 命中回显里的 `%s`,解析出伪值并抑制兜底目录;现在只认探针自己写的文件。
    let home = std::env::temp_dir().join(format!("repomeow-probe-x-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join(".zshrc"), "set -x\n").unwrap();
    let path = super::process::probe_login_shell_path_with_timeout(
        std::path::Path::new("/bin/zsh"),
        &[("ZDOTDIR", home.to_str().unwrap())],
        Duration::from_secs(5),
    );
    let _ = std::fs::remove_dir_all(&home);
    let path = path.expect("set -x 下仍应拿到真实 PATH(文件里只有 printf 的结果)");
    assert!(!path.contains("%s"), "set -x 回显污染进了 PATH: {path}");
    assert!(!path.contains("REPOMEOW_PATH"), "标记残留进了 PATH: {path}");
    assert!(path.contains("/usr/bin"), "PATH 不完整: {path}");
}

#[test]
fn rejects_fish_style_list_and_space_joined_paths() {
    // fish 的 $PATH 是列表:两种可能的形态都必须是「判失败」,不能给出伪值
    // 形态一:列表按元素展开,每行一个目录(新实现用 printf '%s\n' $PATH 后拼接,这是有效值)
    assert_eq!(
        super::process::shell_path_from_file("/opt/homebrew/bin\n/usr/bin\n/bin\n", true)
            .as_deref(),
        Some("/opt/homebrew/bin:/usr/bin:/bin")
    );
    // 形态一之误用:仍用 "$PATH" 逐元素打印,产生多组标记串联 → 判失败
    assert_eq!(
        super::process::shell_path_from_file(
            "REPOMEOW_PATH_BEGIN:/usr/bin:REPOMEOW_PATH_END\n\
             REPOMEOW_PATH_BEGIN:/opt/homebrew/bin:REPOMEOW_PATH_END\n",
            false
        ),
        None
    );
    // 形态二:列表按空格粘连成一个含空格的伪目录 → 判失败(交给兜底目录)
    assert_eq!(
        super::process::shell_path_from_file(
            "REPOMEOW_PATH_BEGIN:/opt/homebrew/bin /usr/bin /bin:REPOMEOW_PATH_END",
            false
        ),
        None
    );
    assert!(!super::process::is_valid_shell_path(
        "/opt/homebrew/bin /usr/bin /bin"
    ));
}

#[test]
fn rejects_pseudo_paths_from_echo_and_relative_entries() {
    // 回显里的 `%s` 伪值:含格式串 → 判失败
    assert!(!super::process::is_valid_shell_path("%s"));
    // 文件里出现多组标记(列表被逐元素展开 / 回显重复)一律判失败,不取其中任意一段
    assert_eq!(
        super::process::parse_shell_path_output(
            "REPOMEOW_PATH_BEGIN:/usr/bin:REPOMEOW_PATH_END\n\
             +printf 'REPOMEOW_PATH_BEGIN:%s:REPOMEOW_PATH_END' ...\n"
        ),
        None
    );
    assert_eq!(
        super::process::parse_shell_path_output(
            "REPOMEOW_PATH_BEGIN:/usr/bin:REPOMEOW_PATH_END\n\
             REPOMEOW_PATH_BEGIN:/opt/homebrew/bin:REPOMEOW_PATH_END\n"
        ),
        None
    );
    // 相对路径不是合法 PATH 段
    assert!(!super::process::is_valid_shell_path("bin:/usr/bin"));
    // 空值与只有分隔符
    assert!(!super::process::is_valid_shell_path(""));
    assert!(!super::process::is_valid_shell_path("::"));
    // 真实存在的目录才认
    assert!(super::process::is_valid_shell_path("/usr/bin:/bin"));
}

#[cfg(unix)]
#[test]
fn tool_probe_is_bounded_when_child_leaves_pipe_holding_grandchild() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    // 工具自己起后台子孙并立即退出:旧实现会在 wait_with_output 里等管道 EOF
    let dir = std::env::temp_dir().join(format!("repomeow-probe-tool-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("slow-tool");
    std::fs::write(&script, "#!/bin/sh\n( sleep 30 ) &\necho 1.0.0\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let started = Instant::now();
    let result = super::process::run_with_timeout_in(&script, &[], Duration::from_secs(2));
    let elapsed = started.elapsed();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        elapsed < Duration::from_secs(6),
        "工具探测被后台子孙拖住: {:?}",
        elapsed
    );
    // 管道被孙进程持有 → 超时后整组 kill,返回 None(调用方按「探测失败」处理)
    assert!(result.is_none() || result.as_ref().is_some_and(|(ok, _)| *ok));
}

#[cfg(unix)]
#[test]
fn run_with_timeout_still_reports_normal_tool_output() {
    use std::time::Duration;
    // 常规路径不能因为改成进程组 + 伴生线程读取而丢输出
    let (ok, output) = super::process::run_with_timeout_in(
        std::path::Path::new("/bin/sh"),
        &["-c", "echo 2.44.0; echo warn >&2"],
        Duration::from_secs(3),
    )
    .expect("普通子进程应正常返回");
    assert!(ok);
    assert!(output.contains("2.44.0"), "丢失 stdout: {output}");
    assert!(output.contains("warn"), "丢失 stderr: {output}");
}

#[cfg(target_os = "macos")]
#[test]
fn fish_branch_joins_list_path_from_probe_file() {
    use std::os::unix::fs::PermissionsExt;

    // 本机不必装 fish:用一个名为 fish 的 stub 复现「$PATH 是列表」的写文件行为
    let dir = std::env::temp_dir().join(format!("repomeow-fish-stub-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let stub = dir.join("fish");
    std::fs::write(
        &stub,
        "#!/bin/sh\nprintf '%s\\n' /opt/homebrew/bin /usr/bin /bin > \"$REPOMEOW_PATH_PROBE_FILE\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

    let path = super::process::probe_login_shell_path(&stub, &[]);
    let _ = std::fs::remove_dir_all(&dir);
    // 列表形态拼回成冒号分隔的 PATH,且通过形态校验(不是伪值)
    assert_eq!(
        path.as_deref(),
        Some("/opt/homebrew/bin:/usr/bin:/bin"),
        "fish 分支未把列表拼回 PATH"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn fish_branch_without_writing_file_falls_back_to_none() {
    use std::os::unix::fs::PermissionsExt;

    // shell 什么都没写(等价于 fish 报错 / 命令失败)→ 必须明确判失败,走兜底目录
    let dir = std::env::temp_dir().join(format!("repomeow-fish-fail-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let stub = dir.join("fish");
    std::fs::write(&stub, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

    let path = super::process::probe_login_shell_path(&stub, &[]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(path, None, "空探针结果必须判失败");
}
