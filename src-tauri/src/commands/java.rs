use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::commands::walk;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::{JavaBuildGroup, JavaBuildTool, JavaCommandAction, JdkCandidate};

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
        if groups
            .iter()
            .any(|g| g.dir == dir_rel && g.tool == tool)
        {
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
    let probe = crate::commands::open::hidden(std::process::Command::new("where")).arg("java").output();
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_project_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "repomeow-java-{tag}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
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
        fs::write(dir.join("plain/pom.xml"), "<project><artifactId>demo</artifactId></project>").unwrap();
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
        let action_cmds: Vec<&str> = groups[0].more_actions.iter().map(|a| a.command.as_str()).collect();
        assert_eq!(
            action_cmds,
            vec!["mvn clean", "mvn package -DskipTests", "mvn install -DskipTests", "mvn test"]
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
}
