//! 从 CC Switch(`~/.cc-switch`)读取供应商供设置页导入。
//!
//! CC Switch 3.x 以 SQLite(`cc-switch.db`)为唯一事实源,旧版为 `config.json`;
//! openclaw / pi 的 `api` 在四种已实现 wire adapter 内原样保留;codex / opencode /
//! hermes / grokbuild 只在能明确判定为 OpenAI Chat Completions 时导入。

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::error::{AppError, AppResult, ErrorCode};

use super::catalog::AiModelDef;
mod parse;
mod read;
#[cfg(test)]
mod tests;

use parse::*;
use read::*;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CcSwitchProvider {
    /// CC Switch 内的供应商 id(前端去重后作为厂商 id 候选)。
    pub id: String,
    pub name: String,
    /// 来源应用:codex / opencode / openclaw / pi / hermes / grokbuild。
    pub app: String,
    pub base_url: String,
    /// 可能为空(如密钥走环境变量),导入后由用户补齐。
    pub api_key: String,
    pub api: String,
    #[serde(default)]
    pub models: Vec<AiModelDef>,
    /// 在 CC Switch 中是否为该应用当前启用项。
    pub current: bool,
}

/// 扫描结果;`found = false` 表示本机未安装/未配置过 CC Switch。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CcSwitchScan {
    pub found: bool,
    pub providers: Vec<CcSwitchProvider>,
}

/// CC Switch 管理的一个技能(~/.cc-switch/skills/<directory>/,SKILL.md 含 frontmatter)。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CcSwitchSkill {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// skills/ 下的子目录名(导出到项目时的目标目录名)。
    pub directory: String,
    /// 在 CC Switch 中对哪些应用启用(claude/codex/gemini/grokbuild/opencode/hermes)。
    #[serde(default)]
    pub enabled_apps: Vec<String>,
}

/// CC Switch 管理的一个 MCP 服务器(mcp_servers 表,server_config 为各应用原生 JSON)。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CcSwitchMcpServer {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// 原始服务器定义(stdio/sse/http 等),导出项目 .mcp.json 时按 name 键原样写入。
    pub server_config: Value,
    #[serde(default)]
    pub enabled_apps: Vec<String>,
}

/// cc-switch 的 skills + MCP 扫描结果;`found = false` 表示本机没有 ~/.cc-switch。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CcSwitchAssets {
    pub found: bool,
    #[serde(default)]
    pub skills: Vec<CcSwitchSkill>,
    #[serde(default)]
    pub mcp_servers: Vec<CcSwitchMcpServer>,
}

/// `~/.cc-switch/` 目录(home_dir 失败按 IO 错误上抛)。
pub fn cc_switch_dir(app: &AppHandle) -> AppResult<PathBuf> {
    let home = app
        .path()
        .home_dir()
        .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))?;
    Ok(home.join(".cc-switch"))
}

/// 从本机 `~/.cc-switch/` 扫描可导入的供应商
/// (openclaw/pi 支持四种 wire adapter,codex/opencode/hermes/grokbuild 仅 OpenAI Chat)。
pub fn scan_cc_switch_providers(app: &AppHandle) -> AppResult<CcSwitchScan> {
    scan_at(&cc_switch_dir(app)?)
}

/// 读取 CC Switch 管理的技能与 MCP 服务器(仅 3.x SQLite 库有这两张表;
/// 旧版 config.json 或缺表按空列表处理)。skills 正文在 skills/ 目录,DB 只存元数据。
pub fn scan_cc_switch_assets(app: &AppHandle) -> AppResult<CcSwitchAssets> {
    let dir = cc_switch_dir(app)?;
    let db_path = dir.join("cc-switch.db");
    if !db_path.is_file() {
        return Ok(CcSwitchAssets {
            found: false,
            skills: Vec::new(),
            mcp_servers: Vec::new(),
        });
    }
    with_staged_db(&db_path, |copy| {
        let conn = Connection::open(copy)?;
        Ok(CcSwitchAssets {
            found: true,
            skills: query_skills(&conn)?,
            mcp_servers: query_mcp_servers(&conn)?,
        })
    })
}

/// 按 id 取单个 MCP 服务器的 (name, server_config),供导出到项目 .mcp.json;
/// 找不到返回 None(调用方报「配置已不存在」)。
pub fn mcp_server_by_id(dir: &Path, server_id: &str) -> AppResult<Option<(String, Value)>> {
    let db_path = dir.join("cc-switch.db");
    if !db_path.is_file() {
        return Ok(None);
    }
    with_staged_db(&db_path, |copy| {
        let conn = Connection::open(copy)?;
        let mut stmt = match conn
            .prepare("SELECT name, server_config FROM mcp_servers WHERE id = ?1")
        {
            Ok(stmt) => stmt,
            Err(rusqlite::Error::SqliteFailure(_, Some(message)))
                if message.contains("no such table") =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        let row = stmt
            .query_row([server_id], |row| {
                let config_text: String = row.get(1)?;
                Ok((
                    row.get::<_, String>(0)?,
                    serde_json::from_str(&config_text).unwrap_or(Value::Null),
                ))
            })
            .optional()?;
        Ok(row)
    })
}

fn scan_at(dir: &Path) -> AppResult<CcSwitchScan> {
    let db_path = dir.join("cc-switch.db");
    let legacy_path = dir.join("config.json");
    let raw = if db_path.is_file() {
        read_providers_db(&db_path)?
    } else if legacy_path.is_file() {
        read_legacy_config(&legacy_path)
    } else {
        return Ok(CcSwitchScan {
            found: false,
            providers: Vec::new(),
        });
    };
    let mut providers: Vec<CcSwitchProvider> = raw.iter().filter_map(convert).collect();
    // 稳定排序:当前启用项优先,其余按名称
    providers.sort_by(|a, b| b.current.cmp(&a.current).then_with(|| a.name.cmp(&b.name)));
    Ok(CcSwitchScan {
        found: true,
        providers,
    })
}

