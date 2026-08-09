use std::path::Path;
use std::process::Command;

use serde::Deserialize;

use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::ComposeServiceState;

fn docker_command() -> Command {
    let mut cmd = Command::new("docker");
    // Windows: 避免 GUI 应用拉起 docker 时闪现控制台黑窗
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// `docker compose ps --format json` 输出的单条记录(只取关心的字段)
#[derive(Debug, Deserialize)]
struct PsEntry {
    #[serde(rename = "Service")]
    service: Option<String>,
    #[serde(rename = "State")]
    state: Option<String>,
    #[serde(rename = "Status")]
    status: Option<String>,
}

/// 解析 ps 输出:compose v2.21+ 输出 JSON 数组,更早版本输出 NDJSON(每行一个对象)
fn parse_ps(output: &str) -> Vec<ComposeServiceState> {
    let entries: Vec<PsEntry> = serde_json::from_str(output.trim())
        .or_else(|_| {
            output
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(serde_json::from_str)
                .collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or_default();
    entries
        .into_iter()
        .filter_map(|e| {
            Some(ComposeServiceState {
                name: e.service?,
                running: e.state.as_deref() == Some("running"),
                status: e.status.unwrap_or_default(),
            })
        })
        .collect()
}

fn ps_blocking(path: &str, file: &str) -> AppResult<Vec<ComposeServiceState>> {
    let dir = Path::new(path);
    if !dir.is_dir() {
        return Err(AppError::coded(ErrorCode::DockerDirNotFound, path));
    }
    // 与前端 up/down 的执行方式保持一致:项目根目录 + 相对 -f 路径,
    // 这样 compose 项目名解析一致,ps 才能命中同一组容器。
    // docker 未安装 / 守护进程未运行 / 项目未启动:一律视为无运行中服务(不报错打扰)
    let output = docker_command()
        .args(["compose", "-f", file, "ps", "--format", "json"])
        .current_dir(dir)
        .output();
    match output {
        Ok(out) if out.status.success() => Ok(parse_ps(&String::from_utf8_lossy(&out.stdout))),
        _ => Ok(Vec::new()),
    }
}

/// 查询 compose 文件中各服务的运行状态(阻塞调用放入线程池,避免卡住 UI)
#[tauri::command]
pub async fn compose_ps(path: String, file: String) -> AppResult<Vec<ComposeServiceState>> {
    tokio::task::spawn_blocking(move || ps_blocking(&path, &file))
        .await
        .map_err(|e| AppError::coded(ErrorCode::DockerTaskFailed, e.to_string()))?
}

fn run_docker(dir: &Path, args: &[&str]) -> AppResult<std::process::Output> {
    docker_command()
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| AppError::coded(ErrorCode::DockerExecFailed, e.to_string()))
}

/// 校验 docker 子命令成功,失败时把 stderr 包成错误
fn ensure_ok(action: &str, out: std::process::Output) -> AppResult<std::process::Output> {
    if out.status.success() {
        Ok(out)
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let context = format!("action={action} stderr={stderr}");
        Err(AppError::coded(ErrorCode::DockerActionFailed, context))
    }
}

/// 服务的容器 id;容器未创建时返回 None
fn container_id(dir: &Path, file: &str, service: &str) -> AppResult<Option<String>> {
    let ps = ensure_ok(
        "查询容器",
        run_docker(dir, &["compose", "-f", file, "ps", "-q", service])?,
    )?;
    let id = String::from_utf8_lossy(&ps.stdout).trim().to_string();
    Ok((!id.is_empty()).then_some(id))
}

/// compose 配置中各服务的镜像名,按配置顺序返回 (service, image)。
/// 不需要容器存在;build 型服务由 compose 计算出默认镜像名
fn service_images(dir: &Path, file: &str) -> AppResult<Vec<(String, String)>> {
    let cfg = ensure_ok(
        "读取 compose 配置",
        run_docker(dir, &["compose", "-f", file, "config", "--format", "json"])?,
    )?;
    let v: serde_json::Value = serde_json::from_slice(&cfg.stdout)
        .map_err(|e| AppError::coded(ErrorCode::DockerComposeParseFailed, e.to_string()))?;
    let services = v
        .get("services")
        .and_then(|s| s.as_object())
        .cloned()
        .unwrap_or_default();
    Ok(services
        .into_iter()
        .filter_map(|(name, svc)| {
            let image = svc.get("image")?.as_str()?.trim().to_string();
            (!image.is_empty()).then_some((name, image))
        })
        .collect())
}

/// docker save 镜像到 dest;本地无此镜像时给出可操作的中文提示
fn save_image(dir: &Path, image: &str, dest: &Path) -> AppResult<()> {
    let out = run_docker(dir, &["save", "-o", &dest.to_string_lossy(), image])?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains("No such image") {
        return Err(AppError::coded(
            ErrorCode::DockerContainerNotCreated,
            format!("image={image}"),
        ));
    }
    Err(AppError::coded(ErrorCode::DockerSaveFailed, stderr.trim().to_string()))
}

/// 导出单个服务:container → docker export(需容器已创建);image → docker save(只需本地有镜像)
fn export_one(dir: &Path, file: &str, service: &str, kind: &str, dest: &Path) -> AppResult<()> {
    match kind {
        "container" => {
            let id = container_id(dir, file, service)?.ok_or_else(|| {
                AppError::coded(ErrorCode::DockerContainerNotCreated, service.to_string())
            })?;
            let out = run_docker(dir, &["export", "-o", &dest.to_string_lossy(), &id])?;
            ensure_ok("导出", out)?;
            Ok(())
        }
        "image" => {
            let image = service_images(dir, file)?
                .into_iter()
                .find(|(name, _)| name == service)
                .map(|(_, image)| image)
                .ok_or_else(|| {
                    AppError::coded(ErrorCode::DockerServiceImageMissing, service.to_string())
                })?;
            save_image(dir, &image, dest)
        }
        _ => Err(AppError::coded(ErrorCode::DockerUnknownExportKind, kind.to_string())),
    }
}

/// 导出 compose 文件全部服务到目录(dest 为目录):逐服务导出 `<service>-<kind>.tar`。
/// container:需容器已创建,未创建的跳过;image:只需本地有镜像,按名去重避免重复 save,
/// 本地缺失的镜像跳过。一个都没导出时才报错
fn export_all(dir: &Path, file: &str, kind: &str, dest_dir: &str) -> AppResult<()> {
    let dest_dir = Path::new(dest_dir);
    if !dest_dir.is_dir() {
        return Err(AppError::coded(
            ErrorCode::DockerDirNotFound,
            dest_dir.display().to_string(),
        ));
    }
    if kind == "image" {
        let mut exported = 0usize;
        let mut saved = std::collections::HashSet::new();
        for (service, image) in service_images(dir, file)? {
            if !saved.insert(image.clone()) {
                continue;
            }
            let dest = dest_dir.join(format!("{service}-image.tar"));
            // 本地未拉取/构建的镜像跳过,不打断整体导出
            if save_image(dir, &image, &dest).is_ok() {
                exported += 1;
            }
        }
        if exported == 0 {
            return Err(AppError::coded(ErrorCode::DockerNoExportableImages, ""));
        }
        return Ok(());
    }
    if kind != "container" {
        return Err(AppError::coded(ErrorCode::DockerUnknownExportKind, kind.to_string()));
    }
    let cfg = ensure_ok(
        "读取服务列表",
        run_docker(dir, &["compose", "-f", file, "config", "--services"])?,
    )?;
    let services: Vec<String> = String::from_utf8_lossy(&cfg.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let mut exported = 0usize;
    for service in &services {
        // 未创建容器的服务跳过,不打断整体导出
        let Some(id) = container_id(dir, file, service)? else {
            continue;
        };
        let dest = dest_dir.join(format!("{service}-container.tar"));
        let out = run_docker(dir, &["export", "-o", &dest.to_string_lossy(), &id])?;
        ensure_ok("导出", out)?;
        exported += 1;
    }
    if exported == 0 {
        return Err(AppError::coded(ErrorCode::DockerNoExportableContainers, ""));
    }
    Ok(())
}

/// 导出 compose 服务的容器文件系统 / 镜像为 tar 包
/// kind: "container" → docker export;"image" → docker save。
/// service 为空时导出该文件的全部服务,此时 dest 为目标目录
#[tauri::command]
pub async fn compose_export(
    path: String,
    file: String,
    service: String,
    kind: String,
    dest: String,
) -> AppResult<()> {
    tokio::task::spawn_blocking(move || {
        let dir = Path::new(&path);
        if !dir.is_dir() {
            return Err(AppError::coded(ErrorCode::DockerDirNotFound, path));
        }
        if service.is_empty() {
            export_all(dir, &file, &kind, &dest)
        } else {
            export_one(dir, &file, &service, &kind, Path::new(&dest))
        }
    })
    .await
    .map_err(|e| AppError::coded(ErrorCode::DockerTaskFailed, e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ndjson_output() {
        // 早期 compose:每行一个 JSON 对象
        let out = concat!(
            r#"{"Name":"app-web-1","Service":"web","State":"running","Status":"Up 2 hours"}"#,
            "\n",
            r#"{"Name":"app-db-1","Service":"db","State":"exited","Status":"Exited (0) 5 minutes ago"}"#,
            "\n"
        );
        let states = parse_ps(out);
        assert_eq!(states.len(), 2);
        assert_eq!(states[0].name, "web");
        assert!(states[0].running);
        assert_eq!(states[0].status, "Up 2 hours");
        assert_eq!(states[1].name, "db");
        assert!(!states[1].running);
    }

    #[test]
    fn parses_json_array_output() {
        // compose v2.21+:整体一个 JSON 数组
        let out = r#"[
          {"Name":"app-web-1","Service":"web","State":"running","Status":"Up 3 seconds"},
          {"Name":"app-api-1","Service":"api","State":"restarting","Status":"Restarting"}
        ]"#;
        let states = parse_ps(out);
        assert_eq!(states.len(), 2);
        assert!(states[0].running);
        assert!(!states[1].running);
    }

    #[test]
    fn empty_or_garbage_output_yields_no_states() {
        assert!(parse_ps("").is_empty());
        assert!(parse_ps("\n\n").is_empty());
        assert!(parse_ps("not json at all").is_empty());
    }
}
