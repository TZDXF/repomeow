use std::fs;
use std::io::Write;
use std::path::PathBuf;

use super::builds::java_builds_from_files;
use super::detection::{check_jdk, detect_jdks_blocking, parse_java_version};
use super::install::{extract_zip, move_dir};
use super::remote::{
    is_zulu_jdk_zip, list_adoptium_releases, list_zulu_releases, zulu_version_label,
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
