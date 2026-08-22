//! 日报/周报定时调度引擎(订阅模式)。
//!
//! 启动一个 tokio 后台循环:
//! 1. 计算所有启用定时任务的下次触发时间
//! 2. `sleep_until` 精确等待到最早触发时刻
//! 3. 时间到 → 拉取 git_log → 调 AI → 保存报告历史 → emit 事件
//! 4. 定时任务变更时通过 Notify 唤醒重算

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use chrono::{Datelike, Local, NaiveDate, NaiveTime, Timelike};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};
use tokio::select;
use tokio::time::{self, Duration, Instant};

use crate::commands::git::{run_git_current_user, run_git_log};
use crate::commands::report::{read_schedules, ReportGeneratedPayload, ReportSchedule};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::GitCommitInfo;
use crate::workday;

/// 调度循环间隔(用于空闲等待与错误重试)
const IDLE_INTERVAL: Duration = Duration::from_secs(60);

// ── AI config (from settings.json) ──────────────────────────────────────

#[derive(Deserialize, Default)]
struct AiConfig {
    #[serde(default)]
    ai_base_url: String,
    #[serde(default)]
    ai_api_key: String,
    #[serde(default)]
    ai_model: String,
}

fn load_ai_config(data_dir: &PathBuf) -> AiConfig {
    let path = data_dir.join("settings.json");
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .map(|v| AiConfig {
            ai_base_url: v
                .get("aiBaseUrl")
                .and_then(|x| x.as_str())
                .map(|s| s.trim().to_string())
                .unwrap_or_default(),
            ai_api_key: v
                .get("aiApiKey")
                .and_then(|x| x.as_str())
                .map(|s| s.trim().to_string())
                .unwrap_or_default(),
            ai_model: v
                .get("aiModel")
                .and_then(|x| x.as_str())
                .map(|s| s.trim().to_string())
                .unwrap_or_default(),
        })
        .unwrap_or_default()
}

/// 从 settings.json 读取界面语言(报告语言与其保持一致),默认 zh-CN
fn load_language(data_dir: &PathBuf) -> String {
    let path = data_dir.join("settings.json");
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| {
            v.get("language")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "zh-CN".into())
}

/// schedule.name 为空时的兜底展示名,沿界面语言分支
/// (en-US 走英文,其他语言都按 zh-CN 处理 —— 目前只支持这两语言)
fn default_schedule_name(is_weekly: bool, language: &str) -> &'static str {
    match (is_weekly, language) {
        (true, "en-US") => "Weekly Schedule",
        (false, "en-US") => "Daily Schedule",
        (true, _) => "周报定时任务",
        (false, _) => "日报定时任务",
    }
}

/// 按报告类型读取自定义提示词(日报 report.md / 周报 report-weekly.md)
fn load_report_prompt(data_dir: &PathBuf, report_type: &str) -> String {
    let file = if report_type == "weekly" {
        "report-weekly.md"
    } else {
        "report.md"
    };
    fs::read_to_string(data_dir.join("prompts").join(file)).unwrap_or_default()
}

/// 内置默认提示词(与前端 ai-prompts.ts 一致)
fn default_prompt(report_type: &str) -> &'static str {
    if report_type == "weekly" {
        "You are a technical project manager. Generate a concise weekly report in Markdown based on git commit records.\n\nGuidelines:\n- Group commits by project\n- Summarize the week's progress, highlighting key changes and their impact\n- Use bullet points for clarity\n- Keep it professional and actionable"
    } else {
        "You are a technical project manager. Generate a concise daily report in Markdown based on git commit records.\n\nGuidelines:\n- Group commits by project\n- Highlight key changes and their impact\n- Use bullet points for clarity\n- Keep it professional and actionable"
    }
}

// ── OpenAI Chat Completions ────────────────────────────────────────────

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatMessage {
    content: String,
}

