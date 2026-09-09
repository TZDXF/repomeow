//! 从 CC Switch(`~/.cc-switch`)读取供应商供设置页导入。
//!
//! CC Switch 3.x 以 SQLite(`cc-switch.db`)为唯一事实源,旧版为 `config.json`;
//! 按来源应用解析出四种已实现 wire adapter 之一:openclaw / pi 的 `api` 原样保留,
//! claude / claude-desktop → anthropic-messages、gemini → google-generative-ai,
//! codex 按 wire_api(chat → completions,缺省 responses → responses)、opencode 按
//! npm SDK 包判定;hermes / grokbuild 固定 OpenAI Chat,无法判定协议的项不导入。

use std::path::{Path, PathBuf};

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
    /// 来源应用:claude / claude-desktop / codex / gemini / opencode / openclaw / pi / hermes / grokbuild。
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

/// `~/.cc-switch/` 目录(home_dir 失败按 IO 错误上抛)。
pub fn cc_switch_dir(app: &AppHandle) -> AppResult<PathBuf> {
    let home = app
        .path()
        .home_dir()
        .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))?;
    Ok(home.join(".cc-switch"))
}

/// 从本机 `~/.cc-switch/` 扫描可导入的供应商
/// (按来源应用映射四种 wire adapter,无法判定协议的项跳过)。
pub fn scan_cc_switch_providers(app: &AppHandle) -> AppResult<CcSwitchScan> {
    scan_at(&cc_switch_dir(app)?)
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
