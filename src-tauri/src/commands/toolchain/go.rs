use crate::error::AppResult;

use super::operation::{unsupported, winget};

pub(super) fn resolve(tool: &str, op: &str, source: Option<&str>) -> AppResult<String> {
    match tool {
        "go" => match op {
            "install" => Ok(install_command()),
            "update" | "uninstall" => package_op(op, source),
            _ => Err(unsupported(tool, op)),
        },
        _ => Err(unsupported(tool, op)),
    }
}

fn install_command() -> String {
    if cfg!(windows) {
        winget("install", "GoLang.Go")
    } else if cfg!(target_os = "macos") {
        "brew install go".to_string()
    } else {
        "sudo apt-get install -y golang-go".to_string()
    }
}

fn package_op(op: &str, source: Option<&str>) -> AppResult<String> {
    let action = if op == "update" {
        "upgrade"
    } else {
        "uninstall"
    };
    if cfg!(windows) {
        Ok(winget(action, "GoLang.Go"))
    } else if cfg!(target_os = "macos") && source == Some("brew") {
        Ok(format!("brew {action} go"))
    } else {
        Err(unsupported("go", op))
    }
}
