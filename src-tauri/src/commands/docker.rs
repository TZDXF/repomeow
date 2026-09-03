use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Deserialize;
use tauri::AppHandle;

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

/// tokio 版 docker 命令(异步 ps 用):kill_on_drop 保证超时/取消后子进程被回收,
/// 不会像 sync `Command::output()` 那样在 docker daemon 卡死时永久悬挂
fn docker_tokio_command() -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("docker");
    #[cfg(windows)]
    {
        // tokio::process::Command 自带 creation_flags(Windows)
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.kill_on_drop(true);
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

/// 单次 ps 的超时:daemon 卡死时 sync `Command::output()` 会无限期挂起,
/// 占用并发许可与线程;超时后子进程经 kill_on_drop 回收,该文件按无服务处理
const PS_TIMEOUT: Duration = Duration::from_secs(20);

/// 拒绝包含 `..` / 绝对路径 / 反斜杠转义的相对路径:
/// docker compose -f 接受任意字符串,但项目根外文件无意义且会泄露同机其他目录信息
fn is_safe_compose_rel_path(file: &str) -> bool {
    !file.is_empty()
        && !file.contains("..")
        && !file.starts_with('/')
        && !file.starts_with('\\')
        && !(file.len() >= 2 && file.as_bytes()[1] == b':')
}

/// 查询单个 compose 文件的服务状态。docker 未安装 / 守护进程未运行 /
/// 项目未启动 / 超时 / 路径非法:一律视为无运行中服务(返回空,不报错打扰)
async fn ps_async(path: &str, file: &str) -> Vec<ComposeServiceState> {
    let dir = Path::new(path);
    if !dir.is_dir() || !is_safe_compose_rel_path(file) {
        return Vec::new();
    }
    // 与前端 up/down 的执行方式保持一致:项目根目录 + 相对 -f 路径,
    // 这样 compose 项目名解析一致,ps 才能命中同一组容器。
    let output = docker_tokio_command()
        .args(["compose", "-f", file, "ps", "--format", "json"])
        .current_dir(dir)
        .output()
        .await;
    match output {
        Ok(out) if out.status.success() => parse_ps(&String::from_utf8_lossy(&out.stdout)),
        _ => Vec::new(),
    }
}

/// 批量查询多个 compose 文件的服务运行状态:一次 IPC 完成全部文件的 ps,
/// 避免前端逐文件发起请求造成大量 HTTP 往返;后端限并发并行拉起 docker 进程。
/// 单文件失败/超时/任务异常都降级为空列表,不影响其他文件。
/// 路径越界(file 含 `..` / 绝对路径)与项目根不是目录时整体按空列表处理,
/// 不让前端误传路径打穿到项目根之外读 docker 元数据
#[tauri::command]
pub async fn compose_ps_batch(
    path: String,
    files: Vec<String>,
) -> AppResult<Vec<Vec<ComposeServiceState>>> {
    let root = Path::new(&path);
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    // 最多同时 4 个 docker CLI 进程,防止文件多时瞬间拉起大量子进程争抢
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
    let mut handles = Vec::with_capacity(files.len());
    for file in files {
        let path = path.clone();
        let sem = sem.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await;
            match tokio::time::timeout(PS_TIMEOUT, ps_async(&path, &file)).await {
                Ok(states) => states,
                Err(_) => {
                    eprintln!("[docker] compose ps 超时({file}),按无运行中服务处理");
                    Vec::new()
                }
            }
        }));
    }
    let mut results = Vec::with_capacity(handles.len());
    for h in handles {
        // 单任务异常(panic/取消)只降级该文件,不中断整批
        results.push(h.await.unwrap_or_else(|e| {
            eprintln!("[docker] compose ps 任务异常: {e}");
            Vec::new()
        }));
    }
    Ok(results)
}

fn run_docker(dir: &Path, args: &[&str]) -> AppResult<std::process::Output> {
    docker_command()
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| AppError::coded(ErrorCode::DockerExecFailed, e.to_string()))
}

/// 校验 docker 子命令成功,失败时把 stderr 包成错误。
/// `action` 是已按界面语言翻译好的可读标签(由调用方在 `compose_export` 入口处
/// 一次解析 language 后传入);此处不再做语言分支,避免每个 ensure_ok 调用点都
/// 重复读 settings
fn ensure_ok(action: &str, out: std::process::Output) -> AppResult<std::process::Output> {
    if out.status.success() {
        Ok(out)
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let context = format!("action={action} stderr={stderr}");
        Err(AppError::coded(ErrorCode::DockerActionFailed, context))
    }
}

