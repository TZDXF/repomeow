use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Deserialize;
use tauri::{AppHandle, Emitter};

use crate::commands::walk;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::{
    JavaBuildGroup, JavaBuildTool, JavaCommandAction, JdkCandidate, JdkVendor, RemoteJdkRelease,
};

/// 构建文件内容读取上限:spring-boot 标记检测只需读文件头部,超大文件截断防卡顿
const MAX_BUILD_FILE_BYTES: usize = 1024 * 1024;
/// `java -version` 探测超时:坏掉的 JDK 不应拖住整个探测流程
const JAVA_VERSION_TIMEOUT: Duration = Duration::from_secs(3);

/// pom.xml 中判定「该模块可用 mvn spring-boot:run 运行」的标记:spring-boot-maven-plugin。
/// 不能用宽泛的 "spring-boot" 字符串:多模块工程里根聚合 pom 与依赖 spring-boot-starter
/// 的库模块都会命中,但它们没有可运行的主类,spring-boot:run 会失败
/// (实测 ruoyi-vue-pro:仅 yudao-server 等入口模块声明该插件)
const POM_MARKER: &str = "spring-boot-maven-plugin";
/// build.gradle(.kts) 中判定 Spring Boot 项目的标记(boot 插件 id)
const GRADLE_MARKER: &str = "org.springframework.boot";

/// 在已遍历的文件清单上提取 Spring Boot 构建分组(供合并扫描复用,避免重复 walk)。
/// 只收录构建文件声明了 spring-boot 运行插件的目录(见各 marker 注释),
/// 普通 Java 项目与多模块工程的库/聚合模块不产出;
/// 同一目录同时存在 build.gradle 与 build.gradle.kts 时按 (dir, tool) 去重。
pub(crate) fn java_builds_from_files(dir: &Path, files: &[PathBuf]) -> Vec<JavaBuildGroup> {
    let mut groups: Vec<JavaBuildGroup> = Vec::new();
    for rel in files {
        let file_name = rel.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let (tool, marker) = match file_name {
            "pom.xml" => (JavaBuildTool::Maven, POM_MARKER),
            "build.gradle" | "build.gradle.kts" => (JavaBuildTool::Gradle, GRADLE_MARKER),
            _ => continue,
        };
        let parent = rel.parent().filter(|p| !p.as_os_str().is_empty());
        let dir_rel = parent.map(walk::to_slash).unwrap_or_else(|| ".".into());
        if groups.iter().any(|g| g.dir == dir_rel && g.tool == tool) {
            continue;
        }
        let Some(content) = read_capped(&dir.join(rel)) else {
            continue;
        };
        if !content.contains(marker) {
            continue;
        }
        let (run_dir, run_command, more_actions) = build_run_spec(dir, &dir_rel, tool);
        groups.push(JavaBuildGroup {
            dir: dir_rel,
            tool,
            run_dir,
            run_command,
            more_actions,
        });
    }
    // 根目录优先,其余按目录字典序(与 package scripts 分组一致)
    groups.sort_by(|a, b| (&a.dir != ".", &a.dir).cmp(&(&b.dir != ".", &b.dir)));
    groups
}