/// 大小写不敏感地查找 ASCII 子串,返回字节索引(标签均为 ASCII,字节索引与原串对齐)
fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    if needle.len() > haystack.len() {
        return None;
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

/// 大小写不敏感地判断 ASCII 前缀
fn starts_with_ascii_case_insensitive(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

/// 剥离输出开头的 <think>...</think> 思考块(推理模型或中转服务可能把思考过程混入正文)。
/// 只处理响应起始位置的思考块:正文中出现的 <think> 字样(如报告介绍该功能本身)必须保留
fn strip_thinking(text: &str) -> String {
    let mut out = text.trim_start();
    while starts_with_ascii_case_insensitive(out, "<think>") {
        match find_ascii_case_insensitive(out, "</think>") {
            Some(i) => out = out[i + "</think>".len()..].trim_start(),
            // 未闭合的思考块:整段都是思考,没有正文
            None => return String::new(),
        }
    }
    out.trim().to_string()
}

/// 按服务商/模型名给出"关闭思考模式"请求参数(仅匹配已知支持方,避免严格网关因未知字段 400)
fn thinking_off_params(base_url: &str, model: &str) -> serde_json::Map<String, Value> {
    let s = format!("{} {}", base_url.to_lowercase(), model.to_lowercase());
    let m = model.to_lowercase();
    let mut map = serde_json::Map::new();
    if s.contains("qwen") || s.contains("dashscope") || s.contains("aliyuncs") {
        // 阿里云百炼 / DashScope 兼容模式
        map.insert("enable_thinking".into(), Value::Bool(false));
        if !s.contains("dashscope") && !s.contains("aliyuncs") {
            // 自建 vLLM/SGLang 部署的 Qwen3 系
            map.insert(
                "chat_template_kwargs".into(),
                serde_json::json!({ "enable_thinking": false }),
            );
        }
    } else if s.contains("glm")
        || s.contains("zhipu")
        || s.contains("bigmodel")
        || s.contains("doubao")
        || s.contains("volces")
    {
        // 智谱 GLM / 火山方舟(豆包)系
        map.insert("thinking".into(), serde_json::json!({ "type": "disabled" }));
    } else if m.starts_with("step-3") || m.starts_with("step-r") {
        // 阶跃星辰 Step 推理系(Step 3.5/3.7 Flash 等):官方接口无完全关闭思考的开关,
        // 用最低推理档尽量缩短思考;思考内容经独立 reasoning 字段返回,不混入正文
        map.insert("reasoning_effort".into(), Value::String("low".into()));
    }
    map
}

async fn call_ai(
    client: &Client,
    config: &AiConfig,
    system_prompt: &str,
    user_prompt: &str,
) -> AppResult<String> {
    let url = format!(
        "{}/chat/completions",
        config.ai_base_url.trim_end_matches('/')
    );

    let mut body = serde_json::json!({
        "model": config.ai_model,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_prompt}
        ]
    });
    // 命中已知推理模型提供方时注入"关闭思考模式"参数
    if let Some(obj) = body.as_object_mut() {
        obj.extend(thinking_off_params(&config.ai_base_url, &config.ai_model));
    }

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", config.ai_api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::coded(ErrorCode::AiRequestFailed, e.to_string()))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(AppError::coded(
            ErrorCode::AiResponseError,
            format!("status={status} body={text}"),
        ));
    }

    let data: ChatResponse = resp
        .json()
        .await
        .map_err(|e| AppError::coded(ErrorCode::AiResponseParseFailed, e.to_string()))?;

    let content = data
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .ok_or_else(|| AppError::coded(ErrorCode::AiEmptyResponse, ""))?;
    Ok(strip_thinking(&content))
}

// ── prompt builder ─────────────────────────────────────────────────────

