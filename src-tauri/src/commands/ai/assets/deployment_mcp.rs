//! MCP 方言在后端统一转换,前端只传资源 ID,不接收任意配置/目标路径。
use super::super::resource_library::{McpServer, McpServerInput, RlResult};
use super::{
    deployment_io::{problem, safe_path},
    mcp_formats::{json_to_toml_item, parse_jsonc},
    McpTarget,
};
use serde_json::{json, Value};
use serde_yaml_ng::{
    value::{Tag, TaggedValue},
    Value as YamlValue,
};
use std::{collections::HashMap, fs, path::Path};
use toml_edit::{DocumentMut, Item, Table};

// Pi 使用服务器名构造工具名,不接受空格、点号或非 ASCII 字符。
fn validate_pi_name(name: &str) -> RlResult<()> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return Err(problem(format!("Pi: invalid MCP server name: {name}")));
    }
    Ok(())
}

/// DSH 将 serverName 用于工具命名,官方约束为 `[A-Za-z0-9_-]{1,32}`。
fn validate_dsh_name(name: &str) -> RlResult<()> {
    if name.is_empty()
        || name.len() > 32
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return Err(problem(format!(
            "DeepSeek Harness: invalid MCP server name: {name}"
        )));
    }
    Ok(())
}

fn dsh_env_access(name: &str) -> String {
    if name
        .bytes()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
    {
        format!("process.env.{name}")
    } else {
        format!("process.env[{}]", json!(name).to_string())
    }
}

fn dsh_push_template_text(out: &mut String, text: &str) {
    let encoded = json!(text).to_string();
    let encoded = &encoded[1..encoded.len() - 1];
    out.push_str(&encoded.replace('`', "\\`").replace("${", "\\${"));
}

/// 资源库跨 harness 的 `${VAR}` 语义映射为 DSH 官方 JS 表达式。
/// 只生成表达式,不读取或固化当前环境变量值。
fn dsh_env_expression(input: &str) -> Option<String> {
    let mut cursor = 0;
    let mut search = 0;
    let mut found = false;
    let mut template = String::from("`");
    while let Some(relative) = input[search..].find("${") {
        let start = search + relative;
        let Some(relative_end) = input[start + 2..].find('}') else {
            break;
        };
        let end = start + 2 + relative_end;
        let name = &input[start + 2..end];
        if name.is_empty() || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
            search = start + 2;
            continue;
        }
        if !found && start == 0 && end + 1 == input.len() {
            return Some(dsh_env_access(name));
        }
        dsh_push_template_text(&mut template, &input[cursor..start]);
        template.push_str("${");
        template.push_str(&dsh_env_access(name));
        template.push('}');
        found = true;
        cursor = end + 1;
        search = cursor;
    }
    if !found {
        return None;
    }
    dsh_push_template_text(&mut template, &input[cursor..]);
    template.push('`');
    Some(template)
}

fn dsh_parse_env_access(expression: &str) -> Option<String> {
    if let Some(name) = expression.strip_prefix("process.env.") {
        return (name
            .bytes()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
            && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'))
        .then(|| name.to_string());
    }
    let name = expression.strip_prefix("process.env[")?.strip_suffix(']')?;
    let name = serde_json::from_str::<String>(name).ok()?;
    (!name.is_empty() && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'))
        .then_some(name)
}

/// 将本模块生成的表达式及官方文档常用形式还原为资源库占位符。
fn dsh_env_placeholder(expression: &str) -> Option<String> {
    let expression = expression.trim();
    if let Some(name) = dsh_parse_env_access(expression) {
        return Some(format!("${{{name}}}"));
    }
    let template = expression.strip_prefix('`')?.strip_suffix('`')?;
    let mut result = String::new();
    let mut literal = String::new();
    let mut cursor = 0;
    while cursor < template.len() {
        let ch = template[cursor..].chars().next()?;
        if ch == '\\' {
            let next = template[cursor + 1..].chars().next()?;
            if next == '`' || (next == '$' && template[cursor + 2..].starts_with('{')) {
                literal.push(next);
            } else {
                literal.push('\\');
                literal.push(next);
            }
            cursor += 1 + next.len_utf8();
            continue;
        }
        if template[cursor..].starts_with("${") {
            let end = cursor + 2 + template[cursor + 2..].find('}')?;
            let name = dsh_parse_env_access(&template[cursor + 2..end])?;
            let decoded = serde_json::from_str::<String>(&format!("\"{literal}\"")).ok()?;
            result.push_str(&decoded);
            result.push_str(&format!("${{{name}}}"));
            literal.clear();
            cursor = end + 1;
            continue;
        }
        match ch {
            '"' => literal.push_str("\\\""),
            '\n' => literal.push_str("\\n"),
            '\r' => literal.push_str("\\r"),
            _ => literal.push(ch),
        }
        cursor += ch.len_utf8();
    }
    let decoded = serde_json::from_str::<String>(&format!("\"{literal}\"")).ok()?;
    result.push_str(&decoded);
    Some(result)
}

fn dsh_definition(server: &McpServer) -> RlResult<Value> {
    validate_dsh_name(&server.name)?;
    let mut value = json!({
        "serverName": server.name,
        "transport": if server.transport == "stdio" { "stdio" } else { "streamable-http" },
    });
    if server.transport == "stdio" {
        let command = server
            .command
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| problem("MCP command missing"))?;
        value["command"] = json!(command);
        value["args"] = json!(server.args);
        if !server.env.is_empty() {
            value["env"] = json!(server.env);
        }
    } else {
        if server.transport != "http" {
            return Err(problem("DeepSeek Harness: SSE unsupported"));
        }
        let url = server
            .url
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| problem("MCP URL missing"))?;
        value["url"] = json!(url);
        if !server.headers.is_empty() {
            value["headers"] = json!(server.headers);
        }
    }
    Ok(value)
}

