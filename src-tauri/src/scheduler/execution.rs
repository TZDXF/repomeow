//! 调度执行:加载项目路径,标记上次运行时间,执行一次定时任务(拉提交 → 调 AI → 落库 → emit)。
use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{Datelike, Local};
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::git::{run_git_current_user, run_git_log};
use crate::commands::report::{ReportGeneratedPayload, ReportSchedule};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::GitCommitInfo;
use crate::workday;

use super::calendar::{daily_report_date, work_week_start};
use super::config::{
    build_report_prompt, call_ai, default_prompt, default_schedule_name, load_ai_config,
    load_language, load_report_prompt,
};
/// 复用 AppHandle 托管的 Db 连接,与主线程共享同一把锁,避免独立连接同文件
/// 的毫秒级竞争窗口。
pub fn load_project_paths(app: &AppHandle) -> AppResult<HashMap<i64, (String, String, String)>> {
    let db = app.state::<crate::db::Db>();
    let conn = db.0.lock().unwrap();
    let mut stmt =
        conn.prepare("SELECT id, path, name, description FROM projects WHERE archived_at IS NULL")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    let mut map = HashMap::new();
    for row in rows {
        let (id, path, name, desc) = row?;
        map.insert(id, (path, name, desc));
    }
    Ok(map)
}

/// 更新 last_run_at,并要求恰好命中一个定时任务。
/// 调用方传入 AppHandle 托管的数据库连接,避免使用独立连接。
pub fn mark_last_run(conn: &rusqlite::Connection, schedule_id: &str) -> AppResult<()> {
    let now_ts = Local::now().timestamp();
    let updated = conn.execute(
        "UPDATE report_schedules SET last_run_at = ?1 WHERE id = ?2",
        rusqlite::params![now_ts, schedule_id],
    )?;
    if updated != 1 {
        return Err(AppError::coded(
            ErrorCode::DbError,
            format!(
                "scheduler: last_run_at update missing schedule_id={schedule_id} updated={updated}"
            ),
        ));
    }
    Ok(())
}

/// 删除指定 report_history 行;关联的 report_commits 由外键级联删除。
/// 未命中恰好一行时返回错误。
pub fn delete_report_history_row(conn: &rusqlite::Connection, history_id: i64) -> AppResult<()> {
    let deleted = conn.execute(
        "DELETE FROM report_history WHERE id = ?1",
        rusqlite::params![history_id],
    )?;
    if deleted != 1 {
        return Err(AppError::coded(
            ErrorCode::DbError,
            format!(
                "scheduler: orphan report_history cleanup failed id={history_id} deleted={deleted}"
            ),
        ));
    }
    Ok(())
}

// ── schedule execution ─────────────────────────────────────────────────