fn build_report_prompt(
    commits_by_project: &[(String, String, Vec<GitCommitInfo>)], // (name, description, commits)
    range_label: &str,
    language: &str,
) -> String {
    let sections: Vec<String> = commits_by_project
        .iter()
        .map(|(name, desc, commits)| {
            let heading = if desc.is_empty() {
                format!("### {name}")
            } else {
                format!("### {name} — {desc}")
            };
            let lines: Vec<String> = commits
                .iter()
                .map(|c| format!("- [{}] {} ({}, {})", c.date, c.subject, c.hash, c.author))
                .collect();
            if lines.is_empty() {
                format!("{heading}\n(no commits)")
            } else {
                format!("{heading}\n{}", lines.join("\n"))
            }
        })
        .collect();

    let lang = if language == "zh-CN" {
        "中文"
    } else {
        "English"
    };
    format!(
        "Time range: {range_label}.\n\nCommit records:\n{}\n\nRespond in {lang}.",
        sections.join("\n\n")
    )
}

// ── next fire time calculation ─────────────────────────────────────────

/// 计算某个 schedule 的下次触发时刻。
/// 返回 `Option<Instant>`: None 表示该 schedule 无需等待(已禁用或无有效时间)。
/// 周报任务同样按每日 time_of_day 唤醒,是否为触发日由 due_schedules 判定。
fn next_fire(schedule: &ReportSchedule, now_local: &chrono::DateTime<Local>) -> Option<Instant> {
    if !schedule.enabled {
        return None;
    }

    let time_parts: Vec<&str> = schedule.time_of_day.split(':').collect();
    if time_parts.len() != 2 {
        return None;
    }
    let hour: u32 = time_parts[0].parse().ok()?;
    let minute: u32 = time_parts[1].parse().ok()?;
    let target_time = NaiveTime::from_hms_opt(hour, minute, 0)?;

    let today = now_local.date_naive();
    let today_target = today
        .and_time(target_time)
        .and_local_timezone(Local)
        .single()?;

    // 若今天的目标时刻已过,则推到明天
    if today_target <= *now_local {
        let tomorrow = today.succ_opt()?;
        return tomorrow
            .and_time(target_time)
            .and_local_timezone(Local)
            .single()
            .map(|dt| {
                let dur = dt.signed_duration_since(*now_local);
                Instant::now() + Duration::from_secs(dur.to_std().unwrap_or_default().as_secs())
            });
    }

    let dur = today_target.signed_duration_since(*now_local);
    Some(Instant::now() + Duration::from_secs(dur.to_std().unwrap_or_default().as_secs()))
}

/// 返回所有启用 schedule 中最早的下次触发时刻
fn earliest_fire(
    schedules: &[ReportSchedule],
    now_local: &chrono::DateTime<Local>,
) -> Option<Instant> {
    schedules
        .iter()
        .filter_map(|s| next_fire(s, now_local))
        .min()
}

// ── 工作周(连续工作周期)计算 ────────────────────────────────────────────
//
// 工作周 = 一段连续的工作周期,周中单日法定节假日不打断:
//   例1: 周一~周五工作、周三法定节假日 → 工作周仍为 周一~周五
//   例2: 周日调休上班、周一~周四工作 → 工作周为 周日~周四
// 调休周日的归属:下周(周一~周日)内有工作日时前挂为下周工作周起点,
// 否则(如下周整周节假日)留在本周末尾。

/// 某周(周一~周日)内是否存在工作日
fn has_workday_in_week_with(monday: NaiveDate, is_workday: &dyn Fn(NaiveDate) -> bool) -> bool {
    (0..7).any(|i| is_workday(monday + chrono::Duration::days(i)))
}

/// 指定日期所在工作周的起始日(判定逻辑由闭包注入,批量场景可预加载数据)
pub(crate) fn work_week_start_with(
    today: NaiveDate,
    is_workday: &dyn Fn(NaiveDate) -> bool,
) -> NaiveDate {
    let monday = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
    let sunday_before = monday - chrono::Duration::days(1);
    if has_workday_in_week_with(monday, is_workday) && is_workday(sunday_before) {
        return sunday_before;
    }
    // 本周第一个工作日(周一为节假日时顺延)
    (0..7)
        .map(|i| monday + chrono::Duration::days(i))
        .find(|d| is_workday(*d))
        .unwrap_or(monday)
}