pub(super) fn definition(server: &McpServer, target: &McpTarget) -> RlResult<Value> {
    if target.agents.contains(&"pi") {
        validate_pi_name(&server.name)?;
    }
    if !server.enabled {
        return Err(problem(format!("disabled MCP: {}", server.name)));
    }
    if target.dialect == "dsh" {
        return dsh_definition(server);
    }
    if server.transport == "sse"
        && (target.dialect == "codex"
            || target.dialect == "opencode"
            || target.agents.contains(&"copilot")
            || target.agents.contains(&"pi"))
    {
        return Err(problem(format!("{}: SSE unsupported", target.path)));
    }
    let mut value = json!({});
    if server.transport == "stdio" {
        let command = server
            .command
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| problem("MCP command missing"))?;
        if target.dialect == "opencode" {
            let mut args = vec![command.to_string()];
            args.extend(server.args.clone());
            value["type"] = json!("local");
            value["command"] = json!(args);
            if !server.env.is_empty() {
                value["environment"] = json!(server.env);
            }
        } else {
            if target.dialect == "claude" && !target.agents.contains(&"kimi") {
                value["type"] = json!("stdio");
            }
            value["command"] = json!(command);
            value["args"] = json!(server.args);
            if !server.env.is_empty() {
                value["env"] = json!(server.env);
            }
        }
    } else {
        if !matches!(server.transport.as_str(), "http" | "sse") {
            return Err(problem("unsupported MCP transport"));
        }
        let url = server
            .url
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| problem("MCP URL missing"))?;
        match target.dialect {
            "gemini" => {
                value[if server.transport == "http" {
                    "httpUrl"
                } else {
                    "url"
                }] = json!(url);
            }
            "opencode" => {
                value["type"] = json!("remote");
                value["url"] = json!(url);
            }
            "codex" => {
                value["url"] = json!(url);
            }
            _ => {
                value[if target.agents.contains(&"kimi") {
                    "transport"
                } else {
                    "type"
                }] = json!(server.transport);
                value["url"] = json!(url);
            }
        }
        if !server.headers.is_empty() {
            value[if target.dialect == "codex" {
                "http_headers"
            } else {
                "headers"
            }] = json!(server.headers);
        }
    }
    Ok(value)
}