/// 执行一次定时任务:拉取提交 → 调 AI → 写报告历史 → 更新运行时间 → emit 事件。
/// 成功返回报告历史 id;任何失败以 Err 返回。
///
/// 报告历史和提交明细在同一事务中写入。更新运行时间失败时尝试删除已写入的
/// 报告历史,并保留原始更新错误;只有运行时间更新成功后才发送前端事件。
pub(crate) async fn fire_schedule(
    app: &AppHandle,
    data_dir: &PathBuf,
    schedule: &ReportSchedule,
) -> AppResult<i64> {
    let task_label = if schedule.name.is_empty() {
        schedule.report_type.clone()
    } else {
        schedule.name.clone()
    };
    let mut background_task =
        crate::background_task::BackgroundTask::new(app, "report", task_label, 4);
    eprintln!(
        "[scheduler] 触发任务({}) @ {}",
        schedule.report_type,
        Local::now().format("%Y-%m-%d %H:%M")
    );

    // 1. 读取项目路径(复用 AppHandle 托管 Db 锁)
    let projects = load_project_paths(app).map_err(|e| {
        eprintln!("[scheduler] 读取项目列表失败: {e}");
        e
    })?;

    // 1.5 按标签反查项目,与显式选择的 project_ids 取并集
    // (任一选中标签命中即纳入;新项目打上标签后自动生效,无需修改任务)
    let effective_project_ids: Vec<i64> = {
        let db = app.state::<crate::db::Db>();
        let conn = db.0.lock().unwrap();
        let tag_pids =
            crate::commands::report::tag_project_ids(&conn, &schedule.tag_ids).map_err(|e| {
                eprintln!("[scheduler] 按标签反查项目失败: {e}");
                e
            })?;
        let mut ids = schedule.project_ids.clone();
        for pid in tag_pids {
            if !ids.contains(&pid) {
                ids.push(pid);
            }
        }
        ids
    };

    // 2. 读取 AI 配置(三项任意一项缺失即拒绝执行)
    let ai_config = load_ai_config(data_dir);
    if ai_config.ai_base_url.is_empty() {
        eprintln!("[scheduler] AI 接口地址未配置,跳过生成");
        return Err(AppError::coded(ErrorCode::AiNotConfigured, "base_url"));
    }
    if ai_config.ai_api_key.is_empty() {
        eprintln!("[scheduler] AI API Key 未配置,跳过生成");
        return Err(AppError::coded(ErrorCode::AiNotConfigured, "api_key"));
    }
    if ai_config.ai_model.is_empty() {
        eprintln!("[scheduler] AI 模型未配置,跳过生成");
        return Err(AppError::coded(ErrorCode::AiNotConfigured, "model"));
    }

    // 3. 读取提示词模板(按报告类型) + 语言(决定 schedule 兜底名 / 报告连词 / AI 输出语言)
    let language = load_language(data_dir);
    // schedule.name 为空时给个兜底展示名,沿界面语言;前端 UI 自己再 `s.name || t("reportSchedule.title")` 兜一次
    let is_weekly = schedule.report_type == "weekly";
    let default_name = default_schedule_name(is_weekly, &language);
    let schedule_name = if schedule.name.is_empty() {
        default_name.to_string()
    } else {
        schedule.name.clone()
    };

    let custom_prompt = load_report_prompt(data_dir, &schedule.report_type);
    let system_prompt = if custom_prompt.trim().is_empty() {
        default_prompt(&schedule.report_type).to_string()
    } else {
        custom_prompt
    };
    let system_prompt = if language == "zh-CN" {
        format!("{system_prompt}\n\nRespond in 中文.")
    } else {
        format!("{system_prompt}\n\nRespond in English.")
    };
    background_task.set_completed(1);

    // 4. 计算日期范围
    let today = Local::now().date_naive();
    let today_str = today.format("%Y-%m-%d").to_string();
    let (date_from, date_to) = if is_weekly {
        if schedule.weekly_workweek {
            // 工作周模式:工作周起始日(含前挂的调休周日)~ 今天
            // (chinese-days 缓存在安装目录 data/,与 data_dir 分开解析)
            let start = work_week_start(today, &workday::cache_root());
            (start.format("%Y-%m-%d").to_string(), today_str.clone())
        } else {
            // 自定义模式:本周 weekly_start_weekday 日 ~ 今天(起始日晚于今天时取上周对应日)
            let monday =
                today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
            let offset = schedule.weekly_start_weekday.clamp(1, 7) - 1;
            let mut start = monday + chrono::Duration::days(offset as i64);
            if start > today {
                start -= chrono::Duration::days(7);
            }
            (start.format("%Y-%m-%d").to_string(), today_str.clone())
        }
    } else {
        // 日报按任务配置取前一天(次日生成,覆盖全天)或当天
        let report_date = daily_report_date(today, schedule.previous_day);
        let date = report_date.format("%Y-%m-%d").to_string();
        (date.clone(), date)
    };
    let since = format!("{date_from} 00:00:00");
    let until = format!("{date_to} 23:59:59");
    let range_label = if date_from == date_to {
        date_from.clone()
    } else if language == "zh-CN" {
        format!("{date_from} 至 {date_to}")
    } else {
        format!("{date_from} ~ {date_to}")
    };

    // 5. 对每个项目拉取 git_log
    let mut commits_by_project: Vec<(i64, String, String, Vec<GitCommitInfo>)> = Vec::new();
    for &pid in &effective_project_ids {
        if let Some((path, name, desc)) = projects.get(&pid) {
            // 解析作者过滤
            let author: Option<String> = if schedule.author_mode == "me" {
                run_git_current_user(path).ok().and_then(|u| {
                    let name = u.name;
                    if name.is_empty() {
                        None
                    } else {
                        Some(name)
                    }
                })
            } else {
                None
            };
            let commits = run_git_log(
                path,
                Some(&since),
                Some(&until),
                Some(500),
                author.as_deref(),
            )
            .unwrap_or_default();
            commits_by_project.push((pid, name.clone(), desc.clone(), commits));
        }
    }

    // 过滤掉时间范围内没有提交的项目(不进 prompt、不写历史)
    commits_by_project.retain(|(_, _, _, c)| !c.is_empty());

    if commits_by_project.is_empty() {
        eprintln!("[scheduler] {schedule_name}: 无提交记录,跳过");
        // 不写历史、不更新 last_run_at,让下一次循环再次尝试
        return Err(AppError::coded(ErrorCode::SchedulerNoCommits, ""));
    }
    background_task.set_completed(2);

    // 6. 组装 prompt
    let prompt_data: Vec<(String, String, Vec<GitCommitInfo>)> = commits_by_project
        .iter()
        .map(
            |(_, name, desc, commits): &(i64, String, String, Vec<GitCommitInfo>)| {
                (name.clone(), desc.clone(), commits.clone())
            },
        )
        .collect();
    let user_prompt = build_report_prompt(&prompt_data, &range_label, &language);

    // 7. 调用 AI(失败不写历史、不更新 last_run_at)
    let ai_started = std::time::Instant::now();
    let output = call_ai(&ai_config, &system_prompt, &user_prompt)
        .await
        .map_err(|e| {
            eprintln!("[scheduler] {schedule_name}: AI 调用失败: {e}");
            e
        })?;
    let result = output.text;
    let ai_usage = output.usage;
    background_task.set_completed(3);

    // 8. 一次性事务保存报告历史 + 关联 commits,失败回滚。
    // 复用 AppHandle 托管 Db 锁,避免与主线程独立连接产生毫秒级竞争窗口。
    let db = app.state::<crate::db::Db>();
    let mut conn = db.0.lock().unwrap();
    let history_id = {
        let tx = conn.transaction()?;
        let now = crate::time_util::now_ts();
        let project_ids: Vec<i64> = commits_by_project.iter().map(|(id, _, _, _)| *id).collect();
        let ids_json = serde_json::to_string(&project_ids).unwrap_or_default();

        tx.execute(
            "INSERT INTO report_history (project_ids, date_from, date_to, range_label, author_mode, language, period_type, result, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![ids_json, date_from, date_to, range_label, &schedule.author_mode, language, schedule.report_type, result, now],
        )?;
        let report_id = tx.last_insert_rowid();

        for (pid, name, desc, commits) in &commits_by_project {
            let commits_json = serde_json::to_string(commits).unwrap_or_default();
            tx.execute(
                "INSERT INTO report_commits (report_id, project_id, project_name, project_description, commit_data)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![report_id, pid, name, desc, commits_json],
            )?;
        }

        // 用量日志与报告同事务落库;记录失败仅记日志,不影响报告保存
        let usage_record = crate::models::AiUsageRecord {
            task_type: "report".to_string(),
            model: ai_config.ai_model.clone(),
            input_tokens: ai_usage.as_ref().and_then(|usage| usage.input_tokens),
            output_tokens: ai_usage.as_ref().and_then(|usage| usage.output_tokens),
            total_tokens: ai_usage.as_ref().and_then(|usage| usage.total_tokens),
            duration_ms: Some(ai_started.elapsed().as_millis() as i64),
            cached_tokens: ai_usage.as_ref().and_then(|usage| usage.cached_tokens),
        };
        if let Err(e) = crate::commands::usage::insert_usage_row(&tx, &usage_record, now) {
            eprintln!("[scheduler] {schedule_name}: 用量日志写入失败: {e}");
        }

        tx.commit()?;
        report_id
    };

    // 9. 更新 last_run_at;失败时清理已提交的历史并保留原始错误。
    if let Err(mark_err) = mark_last_run(&conn, &schedule.id) {
        match delete_report_history_row(&conn, history_id) {
            Ok(()) => eprintln!(
                "[scheduler] 清理孤儿历史成功: history_id={history_id}, schedule_id={}",
                schedule.id
            ),
            Err(cleanup_err) => eprintln!(
                "[scheduler] 清理孤儿历史失败: history_id={history_id}, cleanup_err={cleanup_err}; \
                 保留原始 mark_last_run 错误继续上抛"
            ),
        }
        return Err(mark_err);
    }
    drop(conn);
    background_task.set_completed(4);

    // 10. 通知前端
    let payload = ReportGeneratedPayload {
        schedule_name: schedule_name.clone(),
        history_id,
        date_from,
        date_to,
    };
    if let Err(e) = app.emit("report://generated", payload) {
        eprintln!("[scheduler] 发送前端通知失败: {e}");
    }

    eprintln!("[scheduler] {schedule_name}: 报告已生成 (id={history_id})");
    Ok(history_id)
}
