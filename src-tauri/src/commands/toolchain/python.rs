use crate::error::AppResult;

use super::operation::{unsupported, winget};

pub(super) fn resolve(op: &str, version: Option<&str>, source: Option<&str>) -> AppResult<String> {
    let need_version = || version.ok_or_else(|| unsupported("uv", op));
    match op {
        "install" => Ok(if cfg!(windows) {
            r#"powershell -ExecutionPolicy ByPass -c "irm https://astral.sh/uv/install.ps1 | iex""#
                .to_string()
        } else {
            "curl -LsSf https://astral.sh/uv/install.sh | sh".to_string()
        }),
        "update" => Ok(if source == Some("winget") {
            winget("upgrade", "astral-sh.uv")
        } else if source == Some("brew") {
            "brew upgrade uv".to_string()
        } else {
            "uv self update".to_string()
        }),
        "uninstall" => Ok(match source {
            Some("winget") => winget("uninstall", "astral-sh.uv"),
            Some("brew") => "brew uninstall uv".to_string(),
            Some("rustup") => "cargo uninstall uv".to_string(),
            _ if cfg!(windows) => {
                r#"uv cache clean & del /f "%USERPROFILE%\.local\bin\uv.exe" "%USERPROFILE%\.local\bin\uvx.exe""#
                    .to_string()
            }
            _ => "uv cache clean; rm -f \"$HOME/.local/bin/uv\" \"$HOME/.local/bin/uvx\""
                .to_string(),
        }),
        "use" => Ok(format!("uv python install {} --default", need_version()?)),
        "install_version" => Ok(format!("uv python install {}", need_version()?)),
        "uninstall_version" => Ok(format!("uv python uninstall {}", need_version()?)),
        _ => Err(unsupported("uv", op)),
    }
}