/// 今天所在工作周的起始日
pub(crate) fn work_week_start(today: NaiveDate, cache_root: &PathBuf) -> NaiveDate {
    work_week_start_with(today, &|d| workday::is_workday(d, cache_root))
}

/// 指定日期是否为所在工作周的最后一个工作日(判定逻辑由闭包注入)。
/// 供批量生成周报规划复用,与定时周报触发条件保持一致。
pub(crate) fn is_work_week_last_day_with(
    today: NaiveDate,
    is_workday: &dyn Fn(NaiveDate) -> bool,
) -> bool {
    if !is_workday(today) {
        return false;
    }
    let dow = today.weekday().num_days_from_monday(); // 0=周一 .. 6=周日
    if dow == 6 {
        // 今天是调休周日:下周有工作日时,今天前挂为下周工作周起点,不是本周末日
        let next_monday = today + chrono::Duration::days(1);
        return !has_workday_in_week_with(next_monday, is_workday);
    }
    // 今天之后本周内若还有属于本工作周的工作日,则今天不是末日
    for i in 1..=(6 - dow) {
        let d = today + chrono::Duration::days(i as i64);
        if !is_workday(d) {
            continue;
        }
        if i == 6 - dow {
            // d 是本周日(调休):下周有工作日时归下周,不影响本工作周末日判定
            let next_monday = d + chrono::Duration::days(1);
            if has_workday_in_week_with(next_monday, is_workday) {
                continue;
            }
        }
        return false;
    }
    true
}

/// 今天是否为所在工作周的最后一个工作日(周报触发条件)
fn is_work_week_last_day(today: NaiveDate, cache_root: &PathBuf) -> bool {
    is_work_week_last_day_with(today, &|d| workday::is_workday(d, cache_root))
}

/// 日报的报告日:前一天(次日生成,默认)覆盖前一日全天,
/// 避免执行时间之后的加班提交被漏掉;当天则为触发日本身
fn daily_report_date(fire_date: NaiveDate, previous_day: bool) -> NaiveDate {
    if previous_day {
        fire_date - chrono::Duration::days(1)
    } else {
        fire_date
    }
}

/// 日报星期过滤判定,作用于报告日(触发日前一天)而非触发日当天:
/// 「仅周一至周五 / 仅中国工作日」描述的是哪些天的工作需要出日报
fn daily_filters_allow(
    report_date: NaiveDate,
    weekdays_only: bool,
    chinese_workday_only: bool,
    is_workday: &dyn Fn(NaiveDate) -> bool,
) -> bool {
    if weekdays_only && report_date.weekday().num_days_from_monday() >= 5 {
        return false;
    }
    if chinese_workday_only && !is_workday(report_date) {
        return false;
    }
    true
}

/// 筛选当前时刻应该触发的 schedule(±1 分钟容差)
fn due_schedules(
    schedules: &[ReportSchedule],
    now_local: &chrono::DateTime<Local>,
    cache_root: &PathBuf,
) -> Vec<ReportSchedule> {
    let now_time = now_local.time();
    let today = now_local.date_naive();

    schedules
        .iter()
        .filter(|s| {
            if !s.enabled {
                return false;
            }

            // 时间匹配:±1 分钟容差
            let parts: Vec<&str> = s.time_of_day.split(':').collect();
            if parts.len() != 2 {
                return false;
            }
            let target_h: u32 = parts[0].parse().unwrap_or(99);
            let target_m: u32 = parts[1].parse().unwrap_or(99);
            let diff = (now_time.hour() as i32 * 60 + now_time.minute() as i32)
                - (target_h as i32 * 60 + target_m as i32);
            if diff.abs() > 1 {
                return false;
            }

            // 今天已运行过
            if let Some(last) = s.last_run_at {
                let last_date = chrono::DateTime::from_timestamp(last, 0).map(|dt| dt.date_naive());
                if last_date == Some(today) {
                    return false;
                }
            }

            // 周报:工作周模式→工作周末日触发;自定义模式→结束周几触发
            // (均不使用日报的星期过滤)
            if s.report_type == "weekly" {
                if s.weekly_workweek {
                    return is_work_week_last_day(today, cache_root);
                }
                return today.weekday().number_from_monday() == s.weekly_end_weekday;
            }

            // 日报:报告日 = 触发日前一天(次日生成)或当天;星期过滤按报告日判定
            let report_date = daily_report_date(today, s.previous_day);
            daily_filters_allow(
                report_date,
                s.weekdays_only,
                s.chinese_workday_only,
                &|d| workday::is_workday(d, cache_root),
            )
        })
        .cloned()
        .collect()
}

