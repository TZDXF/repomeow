use std::path::Path;

use crate::commands::walk;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::{ComposeFile, ComposePort, ComposeService, ReadmeContent};

/// README 候选文件名,按优先级排列(大小写常见变体)
const README_CANDIDATES: &[&str] = &[
    "README.md",
    "readme.md",
    "README.MD",
    "Readme.md",
    "README.markdown",
    "README.txt",
    "README",
];

/// README 读取上限 512KB,避免超大文件拖垮前端渲染
const README_MAX_BYTES: u64 = 512 * 1024;

/// compose 文件大小上限 256KB,超过的直接跳过(正常 compose 文件远小于此)
const COMPOSE_MAX_BYTES: u64 = 256 * 1024;

pub(crate) fn ensure_dir(path: &str) -> AppResult<()> {
    if !Path::new(path).is_dir() {
        return Err(AppError::coded(ErrorCode::InvalidPath, path));
    }
    Ok(())
}

/// 在目录中按候选名查找文件,返回第一个存在的文件名。
/// 用 read_dir 做大小写精确匹配,避免 Windows/macOS 大小写不敏感文件系统
/// 把 readme.md 误判成 README.md,保证候选优先级在所有平台行为一致。
fn find_file(dir: &Path, candidates: &[&str]) -> Option<String> {
    let existing: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    candidates
        .iter()
        .find(|name| existing.iter().any(|f| f == *name))
        .map(|name| name.to_string())
}

/// 读取项目 README;不存在时返回 None
#[tauri::command]
pub fn read_readme(path: String) -> AppResult<Option<ReadmeContent>> {
    ensure_dir(&path)?;
    let dir = Path::new(&path);
    let Some(file_name) = find_file(dir, README_CANDIDATES) else {
        return Ok(None);
    };
    let file = dir.join(&file_name);
    // 超过上限只取前 README_MAX_BYTES 字节(按 UTF-8 边界截断)
    let meta = std::fs::metadata(&file)?;
    let content = if meta.len() > README_MAX_BYTES {
        let bytes = std::fs::read(&file)?;
        // 按 UTF-8 边界截断:跳过 continuation byte(0b10xxxxxx)
        let mut end = README_MAX_BYTES as usize;
        while end > 0 && (bytes[end] & 0xC0) == 0x80 {
            end -= 1;
        }
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    } else {
        std::fs::read_to_string(&file)?
    };
    Ok(Some(ReadmeContent { file_name, content }))
}

/// 写入文本到指定路径(供 Markdown 代码块/表格"下载"按钮走 Tauri save dialog 后调用)。
/// 内容上限为 512KB,目标路径不能为空且父目录必须存在。
/// 写入会创建或覆盖目标文件。
const SAVE_TEXT_MAX_BYTES: usize = 512 * 1024;

#[tauri::command]
pub fn save_text_file(path: String, content: String) -> AppResult<()> {
    if path.trim().is_empty() {
        return Err(AppError::coded(ErrorCode::SavePathRequired, ""));
    }
    if content.len() > SAVE_TEXT_MAX_BYTES {
        return Err(AppError::coded(
            ErrorCode::SaveContentTooLarge,
            SAVE_TEXT_MAX_BYTES.to_string(),
        ));
    }
    // 父目录必须存在
    let p = Path::new(&path);
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            return Err(AppError::coded(
                ErrorCode::SaveParentDirMissing,
                parent.display().to_string(),
            ));
        }
    }
    std::fs::write(p, content.as_bytes())?;
    Ok(())
}

/// 判断 YAML 内容是否为 Docker Compose 格式:顶层含 mapping 类型的 services。
/// 是则返回服务列表(含可访问端口);非法 YAML / 无 services(CI 配置等)返回 None。
fn parse_compose(content: &str) -> Option<Vec<ComposeService>> {
    let yaml = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(content).ok()?;
    let services = yaml.get("services")?.as_mapping()?;
    Some(
        services
            .iter()
            .filter_map(|(k, v)| {
                let mut ports = extract_ports(v);
                ports.sort_by_key(|p| (p.published, p.target));
                ports.dedup();
                Some(ComposeService {
                    name: k.as_str()?.to_string(),
                    ports,
                })
            })
            .collect(),
    )
}

/// 提取服务 ports 中可访问的宿主机端口映射:
/// 短语法 "8080:80" / "127.0.0.1:8080:80" / 长语法 { target, published } 取发布端口;
/// 仅容器端口(宿主机随机分配)、UDP、端口段范围无法确定入口,跳过。
fn extract_ports(service: &serde_yaml_ng::Value) -> Vec<ComposePort> {
    use serde_yaml_ng::Value;
    let Some(list) = service.get("ports").and_then(Value::as_sequence) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|item| match item {
            Value::String(s) => port_from_short(s),
            Value::Mapping(m) => port_from_long(m),
            // 纯数字仅声明容器端口,宿主机端口随机,不可直接访问
            _ => None,
        })
        .collect()
}

