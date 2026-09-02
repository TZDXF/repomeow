//! AI 接入配置(多厂商,`~/.repomeow/ai-config.json`)读写命令。
//!
//! 前端设置页与问答面板经这两个命令读写完整配置;Rust 侧各消费方
//! (chat/commit/report/wiki)每次调用时重读文件,配置可热更新。

use std::collections::BTreeMap;

use tauri::AppHandle;

use crate::ai::catalog::{self, AiConfigFile, AiProvider};
use crate::ai::cc_switch::{self, CcSwitchScan};
use crate::commands::open;
use crate::error::{AppError, AppResult, ErrorCode};

/// 读取 AI 接入配置;文件缺失/损坏时自动播种(含旧 settings.json 迁移)。
#[tauri::command]
pub fn ai_config_get(app: AppHandle) -> AppResult<AiConfigFile> {
    Ok(catalog::load_ai_config_file(&app))
}

/// 保存 AI 接入配置(全量覆盖,原子写;保存前做引用归一化)。
#[tauri::command]
pub fn ai_config_save(app: AppHandle, config: AiConfigFile) -> AppResult<()> {
    catalog::save_ai_config_file(&app, &config)
}

/// 内置厂商目录(添加厂商对话框的候选清单;含各厂商预置模型,apiKey 恒为空)。
#[tauri::command]
pub fn ai_config_builtin_providers() -> AppResult<BTreeMap<String, AiProvider>> {
    Ok(catalog::builtin_config().providers)
}

/// 在系统文件管理器中打开配置文件所在目录(先确保配置已播种落盘)。
#[tauri::command]
pub fn ai_config_reveal(app: AppHandle) -> AppResult<()> {
    let path = catalog::config_path(&app)?;
    catalog::load_ai_config_file(&app);
    let dir = path
        .parent()
        .ok_or_else(|| AppError::coded(ErrorCode::InvalidPath, path.display().to_string()))?;
    open::open_explorer(&dir.to_string_lossy())
}

/// 扫描本机 CC Switch(~/.cc-switch)中 OpenAI chat 兼容的供应商,供设置页选择导入。
#[tauri::command]
pub fn ai_cc_switch_providers(app: AppHandle) -> AppResult<CcSwitchScan> {
    cc_switch::scan_cc_switch_providers(&app)
}