/// 按界面语言分支的 action 标签(供 ensure_ok 用);与 tray.rs load_tray_texts
/// 读取 language 的方式保持一致(settings.json 的 language 字段)
fn docker_action_label(zh: &'static str, en: &'static str, language: &str) -> &'static str {
    if language == "en-US" {
        en
    } else {
        zh
    }
}

/// compose 文件全部服务的列表(config --services,按配置顺序)
fn compose_services(dir: &Path, file: &str, language: &str) -> AppResult<Vec<String>> {
    let cfg = ensure_ok(
        docker_action_label("读取服务列表", "list services", language),
        run_docker(dir, &["compose", "-f", file, "config", "--services"])?,
    )?;
    Ok(String::from_utf8_lossy(&cfg.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// 本地是否已存在该镜像(docker image inspect);探测失败一律视为不存在
fn image_exists(dir: &Path, image: &str) -> bool {
    docker_command()
        .args(["image", "inspect", "-q", image])
        .current_dir(dir)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 合并导出容器时临时镜像名的合法化:docker 仓库名仅允许小写字母/数字/. _ -,
/// 其余字符(服务名理论上已合法,防御兜底)归一为 '-'
fn sanitize_repo_name(service: &str) -> String {
    service
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// 导出目标写入器:可选 gzip 压缩。
/// gzip 尾部只有显式 finish 才写出,所以封装成显式完成语义而不是靠 Drop 兜底
enum ExportWriter {
    Plain(BufWriter<File>),
    Gzip(flate2::write::GzEncoder<BufWriter<File>>),
}

impl Write for ExportWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            ExportWriter::Plain(w) => w.write(buf),
            ExportWriter::Gzip(w) => w.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            ExportWriter::Plain(w) => w.flush(),
            ExportWriter::Gzip(w) => w.flush(),
        }
    }
}

impl ExportWriter {
    fn create(dest: &Path, compress: bool) -> AppResult<ExportWriter> {
        let file = File::create(dest)
            .map_err(|e| AppError::coded(ErrorCode::DockerSaveFailed, e.to_string()))?;
        let buf = BufWriter::new(file);
        Ok(if compress {
            ExportWriter::Gzip(flate2::write::GzEncoder::new(
                buf,
                flate2::Compression::default(),
            ))
        } else {
            ExportWriter::Plain(buf)
        })
    }

    fn finish(self) -> std::io::Result<()> {
        match self {
            ExportWriter::Plain(mut w) => w.flush(),
            ExportWriter::Gzip(w) => w.finish().map(|_| ()),
        }
    }
}

/// 运行 docker 命令并把 stdout 流式写入 dest(可选 gzip 压缩)。
/// stderr 独立线程采集,避免管道写满与 stdout 读取相互阻塞死锁;
/// 任何失败(进程非零退出/写盘失败)都删除残缺输出,避免半截 tar 被误认为导出成功
fn run_docker_streaming(
    dir: &Path,
    args: &[&str],
    dest: &Path,
    compress: bool,
    action: &str,
) -> AppResult<()> {
    let mut writer = ExportWriter::create(dest, compress)?;
    let mut child = docker_command()
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AppError::coded(ErrorCode::DockerExecFailed, e.to_string()))?;
    let mut stdout = child.stdout.take().expect("stdout 已管道化");
    let mut stderr = child.stderr.take().expect("stderr 已管道化");
    let err_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).trim().to_string()
    });
    let copy_result = std::io::copy(&mut stdout, &mut writer);
    if copy_result.is_err() {
        // 写盘失败时 docker 可能仍在向无人读取的 stdout 管道写,wait 会死等;先杀进程再回收
        let _ = child.kill();
    }
    let wait_result = child.wait();
    let stderr = err_thread.join().unwrap_or_default();
    if let Err(e) = copy_result {
        let _ = std::fs::remove_file(dest);
        return Err(AppError::coded(ErrorCode::DockerSaveFailed, e.to_string()));
    }
    let status = wait_result.map_err(|e| AppError::coded(ErrorCode::DockerExecFailed, e.to_string()))?;
    if !status.success() {
        let _ = std::fs::remove_file(dest);
        return Err(AppError::coded(
            ErrorCode::DockerActionFailed,
            format!("action={action} stderr={stderr}"),
        ));
    }
    if let Err(e) = writer.finish() {
        let _ = std::fs::remove_file(dest);
        return Err(AppError::coded(ErrorCode::DockerSaveFailed, e.to_string()));
    }
    Ok(())
}