// ── project path lookup ────────────────────────────────────────────────

/// 读取所有未归档项目的 (id, path, name, description)。
/// 复用 AppHandle 托管的 Db 连接,与主线程共享同一把锁,避免独立连接同文件
/// 的毫秒级竞争窗口。
fn load_project_paths(app: &AppHandle) -> AppResult<HashMap<i64, (String, String, String)>> {
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
fn mark_last_run(conn: &rusqlite::Connection, schedule_id: &str) -> AppResult<()> {
    let now_ts = Local::now().timestamp();
    let updated = conn.execute(
        "UPDATE report_schedules SET last_run_at = ?1 WHERE id = ?2",
        rusqlite::params![now_ts, schedule_id],
    )?;
    if updated != 1 {
        return Err(AppError::coded(
            ErrorCode::DbError,
            format!("scheduler: last_run_at update missing schedule_id={schedule_id} updated={updated}"),
        ));
    }
    Ok(())
}

/// 删除指定 report_history 行;关联的 report_commits 由外键级联删除。
/// 未命中恰好一行时返回错误。
fn delete_report_history_row(conn: &rusqlite::Connection, history_id: i64) -> AppResult<()> {
    let deleted = conn.execute(
        "DELETE FROM report_history WHERE id = ?1",
        rusqlite::params![history_id],
    )?;
    if deleted != 1 {
        return Err(AppError::coded(
            ErrorCode::DbError,
            format!("scheduler: orphan report_history cleanup failed id={history_id} deleted={deleted}"),
        ));
    }
    Ok(())
}

// ── schedule execution ─────────────────────────────────────────────────

/// 报告调度/AI 生成共用的异步客户端:仅设连接超时、不设总超时
/// (LLM 生成耗时不可预估,总超时会切断长响应;此处与 java/account 各自的客户端语义不同)
pub(crate) fn report_http_client() -> Client {
    Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| Client::new())
}