/// 短语法:"[IP:]发布端口:容器端口[/协议]"。发布端口恒为末段容器端口前的一段,
/// IPv6 带括号写法([::1]:8080:80)按 ':' 切分后该规律仍成立。
fn port_from_short(s: &str) -> Option<ComposePort> {
    let resolved = resolve_env(s)?;
    let (addr, proto) = resolved
        .split_once('/')
        .map_or((resolved.as_str(), "tcp"), |(a, p)| (a, p));
    if !proto.trim().eq_ignore_ascii_case("tcp") {
        return None; // UDP 等无法通过浏览器访问
    }
    let parts: Vec<&str> = addr.split(':').collect();
    if parts.len() < 2 {
        return None; // 仅容器端口,宿主机端口随机
    }
    Some(ComposePort {
        published: parse_port(parts[parts.len() - 2])?,
        target: parse_port(parts[parts.len() - 1])?,
    })
}

/// 长语法:{ target: 80, published: 8080, protocol: tcp }
fn port_from_long(m: &serde_yaml_ng::Mapping) -> Option<ComposePort> {
    use serde_yaml_ng::Value;
    let proto = m.get("protocol").and_then(Value::as_str).unwrap_or("tcp");
    if !proto.eq_ignore_ascii_case("tcp") {
        return None;
    }
    let parse_field = |key: &str| -> Option<u16> {
        match m.get(key)? {
            Value::Number(n) => n.as_u64().and_then(|v| u16::try_from(v).ok()),
            Value::String(s) => parse_port(&resolve_env(s)?),
            _ => None,
        }
    };
    Some(ComposePort {
        published: parse_field("published")?,
        target: parse_field("target")?,
    })
}

/// 替换 "${VAR:-default}" / "${VAR-default}" 为默认值;
/// 存在无默认值的变量时端口无法确定,整条映射返回 None。
/// 必须先于 ':' 切分执行——默认值语法自身含冒号,直接切分会把变量拆碎。
fn resolve_env(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let end = rest[start..].find('}')? + start;
        let inner = &rest[start + 2..end];
        let default = inner
            .split_once(":-")
            .or_else(|| inner.split_once('-'))
            .map(|(_, d)| d)?;
        out.push_str(default.trim());
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// 解析端口文本:纯数字直接取;端口段范围(8080-8081)无法确定,跳过。
fn parse_port(raw: &str) -> Option<u16> {
    let s = raw.trim();
    if s.contains('-') {
        return None;
    }
    s.parse().ok()
}

/// 是否为可能包含 compose 定义的 YAML 文件(按扩展名粗筛)
fn is_yaml_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("yml") || e.eq_ignore_ascii_case("yaml"))
}

/// 递归扫描项目内的 Docker Compose 文件(尊重 git 排除规则,按内容识别)。
/// 前端走合并扫描 scan_project_assets,此入口保留给测试
#[cfg_attr(not(test), allow(dead_code))]
pub fn scan_compose_files(path: String) -> AppResult<Vec<ComposeFile>> {
    ensure_dir(&path)?;
    let dir = Path::new(&path);
    Ok(compose_files_from_files(dir, &walk::project_files(dir)))
}