/// docker save 一个或多个镜像到 dest(可选 gzip 压缩)。
/// 单镜像时把「本地无此镜像」映射为可操作的错误提示
fn save_images(
    dir: &Path,
    images: &[String],
    dest: &Path,
    compress: bool,
    language: &str,
) -> AppResult<()> {
    let mut args: Vec<&str> = vec!["save"];
    args.extend(images.iter().map(String::as_str));
    let action = docker_action_label("保存镜像", "save images", language);
    match run_docker_streaming(dir, &args, dest, compress, action) {
        Err(AppError::Coded { message, .. })
            if images.len() == 1 && message.contains("No such image") =>
        {
            Err(AppError::coded(
                ErrorCode::DockerContainerNotCreated,
                format!("image={}", images[0]),
            ))
        }
        other => other,
    }
}

/// 服务的容器 id;容器未创建时返回 None
fn container_id(
    dir: &Path,
    file: &str,
    service: &str,
    language: &str,
) -> AppResult<Option<String>> {
    let ps = ensure_ok(
        docker_action_label("查询容器", "list containers", language),
        run_docker(dir, &["compose", "-f", file, "ps", "-q", service])?,
    )?;
    let id = String::from_utf8_lossy(&ps.stdout).trim().to_string();
    Ok((!id.is_empty()).then_some(id))
}