/// 组装 Spring Boot 运行命令与常用操作。返回 (run_dir, run_command, more_actions),
/// run_dir 为相对项目根的目录("." = 项目根)。
///
/// 多模块工程不能在子模块目录里直接跑:子 pom 的父链与 dependencyManagement
/// import 的 BOM 是未安装的 SNAPSHOT,模块内 reactor 只有自己,解析必然失败
/// (实测 ruoyi-vue-pro 的 yudao-server)。两条捷径都走不通:
/// - 根目录 `mvn spring-boot:run -pl <模块> -am`:直接 goal 会作用于 -am 带入的
///   所有模块,在非应用模块上报「找不到主类」;
/// - `-pl <模块> -am install` 后再跑:import 的 BOM 模块(如 yudao-dependencies)
///   不在 -am 依赖闭包里,仍解析不到。
/// 因此子模块统一在根目录两段式:整仓 install -DskipTests 把父链/BOM/兄弟模块
/// 装进本地仓库,再 `-f <模块>/pom.xml` 单模块运行(此时全部可解析,插件版本
/// 来自模块自身声明,前缀也可从模块 pom 解析)。Gradle 原生支持任务路径,一条命令即可。
/// 生命周期目标(clean/package 等)没有直接 goal 的作用域问题:根 reactor 解析
/// 覆盖 BOM,子模块用 `-pl <模块> [-am]` 即可。
fn build_run_spec(
    root: &Path,
    dir_rel: &str,
    tool: JavaBuildTool,
) -> (String, String, Vec<JavaCommandAction>) {
    // wrapper 只在项目根探测(标准布局;gradle 子项目无独立 wrapper)
    let maven_cmd = if has_any(root, &["mvnw", "mvnw.cmd"]) {
        wrapper_name(JavaBuildTool::Maven)
    } else {
        "mvn"
    };
    let gradle_cmd = if has_any(root, &["gradlew", "gradlew.bat"]) {
        wrapper_name(JavaBuildTool::Gradle)
    } else {
        "gradle"
    };
    // 非 Windows 的 wrapper 需显式 ./ 前缀
    let (maven_cmd, gradle_cmd) = if cfg!(windows) {
        (maven_cmd.to_string(), gradle_cmd.to_string())
    } else {
        (format!("./{maven_cmd}"), format!("./{gradle_cmd}"))
    };

    let action = |key: &str, command: String| JavaCommandAction {
        key: key.to_string(),
        command,
    };

    let (command, more_actions) = match (tool, dir_rel) {
        (JavaBuildTool::Maven, ".") => (
            format!("{maven_cmd} spring-boot:run"),
            vec![
                action("java.clean", format!("{maven_cmd} clean")),
                action("java.package", format!("{maven_cmd} package -DskipTests")),
                action("java.install", format!("{maven_cmd} install -DskipTests")),
                action("java.test", format!("{maven_cmd} test")),
            ],
        ),
        (JavaBuildTool::Maven, module) => (
            format!(
                "{maven_cmd} install -DskipTests \
                 && {maven_cmd} -f {module}/pom.xml spring-boot:run"
            ),
            vec![
                action("java.clean", format!("{maven_cmd} clean -pl {module}")),
                action(
                    "java.package",
                    format!("{maven_cmd} package -DskipTests -pl {module} -am"),
                ),
                action(
                    "java.install",
                    format!("{maven_cmd} install -DskipTests -pl {module} -am"),
                ),
                action("java.test", format!("{maven_cmd} test -pl {module} -am")),
            ],
        ),
        (JavaBuildTool::Gradle, ".") => (
            format!("{gradle_cmd} bootRun"),
            vec![
                action("java.clean", format!("{gradle_cmd} clean")),
                action("java.build", format!("{gradle_cmd} build")),
                action("java.test", format!("{gradle_cmd} test")),
            ],
        ),
        // 子项目任务路径:a/b -> :a:b:bootRun
        (JavaBuildTool::Gradle, module) => {
            let path = module.replace('/', ":");
            (
                format!("{gradle_cmd} :{path}:bootRun"),
                vec![
                    action("java.clean", format!("{gradle_cmd} :{path}:clean")),
                    action("java.build", format!("{gradle_cmd} :{path}:build")),
                    action("java.test", format!("{gradle_cmd} :{path}:test")),
                ],
            )
        }
    };
    // 统一从项目根执行(子模块场景依赖根目录的 reactor/wrapper)
    (".".to_string(), command, more_actions)
}

/// wrapper 文件在 Windows 与类 Unix 上名字相同(mvnw/gradlew),无需平台分支
fn wrapper_name(tool: JavaBuildTool) -> &'static str {
    match tool {
        JavaBuildTool::Maven => "mvnw",
        JavaBuildTool::Gradle => "gradlew",
    }
}

fn has_any(dir: &Path, names: &[&str]) -> bool {
    names.iter().any(|n| dir.join(n).exists())
}

/// 读取文件内容,超过上限即截断(标记检测只需头部,UTF-8 无效字节按损失替换)
fn read_capped(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; MAX_BUILD_FILE_BYTES];
    let mut filled = 0;
    loop {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => {
                filled += n;
                if filled == buf.len() {
                    break;
                }
            }
            Err(_) => return None,
        }
    }
    buf.truncate(filled);
    Some(String::from_utf8_lossy(&buf).into_owned())
}

