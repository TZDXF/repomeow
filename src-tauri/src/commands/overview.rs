//! 详情页首屏聚合查询:隐藏项 + 自定义命令一次 IPC 返回,
//! 两条 SELECT 共享一次全局 DB 锁获取,替代 list_hidden_items / list_custom_commands
//! 两条命令分别排队(详情页 PackageScripts / DockerCompose / CustomCommands 三个卡片
//! 同时挂载,各自单独请求会把尾延迟叠加在锁竞争上)。

use serde::Serialize;
use tauri::State;

use crate::commands::{hidden, script};
use crate::db::Db;
use crate::error::AppResult;
use crate::models::{CustomCommand, HiddenItem};

#[derive(Debug, Clone, Serialize)]
pub struct ProjectOverview {
    pub hidden_items: Vec<HiddenItem>,
    pub custom_commands: Vec<CustomCommand>,
}

/// 详情页首屏聚合:一次 IPC + 一次锁内完成两条查询
#[tauri::command]
pub fn get_project_overview(db: State<'_, Db>, project_id: i64) -> AppResult<ProjectOverview> {
    let conn = db.0.lock().unwrap();
    Ok(ProjectOverview {
        hidden_items: hidden::list(&conn, project_id)?,
        custom_commands: script::list_commands(&conn, project_id)?,
    })
}
