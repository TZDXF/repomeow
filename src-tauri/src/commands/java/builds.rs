use std::io::Read;
use std::path::{Path, PathBuf};

use crate::commands::walk;
use crate::models::{JavaBuildGroup, JavaBuildTool, JavaCommandAction};

/// 构建文件内容读取上限:spring-boot 标记检测只需读文件头部,超大文件截断防卡顿
const MAX_BUILD_FILE_BYTES: usize = 1024 * 1024;

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