// ---- JDK 探测 ──────────────────────────────────────────────────────────────

/// 自动探测本机 JDK(detect_jdks 的阻塞实现):扫常见安装目录 + PATH 上的 java
/// 反推 JAVA_HOME,逐个跑 `java -version` 取版本,按真实路径去重。
/// 子进程探测串行执行(每候选一个 -version,JDK 总数小),单候选超时兜底。
fn detect_jdks_blocking() -> AppResult<Vec<JdkCandidate>> {
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

fn env_dir(var: &str) -> Option<PathBuf> {
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
fn java_bin(home: &Path) -> Option<PathBuf> {
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
fn display_path(path: &Path) -> String {
    let s = path.to_string_lossy().into_owned();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

/// 跑 `<java> -version` 并解析版本串;失败/超时返回 None。
/// 版本信息输出在 stderr(`openjdk version "17.0.2" 2022-...`),
/// 兼容个别发行版输出到 stdout 的实现。
fn probe_java_version(java: &Path) -> Option<String> {
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
fn parse_java_version(text: &str) -> Option<String> {
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

// ---- Tauri 命令包装 ────────────────────────────────────────────────────────

/// 自动探测本机 JDK 候选(设置页「自动探测」按钮调用)。
/// 串行子进程探测总时长可能到秒级,放入 spawn_blocking 避免阻塞。
#[tauri::command]
pub async fn detect_jdks() -> AppResult<Vec<JdkCandidate>> {
    tokio::task::spawn_blocking(detect_jdks_blocking)
        .await
        .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))?
}

/// 校验手动添加的 JDK 根目录:bin/java 存在且 -version 可解析,
/// 成功返回版本串(前端用于展示与默认命名),失败返回 JdkInvalid
#[tauri::command]
pub fn check_jdk(path: String) -> AppResult<String> {
    let home = PathBuf::from(&path);
    let Some(java) = java_bin(&home) else {
        return Err(AppError::coded(ErrorCode::JdkInvalid, path));
    };
    probe_java_version(&java).ok_or_else(|| AppError::coded(ErrorCode::JdkInvalid, path))
}

// ---- JDK 在线安装 ───────────────────────────────────────────────────────────

/// 安装进度事件(install_jdk 向前端 emit),stage = "download" | "extract"
const JDK_PROGRESS_EVENT: &str = "jdk://install-progress";
/// 下载进度事件的最小发射间隔(字节):约 2MB 一次,避免高频 IPC 刷屏
const PROGRESS_EMIT_BYTES: u64 = 2 * 1024 * 1024;
/// Zulu 全量列表一次拉取的条数:按最新在前排序,前 200 条覆盖近几个大版本的
/// 全部变体;jdk 8/11 靠后,由 list_zulu_releases 的兜底探测补齐
const ZULU_LIST_PAGE_SIZE: u32 = 200;
/// 下拉里要展示的 Zulu LTS 主版本(与 Adoptium 的 available_lts_releases 对齐)
const ZULU_LTS_MAJORS: [u32; 5] = [8, 11, 17, 21, 25];

/// 解析到的一个可下载发行包
struct RemoteAsset {
    url: String,
    file_name: String,
    /// Windows 两个源都有 zip;非 Windows 的 Adoptium 只有 tar.gz(用系统 tar 解)
    is_zip: bool,
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

fn http_client(timeout: Option<Duration>) -> AppResult<reqwest::blocking::Client> {
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
fn is_zulu_jdk_zip(name: &str) -> bool {
    name.contains("-ca-jdk") && name.ends_with(".zip")
}

/// Zulu 的 java_version 数组([17, 0, 20])拼成点分版本串
fn zulu_version_label(parts: &[u64]) -> String {
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

fn resolve_asset(vendor: JdkVendor, major: u32) -> AppResult<RemoteAsset> {
    match vendor {
        JdkVendor::Adoptium => resolve_adoptium_asset(major),
        JdkVendor::Zulu => resolve_zulu_asset(major),
    }
}

/// Adoptium 可安装的大版本:LTS 全集 + 最新 feature 版(EA-only 的主版本查 ga 会落空,
/// 单项失败跳过不阻塞整个列表)
fn list_adoptium_releases() -> AppResult<Vec<RemoteJdkRelease>> {
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
fn list_zulu_releases() -> AppResult<Vec<RemoteJdkRelease>> {
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

/// 安装根目录 ~/.jdks(与 IntelliJ 下载 JDK 的目录一致,自动探测也覆盖该目录)
fn jdks_root() -> AppResult<PathBuf> {
    let home = env_dir("USERPROFILE")
        .or_else(|| env_dir("HOME"))
        .ok_or_else(|| AppError::coded(ErrorCode::JdkInstallFailed, "no user home dir"))?;
    let root = home.join(".jdks");
    std::fs::create_dir_all(&root)?;
    Ok(root)
}

/// 下载发行包到临时文件,期间按字节量节流 emit 下载进度
fn download_with_progress(app: &AppHandle, url: &str, dest: &Path) -> AppResult<()> {
    // 不设总超时:近 200MB 的包在慢网下远超常规超时,连接超时已兜住坏网络
    let client = http_client(None)?;
    let mut resp = client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| AppError::coded(ErrorCode::JdkInstallFailed, format!("{url}: {e}")))?;
    let total = resp.content_length().unwrap_or(0);
    let mut file = std::fs::File::create(dest)?;
    let mut received = 0u64;
    let mut emitted = 0u64;
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        received += n as u64;
        if received - emitted >= PROGRESS_EMIT_BYTES {
            emitted = received;
            let _ = app.emit(
                JDK_PROGRESS_EVENT,
                serde_json::json!({ "stage": "download", "received": received, "total": total }),
            );
        }
    }
    file.flush()?;
    let _ = app.emit(
        JDK_PROGRESS_EVENT,
        serde_json::json!({ "stage": "download", "received": received, "total": total.max(received) }),
    );
    Ok(())
}

/// 解压 zip 到目标目录(enclosed_name 拒绝 zip-slip 路径穿越;Unix 上恢复可执行位)
fn extract_zip(archive: &Path, dest: &Path) -> AppResult<()> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| AppError::coded(ErrorCode::JdkInstallFailed, format!("zip: {e}")))?;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| AppError::coded(ErrorCode::JdkInstallFailed, format!("zip: {e}")))?;
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut outfile = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut outfile)?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode));
        }
    }
    Ok(())
}