/// 执行一次定时任务:拉取提交 → 调 AI → 写报告历史 → 更新运行时间 → emit 事件。
/// 成功返回报告历史 id;任何失败以 Err 返回。
///
/// 报告历史和提交明细在同一事务中写入。更新运行时间失败时尝试删除已写入的
/// 报告历史,并保留原始更新错误;只有运行时间更新成功后才发送前端事件。
pub(crate) async fn fire_schedule(
    app: &AppHandle,
    client: &Client,
    data_dir: &PathBuf,
    schedule: &ReportSchedule,
) -> AppResult<i64> {
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
        let tag_pids = crate::commands::report::tag_project_ids(&conn, &schedule.tag_ids).map_err(
            |e| {
                eprintln!("[scheduler] 按标签反查项目失败: {e}");
                e
            },
        )?;
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

    // 6. 组装 prompt
    let prompt_data: Vec<(String, String, Vec<GitCommitInfo>)> = commits_by_project
        .iter()
        .map(|(_, name, desc, commits)| (name.clone(), desc.clone(), commits.clone()))
        .collect();
    let user_prompt = build_report_prompt(&prompt_data, &range_label, &language);

    // 7. 调用 AI(失败不写历史、不更新 last_run_at)
    let result = call_ai(client, &ai_config, &system_prompt, &user_prompt)
        .await
        .map_err(|e| {
            eprintln!("[scheduler] {schedule_name}: AI 调用失败: {e}");
            e
        })?;

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

// ── main loop ──────────────────────────────────────────────────────────

/// 从 SQLite 读取全部定时任务(MutexGuard 在函数返回前释放,避免跨 await 持锁)
fn load_schedules(app: &AppHandle) -> AppResult<Vec<ReportSchedule>> {
    let db = app.state::<crate::db::Db>();
    let conn = db.0.lock().unwrap();
    read_schedules(&conn)
}

/// 启动调度器后台循环。
/// 应在 `tauri::Builder::setup` 中通过 `tauri::async_runtime::spawn` 调用。
pub async fn run(app: AppHandle) {
    // data_dir:~/.repomeow(settings.json / prompts);cache_root:安装目录 data/(chinese-days 缓存)
    let data_dir = workday::data_dir(&app);
    let cache_root = workday::cache_root();
    let notify = app
        .state::<crate::commands::report::ScheduleNotify>()
        .0
        .clone();
    let client = report_http_client();

    loop {
        // 重新加载定时任务(从 SQLite 读取;锁在此行结束后立即释放,不跨 await)
        let schedules = match load_schedules(&app) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[scheduler] 读取定时任务失败: {e}");
                time::sleep(IDLE_INTERVAL).await;
                continue;
            }
        };

        let now_local = Local::now();

        // 先检查是否有当前时刻应触发的任务(±1 分钟容差,防止 sleep_until 因重算延迟漏掉)
        let due = due_schedules(&schedules, &now_local, &cache_root);
        for s in &due {
            if let Err(e) = fire_schedule(&app, &client, &data_dir, s).await {
                eprintln!("[scheduler] {}: 执行失败: {e}", s.name);
            }
        }

        // 计算下次触发时刻
        if let Some(deadline) = earliest_fire(&schedules, &now_local) {
            select! {
                _ = time::sleep_until(deadline) => {
                    // 醒来后重新加载 schedules 并检查触发(下一轮循环)
                    continue;
                }
                _ = notify.notified() => {
                    // 定时任务变更,重新计算
                    eprintln!("[scheduler] 定时任务配置已变更,重算触发时间");
                    continue;
                }
            }
        } else {
            // 无启用任务,等待通知或超时
            select! {
                _ = notify.notified() => {
                    // 有新任务加入
                    eprintln!("[scheduler] 收到通知,检查定时任务");
                    continue;
                }
                _ = time::sleep(IDLE_INTERVAL) => {
                    // 定期检查
                    continue;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{default_schedule_name, strip_thinking};

    #[test]
    fn default_schedule_name_follows_ui_language() {
        // 中文(zh-CN 或其他未识别语言):周报/日报
        assert_eq!(default_schedule_name(true, "zh-CN"), "周报定时任务");
        assert_eq!(default_schedule_name(false, "zh-CN"), "日报定时任务");
        assert_eq!(default_schedule_name(true, "ja-JP"), "周报定时任务");
        // 英文(en-US):Weekly / Daily Schedule
        assert_eq!(default_schedule_name(true, "en-US"), "Weekly Schedule");
        assert_eq!(default_schedule_name(false, "en-US"), "Daily Schedule");
    }

    #[test]
    fn strips_leading_think_block() {
        assert_eq!(strip_thinking("<think>推理过程</think>正文"), "正文");
        // 大小写不敏感 + 前导空白
        assert_eq!(strip_thinking("  <THINK>推理</THINK>\n正文"), "正文");
        // 多个连续思考块
        assert_eq!(
            strip_thinking("<think>a</think><think>b</think>正文"),
            "正文"
        );
    }

    #[test]
    fn unclosed_leading_think_yields_empty() {
        assert_eq!(strip_thinking("<think>只有思考没有正文"), "");
    }

    #[test]
    fn keeps_think_mentions_in_body() {
        // 回归:正文(如介绍思考剥离功能的报告)中出现的 <think> 字样必须保留
        let report = "# 日报\n支持成对/未闭合的`<think>`标签块自动剥离\n其余内容";
        assert_eq!(strip_thinking(report), report);
        // 正文中的成对标签同样保留
        let paired = "摘要\n示例:<think>x</think> 是标签";
        assert_eq!(strip_thinking(paired), paired);
    }

    #[test]
    fn plain_text_unchanged() {
        assert_eq!(strip_thinking("  普通报告  "), "普通报告");
    }

    #[test]
    fn daily_report_date_is_previous_day() {
        // 次日生成(默认):8/20 触发 → 报告日 8/19
        assert_eq!(
            super::daily_report_date(chrono::NaiveDate::from_ymd_opt(2026, 8, 20).unwrap(), true),
            chrono::NaiveDate::from_ymd_opt(2026, 8, 19).unwrap()
        );
        // 跨月:8/1 触发 → 报告日 7/31
        assert_eq!(
            super::daily_report_date(chrono::NaiveDate::from_ymd_opt(2026, 8, 1).unwrap(), true),
            chrono::NaiveDate::from_ymd_opt(2026, 7, 31).unwrap()
        );
        // 当天生成:报告日 = 触发日
        let fire = chrono::NaiveDate::from_ymd_opt(2026, 8, 20).unwrap();
        assert_eq!(super::daily_report_date(fire, false), fire);
    }

    #[test]
    fn daily_filters_apply_to_report_date_not_fire_date() {
        use chrono::Datelike;
        let all_days = &|_: chrono::NaiveDate| true;
        // 报告日为周五(触发日周六):「仅周一至周五」仍生成 —— 周五晚加班提交不丢
        let fri = chrono::NaiveDate::from_ymd_opt(2026, 8, 14).unwrap();
        assert_eq!(fri.weekday(), chrono::Weekday::Fri);
        assert!(super::daily_filters_allow(fri, true, false, all_days));
        // 报告日为周六(触发日周日):「仅周一至周五」跳过
        let sat = chrono::NaiveDate::from_ymd_opt(2026, 8, 15).unwrap();
        assert_eq!(sat.weekday(), chrono::Weekday::Sat);
        assert!(!super::daily_filters_allow(sat, true, false, all_days));
        // 「仅中国工作日」:报告日为节假日跳过,为工作日(含调休补班)生成
        let except_fri = &|d: chrono::NaiveDate| d != fri;
        assert!(!super::daily_filters_allow(fri, false, true, except_fri));
        assert!(super::daily_filters_allow(sat, false, true, all_days));
        // 不过滤:任意报告日都生成
        assert!(super::daily_filters_allow(sat, false, false, all_days));
    }

    use crate::commands::report::read_schedules;
    use crate::db;

    fn test_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        conn
    }

    fn insert_schedule(conn: &rusqlite::Connection, id: &str) {
        conn.execute(
            "INSERT INTO report_schedules (id, name, enabled, report_type, project_ids,
                 author_mode, time_of_day, weekdays_only, chinese_workday_only,
                 weekly_workweek, weekly_start_weekday, weekly_end_weekday, last_run_at)
             VALUES (?1, '', 1, 'daily', '[]', 'me', '09:00', 0, 0, 1, 1, 5, NULL)",
            rusqlite::params![id],
        )
        .unwrap();
    }

    #[test]
    fn mark_last_run_writes_timestamp_on_success() {
        // 正常路径:写入 last_run_at,函数返回 Ok
        let conn = test_conn();
        insert_schedule(&conn, "s-ok");

        let result = super::mark_last_run(&conn, "s-ok");
        assert!(result.is_ok(), "正常写入应成功: {result:?}");

        let schedules = read_schedules(&conn).unwrap();
        let s = schedules
            .iter()
            .find(|s| s.id == "s-ok")
            .expect("schedule 存在");
        let ts = s.last_run_at.expect("last_run_at 应被更新");
        let now = chrono::Local::now().timestamp();
        // 允许 ±2s 抖动
        assert!(
            (ts - now).abs() <= 2,
            "last_run_at 应接近当前时间(差值={})",
            ts - now
        );
    }

    #[test]
    fn mark_last_run_returns_error_on_missing_schedule() {
        // 不插入 schedule,确保更新命中零行
        let conn = test_conn();
        let result = super::mark_last_run(&conn, "nonexistent");
        assert!(
            result.is_err(),
            "对不存在的 schedule_id 调用 mark_last_run 应返回 Err"
        );
    }

    #[test]
    fn mark_last_run_no_silent_recovery_on_zero_row_update() {
        // 不插入 schedule,确保更新命中零行
        let conn = test_conn();
        let result = super::mark_last_run(&conn, "missing-schedule");
        assert!(
            result.is_err(),
            "对不存在的 schedule 调用 mark_last_run 应返回 Err(防止 last_run_at 未更新却 emit)"
        );
    }

    #[test]
    fn mark_last_run_persists_timestamp_visible_via_read_schedules() {
        let conn = test_conn();
        insert_schedule(&conn, "s-1");
        let result = super::mark_last_run(&conn, "s-1");
        assert!(result.is_ok());

        let schedules = read_schedules(&conn).unwrap();
        let s = schedules.iter().find(|s| s.id == "s-1").unwrap();
        assert!(s.last_run_at.is_some(), "last_run_at 应被持久化");
    }

    use crate::commands::report::delete_report_history_impl;

    fn insert_history_row(conn: &rusqlite::Connection) -> i64 {
        let now = crate::time_util::now_ts();
        conn.execute(
            "INSERT INTO report_history (project_ids, date_from, date_to, range_label,
                 author_mode, language, period_type, result, created_at)
             VALUES ('[]', '2026-07-01', '2026-07-01', '', 'me', 'zh-CN', 'daily', '', ?1)",
            rusqlite::params![now],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn count_history(conn: &rusqlite::Connection) -> i64 {
        conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM report_history", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn delete_report_history_row_removes_existing_row() {
        let conn = test_conn();
        let id = insert_history_row(&conn);
        assert_eq!(count_history(&conn), 1);

        let res = super::delete_report_history_row(&conn, id);
        assert!(res.is_ok(), "删除存在的行应成功: {res:?}");
        assert_eq!(count_history(&conn), 0, "行应已被删除");
    }

    #[test]
    fn delete_report_history_row_returns_err_for_missing_id() {
        let conn = test_conn();
        let res = super::delete_report_history_row(&conn, 999_999);
        assert!(res.is_err(), "删除不存在的 id 应返回 Err");
    }

    #[test]
    fn delete_report_history_row_via_command_cascades_commits() {
        // 通过命令实现验证 report_commits 的外键级联删除
        let conn = test_conn();
        let history_id = insert_history_row(&conn);
        // 插入一条关联 commit 行,模拟 fire_schedule 写入的子表
        let commits_json =
            r#"[{"hash":"abc","author":"tester","date":"2026-07-01 09:00","subject":"x"}]"#;
        conn.execute(
            "INSERT INTO report_commits (report_id, project_id, project_name,
                 project_description, commit_data)
             VALUES (?1, NULL, '', '', ?2)",
            rusqlite::params![history_id, commits_json],
        )
        .unwrap();
        assert_eq!(count_history(&conn), 1);

        delete_report_history_impl(&conn, history_id).unwrap();
        assert_eq!(count_history(&conn), 0);
        let commit_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM report_commits WHERE report_id = ?1",
                rusqlite::params![history_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(commit_count, 0, "report_commits 应随外键级联删除");
    }
}
