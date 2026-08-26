use crate::error::AppResult;

use super::operation::{unsupported, winget};

pub(super) fn resolve(tool: &str, op: &str, source: Option<&str>) -> AppResult<String> {
    match tool {
        "git" => match op {
            "install" => Ok(install_command("git")),
            "update" | "uninstall" => package_op(tool, op, source, "Git.Git"),
            _ => Err(unsupported(tool, op)),
        },
        "gh" => match op {
            "install" => Ok(install_command("gh")),
            "login" => Ok("gh auth login".to_string()),
            "update" | "uninstall" => package_op(tool, op, source, "GitHub.cli"),
            _ => Err(unsupported(tool, op)),
        },
        _ => Err(unsupported(tool, op)),
    }
}

fn install_command(tool: &str) -> String {
    if cfg!(windows) {
        winget(
            "install",
            if tool == "git" {
                "Git.Git"
            } else {
                "GitHub.cli"
            },
        )
    } else if cfg!(target_os = "macos") {
        format!("brew install {tool}")
    } else {
        format!("sudo apt-get install -y {tool}")
    }
}

fn package_op(tool: &str, op: &str, source: Option<&str>, winget_id: &str) -> AppResult<String> {
    let action = if op == "update" {
        "upgrade"
    } else {
        "uninstall"
    };
    if cfg!(windows) {
        Ok(winget(action, winget_id))
    } else if cfg!(target_os = "macos") && source == Some("brew") {
        Ok(format!("brew {action} {tool}"))
    } else {
        Err(unsupported(tool, op))
    }
}