/// 非 Windows 的 Adoptium tar.gz 用系统 tar 解(bsdtar/gnu tar 均支持 -xzf)
fn extract_tar_gz(archive: &Path, dest: &Path) -> AppResult<()> {
    let out = crate::commands::open::hidden(std::process::Command::new("tar"))
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .output()?;
    if !out.status.success() {
        return Err(AppError::coded(
            ErrorCode::JdkInstallFailed,
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

/// 递归复制目录(rename 跨卷失败时的兜底,如临时目录与用户目录不在同一盘)
fn copy_dir_recursive(src: &Path, dest: &Path) -> AppResult<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn move_dir(src: &Path, dest: &Path) -> AppResult<()> {
    if std::fs::rename(src, dest).is_ok() {
        return Ok(());
    }
    copy_dir_recursive(src, dest)?;
    let _ = std::fs::remove_dir_all(src);
    Ok(())
}

/// install_jdk 的阻塞实现:解析资产 -> 下载(进度事件)-> 解压 -> 挪进 ~/.jdks -> 校验。
/// 目标目录已是有效 JDK 时幂等返回(重复点击安装不报错)
fn install_jdk_blocking(app: &AppHandle, vendor: JdkVendor, major: u32) -> AppResult<JdkCandidate> {
    let asset = resolve_asset(vendor, major)?;
    let jdk_root = jdks_root()?;
    let tmp_file = std::env::temp_dir().join(format!(
        "repomeow-jdk-{}",
        asset.file_name.replace(['/', '\\'], "_")
    ));
    if let Err(e) = download_with_progress(app, &asset.url, &tmp_file) {
        let _ = std::fs::remove_file(&tmp_file);
        return Err(e);
    }
    let extract_dir = std::env::temp_dir().join(format!(
        "repomeow-jdk-extract-{}-{}",
        std::process::id(),
        crate::time_util::now_ts_nanos(),
    ));
    let extracted = (|| -> AppResult<PathBuf> {
        std::fs::create_dir_all(&extract_dir)?;
        let _ = app.emit(
            JDK_PROGRESS_EVENT,
            serde_json::json!({ "stage": "extract", "received": 0, "total": 0 }),
        );
        if asset.is_zip {
            extract_zip(&tmp_file, &extract_dir)?;
        } else {
            extract_tar_gz(&tmp_file, &extract_dir)?;
        }
        // 发行包标准布局:解压出来唯一一个顶层目录即 JDK 根
        let tops: Vec<PathBuf> = std::fs::read_dir(&extract_dir)?
            .flatten()
            .map(|e| e.path())
            .collect();
        if tops.len() != 1 || !tops[0].is_dir() {
            return Err(AppError::coded(
                ErrorCode::JdkInstallFailed,
                format!("unexpected archive layout in {}", tops.len()),
            ));
        }
        Ok(tops[0].clone())
    })();
    let _ = std::fs::remove_file(&tmp_file);
    let top = match extracted {
        Ok(top) => top,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&extract_dir);
            return Err(e);
        }
    };

    let target = jdk_root.join(top.file_name().unwrap_or_default());
    let result = (|| -> AppResult<JdkCandidate> {
        // 幂等:目标已存在且是有效 JDK 直接返回现有安装
        if java_bin(&target).is_some() {
            let version =
                probe_java_version(&java_bin(&target).expect("checked")).ok_or_else(|| {
                    AppError::coded(ErrorCode::JdkInstallFailed, display_path(&target))
                })?;
            return Ok(JdkCandidate {
                path: display_path(&target),
                version,
            });
        }
        if target.exists() {
            return Err(AppError::coded(
                ErrorCode::JdkInstallFailed,
                display_path(&target),
            ));
        }
        move_dir(&top, &target)?;
        let java = java_bin(&target)
            .ok_or_else(|| AppError::coded(ErrorCode::JdkInstallFailed, display_path(&target)))?;
        let version = probe_java_version(&java)
            .ok_or_else(|| AppError::coded(ErrorCode::JdkInstallFailed, display_path(&target)))?;
        Ok(JdkCandidate {
            path: display_path(&target),
            version,
        })
    })();
    let _ = std::fs::remove_dir_all(&extract_dir);
    result
}

/// 列出某安装源可在线安装的 JDK 大版本(设置页在线安装对话框的版本下拉)
#[tauri::command]
pub async fn list_remote_jdks(vendor: JdkVendor) -> AppResult<Vec<RemoteJdkRelease>> {
    tokio::task::spawn_blocking(move || match vendor {
        JdkVendor::Adoptium => list_adoptium_releases(),
        JdkVendor::Zulu => list_zulu_releases(),
    })
    .await
    .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))?
}