/// definition 的逆向:把项目文件里的服务器定义尽力映射回资源库输入,
/// 无法表达的额外字段(超时、cwd 等)丢弃;认领先看指纹,差异走「可更新」。
pub(super) fn parse_server_value(
    target: &McpTarget,
    name: &str,
    value: &Value,
) -> RlResult<McpServerInput> {
    fn str_list(value: Option<&Value>) -> Vec<String> {
        value
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }
    fn str_map(value: Option<&Value>) -> HashMap<String, String> {
        value
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    }
    let base = |transport: &str| McpServerInput {
        name: name.to_string(),
        description: None,
        transport: transport.to_string(),
        command: None,
        args: vec![],
        env: HashMap::new(),
        url: None,
        headers: HashMap::new(),
        enabled: true,
    };
    let obj = value.as_object().ok_or_else(|| problem(name))?;
    if target.dialect == "dsh" {
        validate_dsh_name(name)?;
        if obj.get("serverName").and_then(Value::as_str) != Some(name) {
            return Err(problem("DeepSeek Harness: serverName mismatch"));
        }
        return match obj.get("transport").and_then(Value::as_str) {
            Some("stdio") => {
                let command = obj
                    .get("command")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| problem(name))?;
                Ok(McpServerInput {
                    command: Some(command.to_string()),
                    args: str_list(obj.get("args")),
                    env: str_map(obj.get("env")),
                    ..base("stdio")
                })
            }
            Some("streamable-http") => {
                let url = obj
                    .get("url")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| problem(name))?;
                Ok(McpServerInput {
                    url: Some(url.to_string()),
                    headers: str_map(obj.get("headers")),
                    ..base("http")
                })
            }
            _ => Err(problem("DeepSeek Harness: unsupported MCP transport")),
        };
    }
    if target.agents.contains(&"pi") {
        validate_pi_name(name)?;
        if let Some(kind) = obj.get("type") {
            match kind.as_str() {
                Some("stdio") if obj.contains_key("command") => {}
                Some("http" | "streamable-http")
                    if !obj.contains_key("command") && obj.contains_key("url") => {}
                _ => return Err(problem("Pi: unsupported MCP type or mismatched transport")),
            }
        }
    }
    if target.dialect == "opencode" {
        return match obj.get("type").and_then(Value::as_str) {
            Some("local") => {
                let mut command = str_list(obj.get("command"));
                if command.is_empty() {
                    return Err(problem(name));
                }
                let args = command.split_off(1);
                Ok(McpServerInput {
                    command: Some(command.remove(0)),
                    args,
                    env: str_map(obj.get("environment")),
                    ..base("stdio")
                })
            }
            Some("remote") => {
                let url = obj
                    .get("url")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| problem(name))?;
                Ok(McpServerInput {
                    url: Some(url.to_string()),
                    headers: str_map(obj.get("headers")),
                    ..base("http")
                })
            }
            _ => Err(problem(name)),
        };
    }
    if let Some(command) = obj
        .get("command")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    {
        return Ok(McpServerInput {
            command: Some(command.to_string()),
            args: str_list(obj.get("args")),
            env: str_map(obj.get("env")),
            ..base("stdio")
        });
    }
    let (url, transport) = match target.dialect {
        "gemini" => match obj.get("httpUrl").and_then(Value::as_str) {
            Some(url) => (Some(url), "http"),
            None => (obj.get("url").and_then(Value::as_str), "sse"),
        },
        // codex 项目配置仅支持 streamable http
        "codex" => (obj.get("url").and_then(Value::as_str), "http"),
        _ => (
            obj.get("url").and_then(Value::as_str),
            if obj
                .get(if target.agents.contains(&"kimi") {
                    "transport"
                } else {
                    "type"
                })
                .and_then(Value::as_str)
                == Some("sse")
            {
                "sse"
            } else {
                "http"
            },
        ),
    };
    let url = url
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| problem(name))?;
    let headers_key = if target.dialect == "codex" {
        "http_headers"
    } else {
        "headers"
    };
    Ok(McpServerInput {
        url: Some(url.to_string()),
        headers: str_map(obj.get(headers_key)),
        ..base(transport)
    })
}

