use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::ToolchainCaps;

use super::{dotnet, git, node, python, rust};

pub(super) fn manageable(source: Option<&str>) -> bool {
    if cfg!(windows) {
        true
    } else if cfg!(target_os = "macos") {
        source == Some("brew")
    } else {
        false
    }
}

pub(super) fn caps_for(
    id: &str,
    found: bool,
    source: Option<&str>,
    rustup_found: bool,
) -> ToolchainCaps {
    match id {
        "rustup" | "vp" => ToolchainCaps {
            can_install: !found,
            can_update: found,
            can_uninstall: found,
            can_switch: id == "vp" && found,
            can_list_remote: id == "vp" && found,
        },
        "rustc" | "cargo" => ToolchainCaps {
            can_install: !found && !rustup_found,
            can_update: found && rustup_found,
            can_uninstall: false,
            can_switch: false,
            can_list_remote: false,
        },
        "uv" => ToolchainCaps {
            can_install: !found,
            can_update: found,
            can_uninstall: found,
            can_switch: found,
            can_list_remote: found,
        },
        "nvm" | "fnm" => {
            let manageable = found && manageable(source);
            ToolchainCaps {
                can_install: !found,
                can_update: manageable,
                can_uninstall: manageable,
                can_switch: found,
                can_list_remote: id == "fnm" && found,
            }
        }
        _ => {
            let manageable = found && manageable(source);
            ToolchainCaps {
                can_install: !found,
                can_update: manageable,
                can_uninstall: manageable,
                can_switch: false,
                can_list_remote: false,
            }
        }
    }
}

pub(super) fn winget(action: &str, id: &str) -> String {
    format!("winget {action} --id {id} -e")
}

pub(super) fn unsupported(tool: &str, op: &str) -> AppError {
    AppError::coded(ErrorCode::ToolchainOpUnsupported, format!("{tool} {op}"))
}

pub(super) fn sanitize_version(version: &str) -> AppResult<&str> {
    let valid = !version.is_empty()
        && version.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | '/')
        });
    if valid {
        Ok(version)
    } else {
        Err(AppError::coded(
            ErrorCode::ToolchainVersionInvalid,
            version.to_string(),
        ))
    }
}

pub(super) fn resolve_op(
    tool: &str,
    op: &str,
    version: Option<&str>,
    source: Option<&str>,
) -> AppResult<String> {
    let version = match version.map(str::trim).filter(|version| !version.is_empty()) {
        Some(version) => Some(sanitize_version(version)?),
        None => None,
    };
    match tool {
        "rustup" | "rustc" | "cargo" => rust::resolve(tool, op),
        "uv" => python::resolve(op, version, source),
        "nvm" | "fnm" | "vp" => node::resolve(tool, op, version, source),
        "dotnet" => dotnet::resolve(op, version, source),
        "git" | "gh" => git::resolve(tool, op, source),
        _ => Err(unsupported(tool, op)),
    }
}