/// compose 配置中各服务的镜像名,按配置顺序返回 (service, image)。
/// 不需要容器存在;build 型服务由 compose 计算出默认镜像名
fn service_images(dir: &Path, file: &str, language: &str) -> AppResult<Vec<(String, String)>> {
    let cfg = ensure_ok(
        docker_action_label("读取 compose 配置", "read compose config", language),
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

/// 导出单个服务:container → docker export(需容器已创建);image → docker save(只需本地有镜像)
fn export_one(
    dir: &Path,
    file: &str,
    service: &str,
    kind: &str,
    dest: &Path,
    compress: bool,
    language: &str,
) -> AppResult<()> {
    match kind {
        "container" => {
            let id = container_id(dir, file, service, language)?.ok_or_else(|| {
                AppError::coded(ErrorCode::DockerContainerNotCreated, service.to_string())
            })?;
            run_docker_streaming(
                dir,
                &["export", &id],
                dest,
                compress,
                docker_action_label("导出", "export", language),
            )
        }
        "image" => {
            let image = service_images(dir, file, language)?
                .into_iter()
                .find(|(name, _)| name == service)
                .map(|(_, image)| image)
                .ok_or_else(|| {
                    AppError::coded(ErrorCode::DockerServiceImageMissing, service.to_string())
                })?;
            save_images(dir, &[image], dest, compress, language)
        }
        _ => Err(AppError::coded(
            ErrorCode::DockerUnknownExportKind,
            kind.to_string(),
        )),
    }
}

/// 导出 compose 文件全部服务。
/// 不合并:dest 为目录,逐服务导出 `<service>-<kind>.tar(.gz)`;
/// 合并:dest 为单文件——镜像一次 docker save 全部打包,容器先逐个 docker commit
/// 为临时镜像再 docker save,产物均可 docker load 整体恢复为镜像。
/// container:需容器已创建,未创建的跳过;image:只需本地有镜像,按名去重,
/// 本地缺失的镜像跳过。一个都没导出时才报错
fn export_all(
    dir: &Path,
    file: &str,
    kind: &str,
    dest: &str,
    merge: bool,
    compress: bool,
    language: &str,
) -> AppResult<()> {
    match kind {
        "image" => export_all_images(dir, file, dest, merge, compress, language),
        "container" => export_all_containers(dir, file, dest, merge, compress, language),
        _ => Err(AppError::coded(
            ErrorCode::DockerUnknownExportKind,
            kind.to_string(),
        )),
    }
}

fn export_all_images(
    dir: &Path,
    file: &str,
    dest: &str,
    merge: bool,
    compress: bool,
    language: &str,
) -> AppResult<()> {
    // 按镜像名去重:多服务共用同一镜像时只导一次(单文件导出沿用首个服务名)
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (service, image) in service_images(dir, file, language)? {
        if seen.insert(image.clone()) {
            entries.push((service, image));
        }
    }
    if merge {
        // dest 为单个文件:本地存在的镜像一次 docker save 合并导出(共享层自动去重,docker load 可整体恢复)
        let images: Vec<String> = entries
            .into_iter()
            .filter(|(_, image)| image_exists(dir, image))
            .map(|(_, image)| image)
            .collect();
        if images.is_empty() {
            return Err(AppError::coded(ErrorCode::DockerNoExportableImages, ""));
        }
        return save_images(dir, &images, Path::new(dest), compress, language);
    }
    let dest_dir = Path::new(dest);
    if !dest_dir.is_dir() {
        return Err(AppError::coded(
            ErrorCode::DockerDirNotFound,
            dest_dir.display().to_string(),
        ));
    }
    let ext = if compress { "tar.gz" } else { "tar" };
    let mut exported = 0usize;
    for (service, image) in entries {
        // 本地未拉取/构建的镜像跳过,不打断整体导出
        if !image_exists(dir, &image) {
            continue;
        }
        let dest_file = dest_dir.join(format!("{service}-image.{ext}"));
        save_images(dir, &[image], &dest_file, compress, language)?;
        exported += 1;
    }
    if exported == 0 {
        return Err(AppError::coded(ErrorCode::DockerNoExportableImages, ""));
    }
    Ok(())
}

fn export_all_containers(
    dir: &Path,
    file: &str,
    dest: &str,
    merge: bool,
    compress: bool,
    language: &str,
) -> AppResult<()> {
    let services = compose_services(dir, file, language)?;
    // 未创建容器的服务跳过,不打断整体导出
    let mut containers: Vec<(String, String)> = Vec::new(); // (service, container id)
    for service in services {
        let Some(id) = container_id(dir, file, &service, language)? else {
            continue;
        };
        containers.push((service, id));
    }
    if containers.is_empty() {
        return Err(AppError::coded(ErrorCode::DockerNoExportableContainers, ""));
    }
    if merge {
        return export_containers_merged(dir, &containers, Path::new(dest), compress, language);
    }
    let dest_dir = Path::new(dest);
    if !dest_dir.is_dir() {
        return Err(AppError::coded(
            ErrorCode::DockerDirNotFound,
            dest_dir.display().to_string(),
        ));
    }
    let ext = if compress { "tar.gz" } else { "tar" };
    for (service, id) in containers {
        let dest_file = dest_dir.join(format!("{service}-container.{ext}"));
        run_docker_streaming(
            dir,
            &["export", &id],
            &dest_file,
            compress,
            docker_action_label("导出", "export", language),
        )?;
    }
    Ok(())
}

/// 合并导出多个容器为单文件:docker export 没有「多容器合并」语义,先把每个容器 docker commit 为临时镜像,
/// 再一次 docker save 成单 tar(共享层自动去重,产物可 docker load 整体恢复为镜像);
/// 结束后 rmi 清理临时镜像(失败仅记日志,不阻断导出结果)
fn export_containers_merged(
    dir: &Path,
    containers: &[(String, String)],
    dest: &Path,
    compress: bool,
    language: &str,
) -> AppResult<()> {
    let ts = crate::time_util::now_ts();
    let mut images: Vec<String> = Vec::with_capacity(containers.len());
    for (service, id) in containers {
        let image = format!("repomeow-export-{}:{}", sanitize_repo_name(service), ts);
        let committed = run_docker(dir, &["commit", "--pause=false", id.as_str(), &image]).and_then(|out| {
            ensure_ok(
                docker_action_label("提交临时镜像", "commit temporary image", language),
                out,
            )
        });
        if let Err(e) = committed {
            // commit 失败时清掉已提交的临时镜像,不残留半成品
            for committed_image in &images {
                let _ = run_docker(dir, &["rmi", committed_image.as_str()]);
            }
            return Err(e);
        }
        images.push(image);
    }
    let result = save_images(dir, &images, dest, compress, language);
    // 无论导出成功失败都清理临时镜像(失败仅记日志,不阻断导出结果)
    for image in &images {
        if let Err(e) = run_docker(dir, &["rmi", image.as_str()]) {
            eprintln!("[docker] 清理导出临时镜像失败({image}): {e}");
        }
    }
    result
}

/// 导出 compose 服务的容器文件系统 / 镜像为 tar 包
/// kind: "container" → docker export;"image" → docker save。
/// service 为空时导出该文件的全部服务:merge 为 true 时合并为单个文件(dest 为文件路径),否则 dest 为目标目录;
/// compress 为 true 时产物 gzip 压缩(单服务导出无合并语义,merge 忽略)
#[tauri::command]
pub async fn compose_export(
    app: AppHandle,
    path: String,
    file: String,
    service: String,
    kind: String,
    dest: String,
    merge: bool,
    compress: bool,
) -> AppResult<()> {
    // 一次性读界面语言并向下传递(避免 export_one / export_all 内每条
    // ensure_ok 都重复读 settings.json)
    let language = crate::tray::read_setting_string(&app, "language").unwrap_or_default();
    tokio::task::spawn_blocking(move || {
        let dir = Path::new(&path);
        if !dir.is_dir() {
            return Err(AppError::coded(ErrorCode::DockerDirNotFound, path));
        }
        if service.is_empty() {
            export_all(dir, &file, &kind, &dest, merge, compress, &language)
        } else {
            // 单服务导出没有「合并」语义,merge 仅批量导出使用,此处忽略
            export_one(dir, &file, &service, &kind, Path::new(&dest), compress, &language)
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
