use std::path::Path;

use crate::commands::{files, java, script, walk};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::ProjectAssets;

/// 进入详情页时的资产扫描:单次目录遍历同时产出 package scripts 与 compose 文件,
/// 替代原先 list_package_scripts / scan_compose_files 两条命令各扫一遍。
/// walk 结果带缓存(见 walk::project_files_cached:文件变更监听即时失效 + TTL 兜底),
/// 反复进出详情页、多个卡片同时挂载不会重复遍历目录。
/// 缓存未命中时是全量递归磁盘遍历,放入 spawn_blocking,避免同步命令阻塞主线程。
#[tauri::command]
pub async fn scan_project_assets(path: String) -> AppResult<ProjectAssets> {
    tokio::task::spawn_blocking(move || scan_assets(&path))
        .await
        .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))?
}

fn scan_assets(path: &str) -> AppResult<ProjectAssets> {
    files::ensure_dir(path)?;
    let dir = Path::new(path);
    let walked = walk::project_files_cached(dir);
    Ok(ProjectAssets {
        package_scripts: script::package_scripts_from_files(dir, &walked),
        compose_files: files::compose_files_from_files(dir, &walked),
        java_builds: java::java_builds_from_files(dir, &walked),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_project_dir(tag: &str) -> String {
        let dir = std::env::temp_dir().join(format!(
            "repomeow-scan-{tag}-{}-{}",
            std::process::id(),
            crate::time_util::now_ts_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().to_string()
    }

    #[test]
    fn scans_scripts_and_compose_in_one_pass() {
        let dir = temp_project_dir("assets");
        let p = Path::new(&dir);

        fs::write(
            p.join("package.json"),
            r#"{"name":"demo","scripts":{"dev":"vite"}}"#,
        )
        .unwrap();
        fs::create_dir_all(p.join("deploy")).unwrap();
        fs::write(
            p.join("deploy/app.yml"),
            "services:\n  web:\n    image: nginx\n",
        )
        .unwrap();

        let assets = scan_assets(&dir).unwrap();
        assert_eq!(assets.package_scripts.len(), 1);
        assert_eq!(assets.package_scripts[0].dir, ".");
        assert_eq!(assets.compose_files.len(), 1);
        assert_eq!(assets.compose_files[0].path, "deploy/app.yml");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_missing_dir() {
        assert!(matches!(
            scan_assets("D:/no/such/dir-xyz"),
            Err(ref e) if e.is_code(crate::error::ErrorCode::InvalidPath)
        ));
    }

    #[test]
    fn scans_java_builds_in_one_pass() {
        let dir = temp_project_dir("java");
        let p = Path::new(&dir);

        fs::write(
            p.join("pom.xml"),
            "<project><build><plugins><plugin><artifactId>spring-boot-maven-plugin</artifactId></plugin></plugins></build></project>",
        )
        .unwrap();
        // 普通 Java pom 无 spring-boot 运行插件 -> 不产出
        fs::create_dir_all(p.join("lib")).unwrap();
        fs::write(p.join("lib/pom.xml"), "<project/>").unwrap();

        let assets = scan_assets(&dir).unwrap();
        assert_eq!(assets.java_builds.len(), 1);
        assert_eq!(assets.java_builds[0].dir, ".");
        assert!(assets.java_builds[0]
            .run_command
            .contains("spring-boot:run"));

        let _ = fs::remove_dir_all(&dir);
    }

    /// 手动验证:对本机真实多模块 Spring Boot 工程跑完整扫描(cargo test real_world -- --ignored --nocapture)
    #[test]
    #[ignore]
    fn real_world_ruoyi_scan() {
        let assets = scan_assets(r"D:\code\ruoyi-vue-pro").unwrap();
        println!("package_scripts: {}", assets.package_scripts.len());
        for g in &assets.java_builds {
            println!(
                "java_build: dir={} tool={:?} cmd={}",
                g.dir, g.tool, g.run_command
            );
        }
    }
}