/// serde_yaml_ng 会把 YAML 核心 schema 的 `!!js` 解析成普通字符串。
/// 在解析/序列化边界改用仅供内部识别的本地标签,避免编辑无关条目时丢失表达式。
fn replace_dsh_yaml_tag(source: &str, from: &str, to: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut quote = None;
    let mut block_indent = None;
    for line in source.split_inclusive('\n') {
        let indent = line.bytes().take_while(|c| *c == b' ').count();
        if block_indent.is_some_and(|parent| line.trim().is_empty() || indent > parent) {
            output.push_str(line);
            continue;
        }
        block_indent = None;
        let mut cursor = 0;
        let mut block_header = false;
        while cursor < line.len() {
            let ch = line[cursor..].chars().next().unwrap();
            if let Some(active) = quote {
                output.push(ch);
                cursor += ch.len_utf8();
                if ch == '\\' && active == '"' && cursor < line.len() {
                    let next = line[cursor..].chars().next().unwrap();
                    output.push(next);
                    cursor += next.len_utf8();
                } else if ch == active {
                    if active == '\'' && line[cursor..].starts_with('\'') {
                        output.push('\'');
                        cursor += 1;
                    } else {
                        quote = None;
                    }
                }
                continue;
            }
            if ch == '#' && (cursor == 0 || line[..cursor].ends_with(char::is_whitespace)) {
                output.push_str(&line[cursor..]);
                break;
            }
            if ch == '\'' || ch == '"' {
                quote = Some(ch);
                output.push(ch);
                cursor += ch.len_utf8();
                continue;
            }
            let previous = line[..cursor].trim_end().chars().last();
            let tag_position =
                previous.is_none_or(|c| matches!(c, ':' | '-' | '?' | '[' | '{' | ','));
            if tag_position
                && line[cursor..].starts_with(from)
                && line[cursor + from.len()..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace)
            {
                output.push_str(to);
                cursor += from.len();
                continue;
            }
            let prefix = line[..cursor].trim_end();
            let tagged_block = prefix
                .split_whitespace()
                .last()
                .is_some_and(|token| token.starts_with('!'));
            if matches!(ch, '|' | '>') && (tag_position || tagged_block) {
                let suffix = line[cursor + ch.len_utf8()..].trim();
                let suffix = suffix.split('#').next().unwrap_or("").trim();
                if suffix.chars().all(|c| matches!(c, '+' | '-' | '0'..='9')) {
                    block_header = true;
                }
            }
            output.push(ch);
            cursor += ch.len_utf8();
        }
        if block_header {
            block_indent = Some(indent);
        }
    }
    output
}

struct DshYamlDocument {
    value: YamlValue,
    js_tag: String,
}

impl DshYamlDocument {
    fn parse(text: &str) -> RlResult<Self> {
        let mut suffix = 0;
        let js_tag = loop {
            let candidate = if suffix == 0 {
                "!repomeow-js".to_string()
            } else {
                format!("!repomeow-js-{suffix}")
            };
            if !text.contains(&candidate) {
                break candidate;
            }
            suffix += 1;
        };
        let normalized = replace_dsh_yaml_tag(text, "!!js", &js_tag);
        let value = if normalized.trim().is_empty() {
            YamlValue::Sequence(Vec::new())
        } else {
            serde_yaml_ng::from_str::<YamlValue>(&normalized).map_err(|e| problem(e.to_string()))?
        };
        if !value.is_sequence() {
            return Err(problem("DeepSeek Harness: patch must be a sequence"));
        }
        Ok(Self { value, js_tag })
    }

    fn to_string(&self) -> RlResult<Vec<u8>> {
        let text = serde_yaml_ng::to_string(&self.value).map_err(|e| problem(e.to_string()))?;
        Ok(replace_dsh_yaml_tag(&text, &self.js_tag, "!!js").into_bytes())
    }
}

fn dsh_yaml_definition(value: &Value, js_tag: &str) -> RlResult<YamlValue> {
    let mut yaml = serde_yaml_ng::to_value(value).map_err(|e| problem(e.to_string()))?;
    for section in ["env", "headers"] {
        if let Some(values) = yaml.get_mut(section).and_then(YamlValue::as_mapping_mut) {
            for (_, value) in values.iter_mut() {
                if let Some(expression) = value.as_str().and_then(dsh_env_expression) {
                    *value = YamlValue::Tagged(Box::new(TaggedValue {
                        tag: Tag::new(js_tag),
                        value: YamlValue::String(expression),
                    }));
                }
            }
        }
    }
    Ok(yaml)
}

fn dsh_config_json(config: &YamlValue, js_tag: &str) -> RlResult<Value> {
    let mut config = config.clone();
    for section in ["env", "headers"] {
        if let Some(values) = config.get_mut(section).and_then(YamlValue::as_mapping_mut) {
            for (_, value) in values.iter_mut() {
                if let YamlValue::Tagged(tagged) = value {
                    if tagged.tag == js_tag {
                        if let Some(placeholder) =
                            tagged.value.as_str().and_then(dsh_env_placeholder)
                        {
                            *value = YamlValue::String(placeholder);
                        }
                    }
                }
            }
        }
    }
    serde_json::to_value(config).map_err(|e| problem(e.to_string()))
}

fn dsh_plugin_config(entry: &YamlValue) -> Option<&YamlValue> {
    (entry.get("name")?.as_str()? == "@deepseek-ai/dsh-mcp-client")
        .then(|| entry.get("config"))
        .flatten()
}

fn dsh_server_name(config: &YamlValue) -> Option<&str> {
    config.get("serverName")?.as_str()
}

