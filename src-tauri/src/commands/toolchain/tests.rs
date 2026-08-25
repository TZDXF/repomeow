use std::collections::HashSet;

use crate::error::ErrorCode;
use crate::models::{ToolchainCaps, ToolchainRemoteVersion, ToolchainVersion};

#[cfg(not(windows))]
use super::detect::fill_unix_nvm;
use super::detect::TOOLS;
use super::operation::{caps_for, resolve_op, sanitize_version};
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