/// 在已遍历的文件清单上提取 compose 文件(供合并扫描复用,避免重复 walk)
pub(crate) fn compose_files_from_files(
    dir: &Path,
    walked: &[std::path::PathBuf],
) -> Vec<ComposeFile> {
    let mut files: Vec<ComposeFile> = walked
        .iter()
        .filter(|rel| is_yaml_file(rel))
        .filter(|rel| {
            std::fs::metadata(dir.join(rel))
                .map(|m| m.len() <= COMPOSE_MAX_BYTES)
                .unwrap_or(false)
        })
        .filter_map(|rel| {
            let content = std::fs::read_to_string(dir.join(rel)).ok()?;
            let services = parse_compose(&content)?;
            let file_name = rel.file_name().map(|n| n.to_string_lossy().into_owned())?;
            Some(ComposeFile {
                path: walk::to_slash(rel),
                file_name,
                services,
            })
        })
        .collect();
    // 根目录文件优先,同级按路径字典序
    files.sort_by(|a, b| (a.path.contains('/'), &a.path).cmp(&(b.path.contains('/'), &b.path)));
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// 建一个带唯一名字的临时目录,返回路径字符串
    fn temp_project_dir(tag: &str) -> String {
        let dir = std::env::temp_dir().join(format!(
            "repomeow-files-{tag}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().to_string()
    }

    #[test]
    fn readme_missing_and_found() {
        let dir = temp_project_dir("readme");
        let p = Path::new(&dir);

        assert!(read_readme(dir.clone()).unwrap().is_none());

        fs::write(p.join("readme.md"), "# Hello").unwrap();
        let r = read_readme(dir.clone()).unwrap().unwrap();
        assert_eq!(r.file_name, "readme.md");
        assert_eq!(r.content, "# Hello");

        // 优先级:README.md 高于 readme.md。
        // 注意先删 readme.md:Windows/macOS 大小写不敏感文件系统上,
        // 直接写 README.md 会覆盖同名文件但保留原有目录项大小写。
        fs::remove_file(p.join("readme.md")).unwrap();
        fs::write(p.join("README.md"), "# Priority").unwrap();
        let r = read_readme(dir.clone()).unwrap().unwrap();
        assert_eq!(r.file_name, "README.md");
        assert_eq!(r.content, "# Priority");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn readme_rejects_missing_dir() {
        assert!(matches!(read_readme("D:/no/such/dir-xyz".into()),
                Err(ref e) if e.is_code(crate::error::ErrorCode::InvalidPath)));
    }

    #[test]
    fn compose_scan_by_content_not_name() {
        let dir = temp_project_dir("compose-content");
        let p = Path::new(&dir);

        // 非标准文件名,但内容是 compose 格式 -> 识别
        fs::write(p.join("app.yml"), "services:\n  web:\n    image: nginx\n").unwrap();
        // 标准文件名但无 services -> 不识别
        fs::write(p.join("docker-compose.yml"), "name: demo\n").unwrap();
        // CI 配置(yml 但非 compose)-> 不识别
        fs::write(p.join("ci.yaml"), "on: push\njobs: {}\n").unwrap();
        // 非法 YAML -> 不识别
        fs::write(p.join("broken.yml"), "services: [not a map").unwrap();
        // 非 yml 文件不参与
        fs::write(p.join("services.txt"), "services:\n  x: {}\n").unwrap();

        let files = scan_compose_files(dir.clone()).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "app.yml");
        assert_eq!(files[0].file_name, "app.yml");
        assert_eq!(files[0].services[0].name, "web");
        assert!(files[0].services[0].ports.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compose_scan_nested_and_gitignored() {
        let dir = temp_project_dir("compose-nested");
        let p = Path::new(&dir);

        // 嵌套子目录中的 compose
        fs::create_dir_all(p.join("deploy/prod")).unwrap();
        fs::write(
            p.join("deploy/prod/stack.yaml"),
            "services:\n  api:\n    build: .\n  db:\n    image: postgres:16\n",
        )
        .unwrap();
        // 被 .gitignore 排除的目录不扫描
        fs::create_dir_all(p.join("ignored")).unwrap();
        fs::write(p.join("ignored/svc.yml"), "services:\n  x: {}\n").unwrap();
        fs::write(p.join(".gitignore"), "ignored/\n").unwrap();

        let files = scan_compose_files(dir.clone()).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "deploy/prod/stack.yaml");
        let names: Vec<&str> = files[0].services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["api", "db"]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compose_scan_root_first_ordering() {
        let dir = temp_project_dir("compose-order");
        let p = Path::new(&dir);

        fs::create_dir_all(p.join("abc")).unwrap();
        fs::write(p.join("abc/x.yml"), "services:\n  a: {}\n").unwrap();
        // 根目录文件名字典序更大,但仍应排在前面
        fs::write(p.join("z.yml"), "services:\n  z: {}\n").unwrap();
        fs::write(p.join("a.yml"), "services:\n  a: {}\n").unwrap();

        let files = scan_compose_files(dir.clone()).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["a.yml", "z.yml", "abc/x.yml"]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_text_file_writes_and_validates() {
        let dir = temp_project_dir("save-text");
        let p = Path::new(&dir);

        // 正常写入
        let target = p.join("out.csv");
        save_text_file(target.to_string_lossy().into_owned(), "a,b\n1,2\n".into()).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "a,b\n1,2\n");

        // 空路径报错
        assert!(matches!(
            save_text_file("".into(), "x".into()),
            Err(ref e) if e.is_code(ErrorCode::SavePathRequired)
        ));

        // 父目录不存在报错
        let bad = p.join("missing/out.txt");
        assert!(matches!(
            save_text_file(bad.to_string_lossy().into_owned(), "x".into()),
            Err(ref e) if e.is_code(ErrorCode::SaveParentDirMissing)
        ));

        // 超大内容报错
        let huge = "x".repeat(SAVE_TEXT_MAX_BYTES + 1);
        let target2 = p.join("huge.txt");
        assert!(matches!(
            save_text_file(target2.to_string_lossy().into_owned(), huge),
            Err(ref e) if e.is_code(ErrorCode::SaveContentTooLarge)
        ));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compose_extracts_accessible_ports() {
        let content = r#"
services:
  web:
    image: nginx
    ports:
      - "8080:80"
      - "127.0.0.1:9090:90/tcp"
      - "53:53/udp"
      - "3000"
      - "8081-8082:81-82"
      - target: 443
        published: 8443
        protocol: tcp
      - target: 541
        published: 541
        protocol: udp
  api:
    image: app
    ports:
      - "${API_PORT:-8000}:8000"
  db:
    image: postgres
"#;
        let services = parse_compose(content).unwrap();
        assert_eq!(services.len(), 3);
        // web: 8080/9090/8443 可访问;udp、仅容器端口、端口段范围跳过;去重升序
        let port = |published: u16, target: u16| ComposePort { published, target };
        assert_eq!(
            services[0].ports,
            vec![port(8080, 80), port(8443, 443), port(9090, 90)]
        );
        assert_eq!(services[1].ports, vec![port(8000, 8000)]);
        assert!(services[2].ports.is_empty());
    }
}
