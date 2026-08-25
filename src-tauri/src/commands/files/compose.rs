use std::path::Path;

use super::ensure_dir;
use crate::commands::walk;
use crate::error::AppResult;
use crate::models::{ComposeFile, ComposePort, ComposeService};

/// compose 文件大小上限 256KB,超过的直接跳过(正常 compose 文件远小于此)
const COMPOSE_MAX_BYTES: u64 = 256 * 1024;

/// 判断 YAML 内容是否为 Docker Compose 格式:顶层含 mapping 类型的 services。
/// 是则返回服务列表(含可访问端口);非法 YAML / 无 services(CI 配置等)返回 None。
pub(super) fn parse_compose(content: &str) -> Option<Vec<ComposeService>> {
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

/// compose 判定的廉价粗筛(在完整 YAML 解析之前):
/// - 文件名含 compose(docker-compose.yml / compose.yaml 等惯例命名)直接通过;
/// - 否则只认第 0 列的顶层 `services:` 键——compose 规范要求 services 位于顶层,
///   缩进的同名键是嵌套字段(如 CI 配置里的 services),不算;
/// - 引号包裹键名等罕见写法可能漏过,但只要文件名带 compose 仍会被识别。
pub(super) fn maybe_compose_file(file_name: &str, content: &str) -> bool {
    if file_name.to_ascii_lowercase().contains("compose") {
        return true;
    }
    content.lines().any(|line| {
        line.strip_prefix("services")
            .is_some_and(|rest| rest.trim_start().starts_with(':'))
    })
}

/// 递归扫描项目内的 Docker Compose 文件(尊重 git 排除规则,按内容识别)。
/// 前端走合并扫描 scan_project_assets,此入口保留给测试
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn scan_compose_files(path: String) -> AppResult<Vec<ComposeFile>> {
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
            let file_name = rel.file_name()?.to_string_lossy().into_owned();
            // 廉价粗筛:大项目里 yaml 文件可达上百个,逐个完整解析 YAML 开销可观,
            // 先按文件名/顶层 services 键过滤,只对疑似文件做完整解析
            if !maybe_compose_file(&file_name, &content) {
                return None;
            }
            let services = parse_compose(&content)?;
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
