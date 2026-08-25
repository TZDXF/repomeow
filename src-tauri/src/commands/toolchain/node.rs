use crate::error::AppResult;

use super::operation::{unsupported, winget};

pub(super) fn resolve(
    tool: &str,
    op: &str,
    version: Option<&str>,
    source: Option<&str>,
) -> AppResult<String> {
    let need_version = || version.ok_or_else(|| unsupported(tool, op));
    match tool {
        "nvm" => match op {
            "install" => Ok(if cfg!(windows) {
                winget("install", "CoreyButler.NVMforWindows")
            } else if cfg!(target_os = "macos") {
                "brew install nvm".to_string()
            } else {
                "curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.3/install.sh | bash"
                    .to_string()
            }),
            "update" | "uninstall" => {
                resolve_package_op(tool, op, source, "CoreyButler.NVMforWindows")
            }
            "use" => Ok(if cfg!(windows) {
                format!("nvm use {}", need_version()?)
            } else {
                format!("nvm alias default {}", need_version()?)
            }),
            "install_version" => Ok(format!("nvm install {}", need_version()?)),
            "uninstall_version" => Ok(format!("nvm uninstall {}", need_version()?)),
            _ => Err(unsupported(tool, op)),
        },
        "fnm" => match op {
            "install" => Ok(if cfg!(windows) {
                winget("install", "Schniz.fnm")
            } else if cfg!(target_os = "macos") {
                "brew install fnm".to_string()
            } else {
                "curl -fsSL https://fnm.vercel.app/install | bash".to_string()
            }),
            "update" | "uninstall" => resolve_package_op(tool, op, source, "Schniz.fnm"),
            "use" => Ok(format!("fnm default {}", need_version()?)),
            "install_version" => Ok(format!("fnm install {}", need_version()?)),
            "uninstall_version" => Ok(format!("fnm uninstall {}", need_version()?)),
            _ => Err(unsupported(tool, op)),
        },
        "vp" => match op {
            "install" => Ok(if cfg!(windows) {
                r#"powershell -NoProfile -Command "irm https://vite.plus/ps1 | iex""#.to_string()
            } else {
                "curl -fsSL https://vite.plus | bash".to_string()
            }),
            "update" => Ok("vp upgrade".to_string()),
            "uninstall" => Ok("vp implode".to_string()),
            "use" => Ok(format!("vp env default {}", need_version()?)),
            "install_version" => Ok(format!("vp env install {}", need_version()?)),
            "uninstall_version" => Ok(format!("vp env uninstall {}", need_version()?)),
            _ => Err(unsupported(tool, op)),
        },
        _ => Err(unsupported(tool, op)),
    }
}

fn resolve_package_op(
    tool: &str,
    op: &str,
    source: Option<&str>,
    winget_id: &str,
) -> AppResult<String> {
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