fn dsh_plugins(doc: &YamlValue) -> Vec<&YamlValue> {
    let mut plugins = Vec::new();
    for patch in doc.as_sequence().into_iter().flatten() {
        if dsh_plugin_config(patch).is_some() {
            plugins.push(patch);
        }
        plugins.extend(
            patch
                .get("insert")
                .and_then(YamlValue::as_sequence)
                .into_iter()
                .flatten()
                .filter(|plugin| dsh_plugin_config(plugin).is_some()),
        );
    }
    plugins
}

fn dsh_replace_config(entry: &mut YamlValue, config: YamlValue) {
    if let Some(mapping) = entry.as_mapping_mut() {
        mapping.insert(YamlValue::String("config".into()), config);
    }
}

/// 扫描 DSH patch 中由 mcp-client 插件声明的服务器;损坏文件与无关插件不暴露条目。
pub(super) fn read_dsh_servers(path: &Path) -> Vec<super::McpServerEntry> {
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(doc) = DshYamlDocument::parse(&raw) else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for plugin in dsh_plugins(&doc.value) {
        let Some(config) = dsh_plugin_config(plugin) else {
            continue;
        };
        let Some(name) = dsh_server_name(config) else {
            continue;
        };
        let Ok(config) = dsh_config_json(config, &doc.js_tag) else {
            continue;
        };
        entries.push(super::McpServerEntry {
            name: name.to_string(),
            config,
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

pub(super) struct McpDocument {
    original: Option<Vec<u8>>,
    json: Option<Value>,
    toml: Option<DocumentMut>,
    yaml: Option<DshYamlDocument>,
}

impl McpDocument {
    pub fn read(root: &Path, target: &McpTarget) -> RlResult<Self> {
        let path = safe_path(root, target.path)?;
        let original = match fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let text = original
            .as_ref()
            .map(|bytes| std::str::from_utf8(bytes))
            .transpose()
            .map_err(|e| problem(e.to_string()))?;
        if target.dialect == "dsh" {
            let doc = DshYamlDocument::parse(text.unwrap_or(""))
                .map_err(|e| problem(format!("{}: {e}", target.path)))?;
            Ok(Self {
                original,
                json: None,
                toml: None,
                yaml: Some(doc),
            })
        } else if target.dialect == "codex" {
            let doc = text
                .unwrap_or("")
                .parse::<DocumentMut>()
                .map_err(|e| problem(e.to_string()))?;
            if doc
                .get(target.key)
                .is_some_and(|item| item.as_table_like().is_none())
            {
                return Err(problem(target.path));
            }
            Ok(Self {
                original,
                json: None,
                toml: Some(doc),
                yaml: None,
            })
        } else {
            // VS Code 与 OpenCode 2 配置为 JSONC(允许注释/尾逗号),读侧容忍;
            // 写回(edit)仍输出纯 JSON,注释不保留。
            let value = text
                .map(|text| {
                    if target.jsonc() {
                        parse_jsonc(text)
                    } else {
                        serde_json::from_str::<Value>(text)
                    }
                })
                .transpose()
                .map_err(|e| problem(e.to_string()))?
                .unwrap_or(json!({}));
            let mut cursor = &value;
            if !cursor.is_object() {
                return Err(problem(target.path));
            }
            for key in target.key.split('.') {
                match cursor.get(key) {
                    Some(next) if next.is_object() => cursor = next,
                    Some(_) => {
                        return Err(problem(format!(
                            "{}: {} is not an object",
                            target.path, target.key
                        )))
                    }
                    None => break,
                }
            }
            Ok(Self {
                original,
                json: Some(value),
                toml: None,
                yaml: None,
            })
        }
    }

    pub fn entry(&self, target: &McpTarget, name: &str) -> RlResult<Option<Value>> {
        if let Some(doc) = &self.yaml {
            for plugin in dsh_plugins(&doc.value) {
                if let Some(config) = dsh_plugin_config(plugin) {
                    if dsh_server_name(config) == Some(name) {
                        return dsh_config_json(config, &doc.js_tag).map(Some);
                    }
                }
            }
            return Ok(None);
        }
        if let Some(doc) = &self.toml {
            // toml 的 serde 转换可处理普通表和 inline table,且不会吞掉解析错误。
            let value: toml::Value = toml::from_str(&doc.to_string())
                .map_err(|e: toml::de::Error| problem(e.to_string()))?;
            return value
                .get(target.key)
                .and_then(|map| map.get(name))
                .map(serde_json::to_value)
                .transpose()
                .map_err(|e| problem(e.to_string()));
        }
        let mut cursor = self.json.as_ref().unwrap();
        for key in target.key.split('.') {
            let Some(next) = cursor.get(key) else {
                return Ok(None);
            };
            cursor = next;
        }
        Ok(cursor.get(name).cloned())
    }

    pub fn edit(
        mut self,
        target: &McpTarget,
        name: &str,
        value: Option<&Value>,
    ) -> RlResult<Vec<u8>> {
        if let Some(doc) = self.yaml.as_mut() {
            let patches = doc
                .value
                .as_sequence_mut()
                .ok_or_else(|| problem(target.path))?;
            if let Some(value) = value {
                validate_dsh_name(name)?;
                let config = dsh_yaml_definition(value, &doc.js_tag)?;
                let mut replaced = false;
                for patch in patches.iter_mut() {
                    if dsh_plugin_config(patch).and_then(dsh_server_name) == Some(name) {
                        dsh_replace_config(patch, config.clone());
                        replaced = true;
                    }
                    for existing in patch
                        .get_mut("insert")
                        .and_then(YamlValue::as_sequence_mut)
                        .into_iter()
                        .flatten()
                    {
                        if dsh_plugin_config(existing).and_then(dsh_server_name) == Some(name) {
                            dsh_replace_config(existing, config.clone());
                            replaced = true;
                        }
                    }
                }
                if !replaced {
                    let mut plugin = serde_yaml_ng::to_value(json!({
                        "id": format!("repomeow-mcp-{name}"),
                        "name": "@deepseek-ai/dsh-mcp-client",
                        "config": value,
                    }))
                    .map_err(|e| problem(e.to_string()))?;
                    dsh_replace_config(&mut plugin, config);
                    let mut patch = serde_yaml_ng::to_value(json!({ "insert": [] }))
                        .map_err(|e| problem(e.to_string()))?;
                    patch
                        .get_mut("insert")
                        .and_then(YamlValue::as_sequence_mut)
                        .unwrap()
                        .push(plugin);
                    patches.push(patch);
                }
            } else {
                patches.retain_mut(|patch| {
                    if dsh_plugin_config(patch).and_then(dsh_server_name) == Some(name) {
                        return false;
                    }
                    if let Some(plugins) =
                        patch.get_mut("insert").and_then(YamlValue::as_sequence_mut)
                    {
                        plugins.retain(|plugin| {
                            dsh_plugin_config(plugin).and_then(dsh_server_name) != Some(name)
                        });
                    }
                    true
                });
                patches.retain(|patch| {
                    patch
                        .get("insert")
                        .and_then(YamlValue::as_sequence)
                        .is_none_or(|plugins| !plugins.is_empty())
                });
            }
            return doc.to_string();
        }
        if let Some(doc) = self.toml.as_mut() {
            if doc.get(target.key).is_none() && value.is_some() {
                doc.insert(target.key, Item::Table(Table::new()));
            }
            if let Some(table) = doc.get_mut(target.key).and_then(Item::as_table_like_mut) {
                if let Some(value) = value {
                    table.insert(name, json_to_toml_item(value)?);
                } else {
                    table.remove(name);
                }
            }
            Ok(doc.to_string().into_bytes())
        } else {
            let mut cursor = self.json.as_mut().unwrap();
            for key in target.key.split('.') {
                cursor = cursor
                    .as_object_mut()
                    .ok_or_else(|| problem(target.path))?
                    .entry(key)
                    .or_insert(json!({}));
            }
            let map = cursor.as_object_mut().ok_or_else(|| problem(target.path))?;
            if let Some(value) = value {
                if target.agents.contains(&"pi") {
                    validate_pi_name(name)?;
                    let normalized = name.replace('-', "_");
                    if map
                        .keys()
                        .any(|key| key != name && key.replace('-', "_") == normalized)
                    {
                        return Err(problem(format!("Pi: conflicting MCP server name: {name}")));
                    }
                }
                map.insert(name.to_string(), value.clone());
            } else {
                map.remove(name);
            }
            serde_json::to_vec_pretty(self.json.as_ref().unwrap())
                .map_err(|e| problem(e.to_string()))
        }
    }

    pub fn unchanged(&self, root: &Path, target: &McpTarget) -> RlResult<bool> {
        let current = Self::read(root, target)?;
        Ok(current.original == self.original)
    }
}
