use std::collections::HashSet;
use std::time::Duration;

use serde::Deserialize;

use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::{JdkVendor, RemoteJdkRelease};

/// Zulu 全量列表一次拉取的条数:按最新在前排序,前 200 条覆盖近几个大版本的
/// 全部变体;jdk 8/11 靠后,由 list_zulu_releases 的兜底探测补齐
const ZULU_LIST_PAGE_SIZE: u32 = 200;
/// 下拉里要展示的 Zulu LTS 主版本(与 Adoptium 的 available_lts_releases 对齐)
const ZULU_LTS_MAJORS: [u32; 5] = [8, 11, 17, 21, 25];

/// 解析到的一个可下载发行包
pub(super) struct RemoteAsset {
    pub(super) url: String,
    pub(super) file_name: String,
    /// Windows 两个源都有 zip;非 Windows 的 Adoptium 只有 tar.gz(用系统 tar 解)
    pub(super) is_zip: bool,
    /// 展示用完整版本串(如 "17.0.20+8")
    version: String,
}

// -- API 响应结构(只建模用到的字段) --

/// Adoptium `/v3/info/available_releases`
#[derive(Deserialize)]
struct AdoptiumAvailable {
    available_lts_releases: Vec<u32>,
    most_recent_feature_release: u32,
}

/// Adoptium `/v3/assets/feature_releases/{major}/ga` 的单条发布
#[derive(Deserialize)]
struct AdoptiumRelease {
    /// 如 "jdk-17.0.20+8"
    release_name: String,
    #[serde(default)]
    binaries: Vec<AdoptiumBinary>,
}

#[derive(Deserialize)]
struct AdoptiumBinary {
    #[serde(default)]
    package: Option<AdoptiumPackage>,
}

#[derive(Deserialize)]
struct AdoptiumPackage {
    name: String,
    link: String,
}

/// Azul `/metadata/v1/zulu/packages/` 的单条包
#[derive(Deserialize)]
struct ZuluPackage {
    name: String,
    #[serde(default)]
    java_version: Vec<u64>,
    download_url: String,
}

pub(super) fn http_client(timeout: Option<Duration>) -> AppResult<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(timeout)
        .build()
        .map_err(|e| AppError::coded(ErrorCode::JdkInstallFailed, e.to_string()))
}

/// GET 一个 JSON 元数据端点,网络/状态码/解析错误统一映射 JdkInstallFailed,
/// message 携带 URL 与原因(前端经 GENERIC_DETAIL_CODES 附带展示)
fn http_json<T: serde::de::DeserializeOwned>(url: &str) -> AppResult<T> {
    let client = http_client(Some(Duration::from_secs(20)))?;
    let resp = client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| AppError::coded(ErrorCode::JdkInstallFailed, format!("{url}: {e}")))?;
    resp.json::<T>()
        .map_err(|e| AppError::coded(ErrorCode::JdkInstallFailed, format!("{url}: {e}")))
}

fn adoptium_os() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "mac"
    } else {
        "linux"
    }
}

fn adoptium_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x64"
    }
}

fn zulu_os() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macosx"
    } else {
        "linux"
    }
}

fn zulu_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x64"
    }
}

/// Zulu 包名里「纯 JDK zip」的判定:`-ca-jdk` 排除 fx(含 JavaFX)/crac/jre 变体,
/// `.zip` 结尾排除 msi/deb/rpm/tar.gz(安装到用户目录只接受解压即用的 zip)
pub(super) fn is_zulu_jdk_zip(name: &str) -> bool {
    name.contains("-ca-jdk") && name.ends_with(".zip")
}

/// Zulu 的 java_version 数组([17, 0, 20])拼成点分版本串
pub(super) fn zulu_version_label(parts: &[u64]) -> String {
    parts
        .iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(".")
}

/// 解析 Adoptium 某主版本最新 GA 发布的发行包
fn resolve_adoptium_asset(major: u32) -> AppResult<RemoteAsset> {
    let url = format!(
        "https://api.adoptium.net/v3/assets/feature_releases/{major}/ga?architecture={}&image_type=jdk&os={}&page_size=1",
        adoptium_arch(),
        adoptium_os(),
    );
    let releases: Vec<AdoptiumRelease> = http_json(&url)?;
    let release = releases.into_iter().next().ok_or_else(|| {
        AppError::coded(
            ErrorCode::JdkInstallFailed,
            format!("java {major}: no GA release"),
        )
    })?;
    // Windows 用 zip 解压安装;非 Windows 的 package 资产是 tar.gz
    let want_zip = cfg!(windows);
    let package = release
        .binaries
        .iter()
        .filter_map(|b| b.package.as_ref())
        .find(|p| {
            if want_zip {
                p.name.ends_with(".zip")
            } else {
                p.name.ends_with(".tar.gz")
            }
        })
        .ok_or_else(|| {
            AppError::coded(
                ErrorCode::JdkInstallFailed,
                format!("java {major}: no archive asset"),
            )
        })?;
    let version = release
        .release_name
        .strip_prefix("jdk-")
        .unwrap_or(&release.release_name)
        .to_string();
    Ok(RemoteAsset {
        url: package.link.clone(),
        file_name: package.name.clone(),
        is_zip: want_zip,
        version,
    })
}

