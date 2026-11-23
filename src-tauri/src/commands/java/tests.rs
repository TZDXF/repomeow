use std::fs;
use std::io::Write;
use std::path::PathBuf;

use super::builds::java_builds_from_files;
use super::detection::{check_jdk, detect_jdks_blocking, parse_java_version};
use super::install::{extract_zip, move_dir};
use super::remote::{
    is_zulu_jdk_zip, list_adoptium_releases, list_openjdk_releases, list_zulu_releases,
    openjdk_file_name, zulu_version_label,
};
use crate::error::ErrorCode;
use crate::models::{JavaBuildTool, JdkVendor};

fn temp_project_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "repomeow-java-{tag}-{}-{}",
        std::process::id(),
        crate::time_util::now_ts_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// 期望的 wrapper 命令前缀:类 Unix 需 `./`(项目内相对路径),Windows 直接写裸名。
fn wrapper_prefix(name: &str) -> String {
    if cfg!(windows) {
        name.to_string()
    } else {
        format!("./{name}")
    }
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
    // maven 子模块:根目录两段式(install 后 -f 单模块运行)。
    // fixture 只有 gradlew 没有 mvnw,所以 maven 侧必须走系统 `mvn`(不带 `./`),
    // 而下面 gradle 侧因为是真 wrapper 才带前缀 —— 两者共同钉住「前缀跟 wrapper 走」。
    assert_eq!(groups[0].dir, "server");
    assert_eq!(groups[0].tool, JavaBuildTool::Maven);
    assert_eq!(groups[0].run_dir, ".");
    assert_eq!(
        groups[0].run_command,
        "mvn install -DskipTests && mvn -f server/pom.xml spring-boot:run \
         \"-Dspring-boot.run.fork=false\" \"-Dspring-boot.run.useTestClasspath=false\""
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
    // 无 mvnw → 用系统 `mvn`,**不加** `./` 前缀:加了会在项目目录里执行不存在的文件,
    // 使 macOS/Linux 上的运行与生命周期命令全部失败。
    assert_eq!(
        groups[0].run_command,
        "mvn spring-boot:run \"-Dspring-boot.run.fork=false\" \"-Dspring-boot.run.useTestClasspath=false\""
    );
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

/// 命令前缀只取决于「有没有 wrapper」,不取决于平台:有 wrapper 才加 `./`(unix 下
/// 项目内相对路径必须带前缀才能执行),回退系统命令时不加。否则无 wrapper 的工程在
/// macOS/Linux 上会去项目目录找不存在的 `./mvn`/`./gradle`。
#[test]
fn wrapper_prefix_follows_wrapper_not_platform() {
    let dir = temp_project_dir("wrapper-prefix");
    fs::write(
        dir.join("pom.xml"),
        "<project><build><plugins><plugin><artifactId>spring-boot-maven-plugin</artifactId></plugin></plugins></build></project>",
    )
    .unwrap();
    // 两个 wrapper 都在项目根(标准布局),都留在文件清单里以便 maven/gradle 各出一组
    fs::write(dir.join("mvnw"), "#!/bin/sh\n").unwrap();
    fs::write(dir.join("gradlew"), "#!/bin/sh\n").unwrap();
    fs::create_dir_all(dir.join("svc")).unwrap();
    fs::write(
        dir.join("svc/build.gradle"),
        "plugins { id 'org.springframework.boot' version '3.2.0' }",
    )
    .unwrap();

    let files: Vec<PathBuf> = ["pom.xml", "svc/build.gradle"]
        .iter()
        .map(|p| PathBuf::from(p.replace('/', std::path::MAIN_SEPARATOR_STR)))
        .collect();
    let groups = java_builds_from_files(&dir, &files);
    assert_eq!(groups.len(), 2);

    let (maven, gradle) = (wrapper_prefix("mvnw"), wrapper_prefix("gradlew"));
    let maven_group = groups
        .iter()
        .find(|g| g.tool == JavaBuildTool::Maven)
        .unwrap();
    assert!(
        maven_group.run_command.starts_with(&format!("{maven} spring-boot:run")) ,
        "有 wrapper 时应执行 `{maven} spring-boot:run`,实际: {}",
        maven_group.run_command
    );
    // 生命周期命令共用同一前缀,不能出现裸 `mvn`
    assert!(!maven_group
        .more_actions
        .iter()
        .any(|a| a.command.starts_with("mvn ")));

    let gradle_group = groups
        .iter()
        .find(|g| g.tool == JavaBuildTool::Gradle)
        .unwrap();
    assert_eq!(gradle_group.run_command, format!("{gradle} :svc:bootRun"));

    let _ = fs::remove_dir_all(&dir);

    // 反向:两个 wrapper 都没有时,命令必须是不带前缀的系统命令。这是修复前的实际故障面——
    // 前缀按平台无条件加,导致 macOS/Linux 上执行不存在的 ./mvn 与 ./gradle。
    let bare = temp_project_dir("no-wrapper");
    fs::write(
        bare.join("pom.xml"),
        "<project><build><plugins><plugin><artifactId>spring-boot-maven-plugin</artifactId></plugin></plugins></build></project>",
    )
    .unwrap();
    fs::create_dir_all(bare.join("svc")).unwrap();
    fs::write(
        bare.join("svc/build.gradle"),
        "plugins { id 'org.springframework.boot' version '3.2.0' }",
    )
    .unwrap();
    let files: Vec<PathBuf> = ["pom.xml", "svc/build.gradle"]
        .iter()
        .map(|p| PathBuf::from(p.replace('/', std::path::MAIN_SEPARATOR_STR)))
        .collect();
    let groups = java_builds_from_files(&bare, &files);
    assert_eq!(groups.len(), 2);
    for group in &groups {
        assert!(
            !group.run_command.starts_with("./"),
            "无 wrapper 时不得加 ./ 前缀,实际: {}",
            group.run_command
        );
        assert!(
            group
                .more_actions
                .iter()
                .all(|action| !action.command.starts_with("./")),
            "无 wrapper 时生命周期命令也不得带 ./ 前缀: {:?}",
            group
                .more_actions
                .iter()
                .map(|action| action.command.as_str())
                .collect::<Vec<_>>()
        );
    }

    let _ = fs::remove_dir_all(&bare);
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

/// 手动验证:对三个安装源拉真实元数据(cargo test real_world -- --ignored --nocapture)
#[test]
#[ignore]
fn real_world_list_remote_jdks() {
    for vendor in [JdkVendor::Adoptium, JdkVendor::Zulu, JdkVendor::Openjdk] {
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
            JdkVendor::Openjdk => {
                for r in list_openjdk_releases().unwrap() {
                    println!("openjdk: java {} ({})", r.major, r.version);
                }
            }
        }
    }
}

#[test]
fn labels_openjdk_ri_file_names() {
    assert_eq!(
        openjdk_file_name(21, 35, "windows-x64", "zip"),
        "openjdk-21+35_windows-x64_bin.zip"
    );
    assert_eq!(
        openjdk_file_name(26, 35, "linux-x64", "tar.gz"),
        "openjdk-26+35_linux-x64_bin.tar.gz"
    );
}

#[test]
fn lists_openjdk_releases_newest_first() {
    // Windows/Linux x64 平台(本仓库开发/CI 环境)应返回常量表全量
    if cfg!(any(
        all(windows, target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64")
    )) {
        let releases = list_openjdk_releases().unwrap();
        assert!(!releases.is_empty());
        assert!(releases.windows(2).all(|w| w[0].major > w[1].major));
        assert!(releases
            .iter()
            .all(|r| r.version.starts_with(&format!("{}+", r.major))));
    }
}
