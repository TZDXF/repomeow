//! MCP 方言在后端统一转换,前端只传资源 ID,不接收任意配置/目标路径。
use super::super::resource_library::{McpServer, McpServerInput, RlResult};
use super::{
    deployment_io::{problem, safe_path},
    mcp_formats::{json_to_toml_item, parse_jsonc},
    McpTarget,
};
use serde_json::{json, Value};
use serde_yaml_ng::Value as YamlValue;
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

fn dsh_plugin_config(entry: &YamlValue) -> Option<&YamlValue> {
    (entry.get("name")?.as_str()? == "@deepseek-ai/dsh-mcp-client")
        .then(|| entry.get("config"))
        .flatten()
}

fn dsh_server_name(config: &YamlValue) -> Option<&str> {
    config.get("serverName")?.as_str()
}

/// 扫描 DSH patch 中由 mcp-client 插件声明的服务器;损坏文件与无关插件不暴露条目。
pub(super) fn read_dsh_servers(path: &Path) -> Vec<super::McpServerEntry> {
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(doc) = serde_yaml_ng::from_str::<YamlValue>(&raw) else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for patch in doc.as_sequence().into_iter().flatten() {
        for plugin in patch
            .get("insert")
            .and_then(YamlValue::as_sequence)
            .into_iter()
            .flatten()
        {
            let Some(config) = dsh_plugin_config(plugin) else {
                continue;
            };
            let Some(name) = dsh_server_name(config) else {
                continue;
            };
            let Ok(config) = serde_json::to_value(config) else {
                continue;
            };
            entries.push(super::McpServerEntry {
                name: name.to_string(),
                config,
            });
        }
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

pub(super) struct McpDocument {
    original: Option<Vec<u8>>,
    json: Option<Value>,
    toml: Option<DocumentMut>,
    yaml: Option<YamlValue>,
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
            let doc = if text.is_some_and(|text| text.trim().is_empty()) || text.is_none() {
                YamlValue::Sequence(Vec::new())
            } else {
                serde_yaml_ng::from_str::<YamlValue>(text.unwrap())
                    .map_err(|e| problem(e.to_string()))?
            };
            if !doc.is_sequence() {
                return Err(problem(format!(
                    "{}: patch must be a sequence",
                    target.path
                )));
            }
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
            for patch in doc.as_sequence().into_iter().flatten() {
                for plugin in patch
                    .get("insert")
                    .and_then(YamlValue::as_sequence)
                    .into_iter()
                    .flatten()
                {
                    if let Some(config) = dsh_plugin_config(plugin) {
                        if dsh_server_name(config) == Some(name) {
                            return serde_json::to_value(config)
                                .map(Some)
                                .map_err(|e| problem(e.to_string()));
                        }
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
            let patches = doc.as_sequence_mut().ok_or_else(|| problem(target.path))?;
            if let Some(value) = value {
                validate_dsh_name(name)?;
                let plugin = serde_yaml_ng::to_value(json!({
                    "id": format!("repomeow-mcp-{name}"),
                    "name": "@deepseek-ai/dsh-mcp-client",
                    "config": value,
                }))
                .map_err(|e| problem(e.to_string()))?;
                let mut replaced = false;
                for patch in patches.iter_mut() {
                    for existing in patch
                        .get_mut("insert")
                        .and_then(YamlValue::as_sequence_mut)
                        .into_iter()
                        .flatten()
                    {
                        if dsh_plugin_config(existing).and_then(dsh_server_name) == Some(name) {
                            *existing = plugin.clone();
                            replaced = true;
                        }
                    }
                }
                if !replaced {
                    let patch = serde_yaml_ng::to_value(json!({ "insert": [plugin] }))
                        .map_err(|e| problem(e.to_string()))?;
                    patches.push(patch);
                }
            } else {
                for patch in patches.iter_mut() {
                    if let Some(plugins) =
                        patch.get_mut("insert").and_then(YamlValue::as_sequence_mut)
                    {
                        plugins.retain(|plugin| {
                            dsh_plugin_config(plugin).and_then(dsh_server_name) != Some(name)
                        });
                    }
                }
                patches.retain(|patch| {
                    patch
                        .get("insert")
                        .and_then(YamlValue::as_sequence)
                        .is_none_or(|plugins| !plugins.is_empty())
                });
            }
            return serde_yaml_ng::to_string(doc)
                .map(String::into_bytes)
                .map_err(|e| problem(e.to_string()));
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
