use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::JdkCandidate;

/// `java -version` 探测超时:坏掉的 JDK 不应拖住整个探测流程
const JAVA_VERSION_TIMEOUT: Duration = Duration::from_secs(3);

/// 自动探测本机 JDK(detect_jdks 的阻塞实现):扫常见安装目录 + PATH 上的 java
/// 反推 JAVA_HOME,逐个跑 `java -version` 取版本,按真实路径去重。
/// 子进程探测串行执行(每候选一个 -version,JDK 总数小),单候选超时兜底。
pub(super) fn detect_jdks_blocking() -> AppResult<Vec<JdkCandidate>> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut result = Vec::new();
    for home in candidate_homes() {
        let Some(java) = java_bin(&home) else {
            continue;
        };
        let key = norm_key(&home);
        if !seen.insert(key) {
            continue;
        }
        if let Some(version) = probe_java_version(&java) {
            result.push(JdkCandidate {
                path: display_path(&home),
                version,
            });
        }
    }
    result.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(result)
}

/// 汇总 JDK 根目录候选(不校验,由调用方逐一验证)
fn candidate_homes() -> Vec<PathBuf> {
    let mut homes = Vec::new();
    // PATH 上的 java 优先:最可能正在被使用
    for java in java_paths_on_path() {
        // bin/java(.exe) 的上两级即 JAVA_HOME;WindowsApps 下的 java.exe 是
        // Store 存根(执行会拉起商店),跳过
        if let Some(home) = java.parent().and_then(Path::parent) {
            let s = home.to_string_lossy();
            if cfg!(windows) && s.contains("WindowsApps") {
                continue;
            }
            homes.push(home.to_path_buf());
        }
    }
    #[cfg(windows)]
    {
        if let Some(pf) = env_dir("ProgramFiles") {
            for vendor in [
                "Java",
                "Eclipse Adoptium",
                "Microsoft",
                "Amazon Corretto",
                "Zulu",
                "BellSoft",
            ] {
                homes.extend(child_dirs(&pf.join(vendor)));
            }
        }
        // IntelliJ IDEA 下载/登记的 JDK
        if let Some(user) = env_dir("USERPROFILE") {
            homes.extend(child_dirs(&user.join(".jdks")));
        }
    }
    #[cfg(target_os = "macos")]
    {
        homes.extend(
            child_dirs(&PathBuf::from("/Library/Java/JavaVirtualMachines"))
                .into_iter()
                .map(|d| d.join("Contents").join("Home")),
        );
    }
    #[cfg(target_os = "linux")]
    {
        homes.extend(child_dirs(&PathBuf::from("/usr/lib/jvm")));
        if let Some(user) = env_dir("HOME") {
            homes.extend(child_dirs(&user.join(".sdkman/candidates/java")));
        }
    }
    homes
}

/// where/which java 的全部命中路径(找不到或执行失败返回空)
fn java_paths_on_path() -> Vec<PathBuf> {
    #[cfg(windows)]
    let probe = crate::commands::open::hidden(std::process::Command::new("where"))
        .arg("java")
        .output();
    #[cfg(not(windows))]
    let probe = std::process::Command::new("which").arg("java").output();
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

pub(super) fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn child_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

/// JDK 根目录下的 java 可执行文件;不存在则该候选无效
pub(super) fn java_bin(home: &Path) -> Option<PathBuf> {
    if cfg!(windows) {
        let exe = home.join("bin").join("java.exe");
        if exe.is_file() {
            return Some(exe);
        }
        None
    } else {
        let bin = home.join("bin").join("java");
        if bin.is_file() {
            return Some(bin);
        }
        None
    }
}

/// 去重用的稳定 key:canonicalize 失败(目录已消失等)回退原路径
fn norm_key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// 展示/入库路径:剥掉 canonicalize 带来的 \\?\ 前缀
pub(super) fn display_path(path: &Path) -> String {
    let s = path.to_string_lossy().into_owned();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

/// 跑 `<java> -version` 并解析版本串;失败/超时返回 None。
/// 版本信息输出在 stderr(`openjdk version "17.0.2" 2022-...`),
/// 兼容个别发行版输出到 stdout 的实现。
pub(super) fn probe_java_version(java: &Path) -> Option<String> {
    let mut child = crate::commands::open::hidden(std::process::Command::new(java))
        .arg("-version")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + JAVA_VERSION_TIMEOUT;
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
    parse_java_version(&String::from_utf8_lossy(&out.stderr))
        .or_else(|| parse_java_version(&String::from_utf8_lossy(&out.stdout)))
}

/// 从 `-version` 输出首行提取 `version "x.y.z"` 的引号内内容
pub(super) fn parse_java_version(text: &str) -> Option<String> {
    let start = text.find("version \"")? + "version \"".len();
    let rest = &text[start..];
    let end = rest.find('"')?;
    let version = &rest[..end];
    if version.is_empty() {
        None
    } else {
        Some(version.to_string())
    }
}

/// 自动探测本机 JDK 候选(设置页「自动探测」按钮调用)。
/// 串行子进程探测总时长可能到秒级,放入 spawn_blocking 避免阻塞。
pub(super) async fn detect_jdks() -> AppResult<Vec<JdkCandidate>> {
    tokio::task::spawn_blocking(detect_jdks_blocking)
        .await
        .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))?
}

/// 校验手动添加的 JDK 根目录:bin/java 存在且 -version 可解析,
/// 成功返回版本串(前端用于展示与默认命名),失败返回 JdkInvalid
pub(super) fn check_jdk(path: String) -> AppResult<String> {
    let home = PathBuf::from(&path);
    let Some(java) = java_bin(&home) else {
        return Err(AppError::coded(ErrorCode::JdkInvalid, path));
    };
    probe_java_version(&java).ok_or_else(|| AppError::coded(ErrorCode::JdkInvalid, path))
}
