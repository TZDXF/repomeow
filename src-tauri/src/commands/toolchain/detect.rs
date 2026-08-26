use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::models::{ToolchainCaps, ToolchainKind, ToolchainStatus, ToolchainVersion};

use super::operation::caps_for;
use super::process::{
    classify_source, cli_hits_on_path, display_path, run_with_timeout, run_with_timeout_in,
};
use super::version::{
    parse_dotnet_sdks, parse_gh_auth_status, parse_nvm_list, parse_token_versions,
    parse_vp_env_list, probe_version, uv_python_versions,
};
#[cfg(not(windows))]
use super::{process::user_home_path, version::natural_version_cmp};

const AUTH_PROBE_TIMEOUT: Duration = Duration::from_secs(8);

pub(super) struct ToolSpec {
    pub(super) id: &'static str,
    kind: ToolchainKind,
    version_args: &'static [&'static str],
}

pub(super) const TOOLS: &[ToolSpec] = &[
    ToolSpec {
        id: "rustup",
        kind: ToolchainKind::Rust,
        version_args: &["--version"],
    },
    ToolSpec {
        id: "rustc",
        kind: ToolchainKind::Rust,
        version_args: &["--version"],
    },
    ToolSpec {
        id: "cargo",
        kind: ToolchainKind::Rust,
        version_args: &["--version"],
    },
    ToolSpec {
        id: "uv",
        kind: ToolchainKind::Python,
        version_args: &["--version"],
    },
    ToolSpec {
        id: "nvm",
        kind: ToolchainKind::Node,
        version_args: &["--version", "version"],
    },
    ToolSpec {
        id: "fnm",
        kind: ToolchainKind::Node,
        version_args: &["--version"],
    },
    ToolSpec {
        id: "vp",
        kind: ToolchainKind::Node,
        version_args: &["--version", "-v"],
    },
    ToolSpec {
        id: "dotnet",
        kind: ToolchainKind::Dotnet,
        version_args: &["--version"],
    },
    ToolSpec {
        id: "git",
        kind: ToolchainKind::Git,
        version_args: &["--version"],
    },
    ToolSpec {
        id: "gh",
        kind: ToolchainKind::Git,
        version_args: &["--version"],
    },
];

pub(super) fn detect_toolchains_blocking() -> Vec<ToolchainStatus> {
    let mut statuses: Vec<ToolchainStatus> = TOOLS.iter().map(detect_one).collect();
    let nvm_binary = statuses
        .iter()
        .any(|status| status.id == "nvm" && status.found);

    #[cfg(not(windows))]
    {
        if let Some(status) = statuses.iter_mut().find(|status| status.id == "nvm") {
            if !status.found {
                fill_unix_nvm(status);
            }
        }
    }

    for status in &mut statuses {
        if !status.found {
            continue;
        }
        let Some(path) = status.path.as_ref() else {
            continue;
        };
        let exe = PathBuf::from(path);
        status.versions = match status.id.as_str() {
            "nvm" => command_versions(&exe, &["list"], parse_nvm_list),
            "fnm" => command_versions(&exe, &["list"], parse_token_versions),
            "vp" => command_versions(&exe, &["env", "list"], parse_vp_env_list),
            "dotnet" => command_versions(&exe, &["--list-sdks"], parse_dotnet_sdks),
            "uv" => uv_python_versions(&exe),
            _ => Vec::new(),
        };
    }

    if let Some(status) = statuses
        .iter_mut()
        .find(|status| status.id == "gh" && status.found)
    {
        if let Some(exe) = status.path.as_ref().map(PathBuf::from) {
            status.account = run_with_timeout_in(&exe, &["auth", "status"], AUTH_PROBE_TIMEOUT)
                .filter(|(ok, _)| *ok)
                .and_then(|(_, output)| parse_gh_auth_status(&output));
        }
    }

    let rustup_found = statuses
        .iter()
        .any(|status| status.id == "rustup" && status.found);
    for status in &mut statuses {
        status.caps = caps_for(
            &status.id,
            status.found,
            status.source.as_deref(),
            rustup_found,
        );
        if status.id == "nvm" {
            status.caps.can_list_remote = nvm_binary;
        }
    }
    statuses
}

fn command_versions(
    exe: &Path,
    args: &[&str],
    parse: fn(&str) -> Vec<ToolchainVersion>,
) -> Vec<ToolchainVersion> {
    run_with_timeout(exe, args)
        .filter(|(ok, _)| *ok)
        .map(|(_, output)| parse(&output))
        .unwrap_or_default()
}

fn detect_one(spec: &ToolSpec) -> ToolchainStatus {
    let hit = cli_hits_on_path(spec.id).into_iter().next();
    let mut status = ToolchainStatus {
        id: spec.id.to_string(),
        kind: spec.kind,
        found: hit.is_some(),
        version: None,
        path: hit.as_ref().map(|path| display_path(path)),
        source: hit.as_ref().map(|path| classify_source(path)),
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
    if let Some(exe) = &hit {
        status.version = probe_version(exe, spec.version_args);
    }
    status
}

#[cfg(not(windows))]
pub(super) fn fill_unix_nvm(status: &mut ToolchainStatus) {
    let Some(home) = user_home_path() else {
        return;
    };
    let root = home.join(".nvm");
    if !root.is_dir() {
        return;
    }
    let mut names: Vec<String> = child_dir_names(&root.join("versions").join("node"))
        .into_iter()
        .filter_map(|name| {
            let bare = name.trim_start_matches('v').to_string();
            bare.starts_with(|character: char| character.is_ascii_digit())
                .then_some(bare)
        })
        .collect();
    names.sort_by(|a, b| natural_version_cmp(a, b));
    let alias = std::fs::read_to_string(root.join("alias").join("default"))
        .unwrap_or_default()
        .trim()
        .trim_start_matches('v')
        .to_string();
    let current_name = if let Some(exact) = names.iter().find(|name| **name == alias) {
        Some(exact.clone())
    } else {
        names
            .iter()
            .filter(|name| name.starts_with(&format!("{alias}.")))
            .max()
            .cloned()
    };
    status.found = true;
    status.version = None;
    status.path = Some(display_path(&root));
    status.source = Some("standalone".to_string());
    status.versions = names
        .into_iter()
        .map(|name| ToolchainVersion {
            current: Some(&name) == current_name.as_ref(),
            name,
        })
        .collect();
}

#[cfg(not(windows))]
fn child_dir_names(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}
