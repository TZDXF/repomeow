use crate::error::AppResult;

use super::operation::{unsupported, winget};

pub(super) fn resolve(tool: &str, op: &str) -> AppResult<String> {
    match tool {
        "rustup" => match op {
            "install" => Ok(install_command()),
            "update" => Ok("rustup update".to_string()),
            "uninstall" => Ok("rustup self uninstall -y".to_string()),
            _ => Err(unsupported(tool, op)),
        },
        "rustc" | "cargo" => match op {
            "install" => Ok(install_command()),
            "update" => Ok("rustup update".to_string()),
            _ => Err(unsupported(tool, op)),
        },
        _ => Err(unsupported(tool, op)),
    }
}

fn install_command() -> String {
    if cfg!(windows) {
        winget("install", "Rustlang.Rustup")
    } else if cfg!(target_os = "macos") {
        "brew install rustup".to_string()
    } else {
        r#"curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"#.to_string()
    }
}
