use std::path::PathBuf;

use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::ToolchainRemoteVersion;

use super::process::{cli_hits_on_path, run_with_timeout};
use super::version::{
    natural_version_cmp, parse_nvm_available_table, parse_remote_tokens, parse_uv_python_remote,
    parse_vp_remote,
};

pub(super) fn list_toolchain_versions_blocking(
    tool: &str,
) -> AppResult<Vec<ToolchainRemoteVersion>> {
    let unsupported = || {
        Err(AppError::coded(
            ErrorCode::ToolchainOpUnsupported,
            format!("{tool} list_remote"),
        ))
    };
    let run =
        |exe: Option<PathBuf>, args: &[&str]| exe.and_then(|exe| run_with_timeout(&exe, args));
    let result = match tool {
        "uv" => run(
            cli_hits_on_path("uv").into_iter().next(),
            &["python", "list", "--only-downloads"],
        ),
        "nvm" => {
            #[cfg(windows)]
            let exe = cli_hits_on_path("nvm").into_iter().next();
            #[cfg(not(windows))]
            let exe: Option<PathBuf> = None;
            run(exe, &["list", "available"])
        }
        "fnm" => run(cli_hits_on_path("fnm").into_iter().next(), &["list-remote"]),
        "vp" => run(
            cli_hits_on_path("vp").into_iter().next(),
            &["env", "list-remote"],
        ),
        _ => None,
    };
    let Some((true, output)) = result else {
        return unsupported();
    };
    let mut versions = match tool {
        "nvm" => parse_nvm_available_table(&output),
        "vp" => parse_vp_remote(&output),
        "uv" => parse_uv_python_remote(&output),
        _ => parse_remote_tokens(&output)
            .into_iter()
            .map(|name| ToolchainRemoteVersion { name, tag: None })
            .collect(),
    };
    versions.sort_by(|a, b| natural_version_cmp(&b.name, &a.name));
    Ok(versions)
}
