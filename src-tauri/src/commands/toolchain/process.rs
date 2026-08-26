use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::commands::open::hidden;

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// where/which 命中的路径(找不到或执行失败返回空)。
pub(super) fn cli_hits_on_path(cli: &str) -> Vec<PathBuf> {
    #[cfg(windows)]
    let probe = hidden(Command::new("where")).arg(cli).output();
    #[cfg(not(windows))]
    let probe = Command::new("which").arg(cli).output();
    let out = match probe {
        Ok(out) if out.status.success() => out,
        _ => return Vec::new(),
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// 展示用路径:剥掉 canonicalize 带来的 `\\?\` 前缀。
pub(super) fn display_path(path: &Path) -> String {
    let value = path.to_string_lossy().into_owned();
    value.strip_prefix(r"\\?\").unwrap_or(&value).to_string()
}

/// 按可执行文件路径推断安装来源。
pub(super) fn classify_source(path: &Path) -> String {
    let value = path.to_string_lossy().to_lowercase();
    if value.contains("winget") {
        "winget".to_string()
    } else if value.contains(r"\.cargo") || value.contains("/.cargo") {
        "rustup".to_string()
    } else if value.contains("/opt/homebrew/")
        || value.contains("/usr/local/cellar")
        || value.contains("/home/linuxbrew")
    {
        "brew".to_string()
    } else {
        "standalone".to_string()
    }
}

/// 带默认超时跑命令并合并 stdout+stderr。
pub(super) fn run_with_timeout(exe: &Path, args: &[&str]) -> Option<(bool, String)> {
    run_with_timeout_in(exe, args, PROBE_TIMEOUT)
}

/// 同上，允许为联网探测指定更长超时。
pub(super) fn run_with_timeout_in(
    exe: &Path,
    args: &[&str],
    timeout: Duration,
) -> Option<(bool, String)> {
    let mut child = hidden(Command::new(exe))
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
    let out = child.wait_with_output().ok()?;
    Some((
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    ))
}

pub(super) fn user_home_path() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}
