use crate::error::AppResult;

use super::operation::{unsupported, winget};
use super::process::{cli_hits_on_path, run_with_timeout};
use super::version::parse_dotnet_sdks;

pub(super) fn resolve(op: &str, version: Option<&str>, source: Option<&str>) -> AppResult<String> {
    let need_version = || version.ok_or_else(|| unsupported("dotnet", op));
    match op {
        "install" => {
            let major = need_version()?;
            Ok(if cfg!(windows) {
                winget("install", &format!("Microsoft.DotNet.SDK.{major}"))
            } else if cfg!(target_os = "macos") {
                "brew install --cask dotnet-sdk".to_string()
            } else {
                format!(
                    "curl -sSL https://dot.net/v1/dotnet-install.sh | bash -s -- --channel {major}"
                )
            })
        }
        "update" | "uninstall" => {
            let action = if op == "update" {
                "upgrade"
            } else {
                "uninstall"
            };
            if cfg!(windows) {
                let major = highest_major().ok_or_else(|| unsupported("dotnet", op))?;
                Ok(winget(action, &format!("Microsoft.DotNet.SDK.{major}")))
            } else if cfg!(target_os = "macos") && source == Some("brew") {
                Ok(format!("brew {action} --cask dotnet-sdk"))
            } else {
                Err(unsupported("dotnet", op))
            }
        }
        _ => Err(unsupported("dotnet", op)),
    }
}

fn highest_major() -> Option<String> {
    let exe = cli_hits_on_path("dotnet").into_iter().next()?;
    let Some((true, output)) = run_with_timeout(&exe, &["--list-sdks"]) else {
        return None;
    };
    parse_dotnet_sdks(&output)
        .iter()
        .filter_map(|version| {
            version
                .name
                .split('.')
                .next()
                .and_then(|major| major.parse::<u32>().ok())
        })
        .max()
        .map(|major| major.to_string())
}