/// 在线下载并安装 JDK 到 ~/.jdks(install_jdk_blocking 见块注释)
#[tauri::command]
pub async fn install_jdk(app: AppHandle, vendor: JdkVendor, major: u32) -> AppResult<JdkCandidate> {
    tokio::task::spawn_blocking(move || install_jdk_blocking(&app, vendor, major))
        .await
        .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_project_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "repomeow-java-{tag}-{}-{}",
            std::process::id(),
            crate::time_util::now_ts_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_java_versions() {
        assert_eq!(
            parse_java_version(r#"openjdk version "17.0.2" 2022-01-18"#).as_deref(),
            Some("17.0.2")
        );
        assert_eq!(
            parse_java_version(r#"java version "1.8.0_392""#).as_deref(),
            Some("1.8.0_392")
        );
        assert_eq!(parse_java_version(r#"openjdk version """#), None);
        assert_eq!(parse_java_version("no version here"), None);
    }

    #[test]
    fn discovers_spring_boot_builds() {
        let dir = temp_project_dir("builds");
        // 根聚合 pom 只依赖 spring-boot(无运行插件)-> 跳过(多模块工程根不可 run)
        fs::write(
            dir.join("pom.xml"),
            "<project><modules><module>server</module></modules></project>",
        )
        .unwrap();
        // 入口模块声明 spring-boot-maven-plugin -> 可运行
        fs::create_dir_all(dir.join("server")).unwrap();
        fs::write(
            dir.join("server/pom.xml"),
            "<project><build><plugins><plugin><artifactId>spring-boot-maven-plugin</artifactId></plugin></plugins></build></project>",
        )
        .unwrap();
        // 无任何 spring 标记的普通 Java pom -> 跳过
        fs::create_dir_all(dir.join("plain")).unwrap();
        fs::write(
            dir.join("plain/pom.xml"),
            "<project><artifactId>demo</artifactId></project>",
        )
        .unwrap();
        // gradle 子项目 + 根目录 wrapper
        fs::create_dir_all(dir.join("svc")).unwrap();
        fs::write(
            dir.join("svc/build.gradle"),
            "plugins { id 'org.springframework.boot' version '3.2.0' }",
        )
        .unwrap();
        fs::write(dir.join("gradlew"), "#!/bin/sh\n").unwrap();

        let files: Vec<PathBuf> = [
            "pom.xml",
            "server/pom.xml",
            "plain/pom.xml",
            "svc/build.gradle",
            "gradlew",
        ]
        .iter()
        .map(|p| PathBuf::from(p.replace('/', std::path::MAIN_SEPARATOR_STR)))
        .collect();
        let groups = java_builds_from_files(&dir, &files);
        assert_eq!(groups.len(), 2);
        // maven 子模块:根目录两段式(install 后 -f 单模块运行)
        assert_eq!(groups[0].dir, "server");
        assert_eq!(groups[0].tool, JavaBuildTool::Maven);
        assert_eq!(groups[0].run_dir, ".");
        assert_eq!(
            groups[0].run_command,
            "mvn install -DskipTests && mvn -f server/pom.xml spring-boot:run"
        );
        // gradle 子项目:根目录任务路径
        assert_eq!(groups[1].dir, "svc");
        assert_eq!(groups[1].tool, JavaBuildTool::Gradle);
        assert_eq!(groups[1].run_dir, ".");
        if cfg!(windows) {
            assert_eq!(groups[1].run_command, "gradlew :svc:bootRun");
        } else {
            assert_eq!(groups[1].run_command, "./gradlew :svc:bootRun");
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn root_module_runs_directly() {
        let dir = temp_project_dir("root");
        fs::write(
            dir.join("pom.xml"),
            "<project><build><plugins><plugin><artifactId>spring-boot-maven-plugin</artifactId></plugin></plugins></build></project>",
        )
        .unwrap();

        let files = vec![PathBuf::from("pom.xml")];
        let groups = java_builds_from_files(&dir, &files);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].dir, ".");
        assert_eq!(groups[0].run_dir, ".");
        assert_eq!(groups[0].run_command, "mvn spring-boot:run");
        // 常用操作:根模块直接跟生命周期目标
        let action_cmds: Vec<&str> = groups[0]
            .more_actions
            .iter()
            .map(|a| a.command.as_str())
            .collect();
        assert_eq!(
            action_cmds,
            vec![
                "mvn clean",
                "mvn package -DskipTests",
                "mvn install -DskipTests",
                "mvn test"
            ]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dedups_gradle_variants() {
        let dir = temp_project_dir("gradle-dup");
        fs::create_dir_all(dir.join("app")).unwrap();
        fs::write(
            dir.join("app/build.gradle"),
            "dependencies { implementation 'org.springframework.boot:spring-boot-starter' }",
        )
        .unwrap();
        fs::write(
            dir.join("app/build.gradle.kts"),
            "plugins { id(\"org.springframework.boot\") }",
        )
        .unwrap();

        let files: Vec<PathBuf> = ["app/build.gradle", "app/build.gradle.kts"]
            .iter()
            .map(|p| PathBuf::from(p.replace('/', std::path::MAIN_SEPARATOR_STR)))
            .collect();
        let groups = java_builds_from_files(&dir, &files);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].dir, "app");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_invalid_jdk_dir() {
        let dir = temp_project_dir("jdk-invalid");
        assert!(matches!(
            check_jdk(dir.to_string_lossy().to_string()),
            Err(ref e) if e.is_code(ErrorCode::JdkInvalid)
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    /// 手动验证:对本机真实 JDK 跑探测与校验(cargo test real_world -- --ignored --nocapture)
    #[test]
    #[ignore]
    fn real_world_detect_jdks() {
        for c in detect_jdks_blocking().unwrap() {
            println!("detect: {} -> {}", c.path, c.version);
            let checked = check_jdk(c.path.clone()).unwrap();
            assert_eq!(checked, c.version);
        }
    }

    #[test]
    fn filters_zulu_jdk_zip_names() {
        // 纯 JDK zip 命中
        assert!(is_zulu_jdk_zip("zulu17.68.17-ca-jdk17.0.20-win_x64.zip"));
        assert!(is_zulu_jdk_zip("zulu8.84.0.15-ca-jdk8.0.452-win_x64.zip"));
        // fx / crac / jre 变体排除
        assert!(!is_zulu_jdk_zip(
            "zulu17.68.17-ca-fx-jdk17.0.20-win_x64.zip"
        ));
        assert!(!is_zulu_jdk_zip(
            "zulu17.66.19-ca-crac-jdk17.0.19-win_x64.zip"
        ));
        assert!(!is_zulu_jdk_zip("zulu17.68.17-ca-jre17.0.20-win_x64.zip"));
        // 非 zip 安装包排除
        assert!(!is_zulu_jdk_zip("zulu17.68.17-ca-jdk17.0.20-win_x64.msi"));
        assert!(!is_zulu_jdk_zip(
            "zulu17.68.17-ca-jdk17.0.20-win_x64.tar.gz"
        ));
    }

    #[test]
    fn labels_zulu_versions() {
        assert_eq!(zulu_version_label(&[17, 0, 20]), "17.0.20");
        assert_eq!(zulu_version_label(&[8, 0, 452]), "8.0.452");
        assert_eq!(zulu_version_label(&[]), "");
    }

    /// 覆盖 install_jdk 的解压->顶层目录->搬移路径(不触网):用 zip crate 造一个
    /// 标准布局(唯一顶层目录)的归档,验证 extract_zip 与 move_dir
    #[test]
    fn extracts_zip_and_moves_top_dir() {
        let dir = temp_project_dir("zip");
        let zip_path = dir.join("pkg.zip");
        let mut writer = zip::ZipWriter::new(fs::File::create(&zip_path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("jdk-17.0.1/bin/java", options).unwrap();
        writer.write_all(b"fake").unwrap();
        writer.start_file("jdk-17.0.1/release", options).unwrap();
        writer.write_all(b"JAVA_VERSION=17").unwrap();
        writer.finish().unwrap();

        let extract_dir = dir.join("out");
        fs::create_dir_all(&extract_dir).unwrap();
        extract_zip(&zip_path, &extract_dir).unwrap();
        let tops: Vec<PathBuf> = fs::read_dir(&extract_dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        assert_eq!(tops.len(), 1);
        assert!(tops[0].is_dir());

        let target = dir.join("dest");
        move_dir(&tops[0], &target).unwrap();
        assert!(target.join("bin").join("java").is_file());
        assert!(target.join("release").is_file());

        let _ = fs::remove_dir_all(&dir);
    }

    /// 手动验证:对两个安装源拉真实元数据(cargo test real_world -- --ignored --nocapture)
    #[test]
    #[ignore]
    fn real_world_list_remote_jdks() {
        for vendor in [JdkVendor::Adoptium, JdkVendor::Zulu] {
            match vendor {
                JdkVendor::Adoptium => {
                    for r in list_adoptium_releases().unwrap() {
                        println!("adoptium: java {} ({})", r.major, r.version);
                    }
                }
                JdkVendor::Zulu => {
                    for r in list_zulu_releases().unwrap() {
                        println!("zulu: java {} ({})", r.major, r.version);
                    }
                }
            }
        }
    }
}