/// 解析 Zulu 某主版本最新 GA 的纯 JDK zip
fn resolve_zulu_asset(major: u32) -> AppResult<RemoteAsset> {
    let url = format!(
        "https://api.azul.com/metadata/v1/zulu/packages/?java_version={major}&os={}&arch={}&package_type=jdk&release_status=ga&availability=ready_for_download&javafx=false&page=1&page_size=50",
        zulu_os(),
        zulu_arch(),
    );
    let packages: Vec<ZuluPackage> = http_json(&url)?;
    // 响应按新到旧排序,首个命中即该主版本最新
    let package = packages
        .iter()
        .find(|p| is_zulu_jdk_zip(&p.name) && !p.java_version.is_empty())
        .ok_or_else(|| {
            AppError::coded(
                ErrorCode::JdkInstallFailed,
                format!("java {major}: no jdk zip asset"),
            )
        })?;
    Ok(RemoteAsset {
        url: package.download_url.clone(),
        file_name: package.name.clone(),
        is_zip: true,
        version: zulu_version_label(&package.java_version),
    })
}

pub(super) fn resolve_asset(vendor: JdkVendor, major: u32) -> AppResult<RemoteAsset> {
    match vendor {
        JdkVendor::Adoptium => resolve_adoptium_asset(major),
        JdkVendor::Zulu => resolve_zulu_asset(major),
    }
}

/// Adoptium 可安装的大版本:LTS 全集 + 最新 feature 版(EA-only 的主版本查 ga 会落空,
/// 单项失败跳过不阻塞整个列表)
pub(super) fn list_adoptium_releases() -> AppResult<Vec<RemoteJdkRelease>> {
    let available: AdoptiumAvailable =
        http_json("https://api.adoptium.net/v3/info/available_releases")?;
    let mut majors = available.available_lts_releases;
    let latest = available.most_recent_feature_release;
    if !majors.contains(&latest) {
        majors.push(latest);
    }
    majors.sort_unstable_by(|a, b| b.cmp(a));
    let mut out = Vec::new();
    for major in majors {
        if let Ok(asset) = resolve_adoptium_asset(major) {
            out.push(RemoteJdkRelease {
                major,
                version: asset.version,
            });
        }
    }
    Ok(out)
}

/// Zulu 可安装的大版本:一次全量列表按主版本去重(取各自最新),下拉只保留
/// LTS 主版本 + 最新的两个非 LTS;jdk 8/11 排不进前 200 条时单独探测兜底
pub(super) fn list_zulu_releases() -> AppResult<Vec<RemoteJdkRelease>> {
    let url = format!(
        "https://api.azul.com/metadata/v1/zulu/packages/?os={}&arch={}&package_type=jdk&release_status=ga&availability=ready_for_download&javafx=false&page=1&page_size={ZULU_LIST_PAGE_SIZE}",
        zulu_os(),
        zulu_arch(),
    );
    let packages: Vec<ZuluPackage> = http_json(&url)?;
    let mut seen: HashSet<u32> = HashSet::new();
    let mut all: Vec<RemoteJdkRelease> = Vec::new();
    for package in &packages {
        if !is_zulu_jdk_zip(&package.name) || package.java_version.is_empty() {
            continue;
        }
        let major = package.java_version[0] as u32;
        if seen.insert(major) {
            all.push(RemoteJdkRelease {
                major,
                version: zulu_version_label(&package.java_version),
            });
        }
    }
    for major in ZULU_LTS_MAJORS {
        if seen.contains(&major) {
            continue;
        }
        if let Ok(asset) = resolve_zulu_asset(major) {
            all.push(RemoteJdkRelease {
                major,
                version: asset.version,
            });
        }
    }
    all.sort_unstable_by(|a, b| b.major.cmp(&a.major));
    // 下拉降噪:与 Adoptium 列表规模对齐(LTS + 最近两个大版本)
    let newest = all.first().map(|r| r.major).unwrap_or(0);
    let second = all.get(1).map(|r| r.major).unwrap_or(0);
    Ok(all
        .into_iter()
        .filter(|r| ZULU_LTS_MAJORS.contains(&r.major) || r.major == newest || r.major == second)
        .collect())
}

/// 列出某安装源可在线安装的 JDK 大版本(设置页在线安装对话框的版本下拉)
pub(super) async fn list_remote_jdks(vendor: JdkVendor) -> AppResult<Vec<RemoteJdkRelease>> {
    tokio::task::spawn_blocking(move || match vendor {
        JdkVendor::Adoptium => list_adoptium_releases(),
        JdkVendor::Zulu => list_zulu_releases(),
    })
    .await
    .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))?
}
