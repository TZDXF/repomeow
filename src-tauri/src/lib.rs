mod agent;
mod ai;
mod background_task;
mod commands;
mod db;
mod error;
pub mod mcp;
mod models;
mod path_util;
mod scheduler;
mod time_util;
mod tray;
mod workday;

use std::sync::Arc;

use db::Db;
use tauri::Manager;
use tokio::sync::Notify;

/// 应用数据目录名(位于用户主目录下)
pub(crate) const APP_DATA_DIR_NAME: &str = ".repomeow";

/// RepoMeow 持久化数据根目录。AI、提示词、Wiki 等后端模块统一从这里派生路径。
pub(crate) fn app_data_dir(app: &tauri::AppHandle) -> error::AppResult<std::path::PathBuf> {
    let home = app
        .path()
        .home_dir()
        .map_err(|e| error::AppError::coded(error::ErrorCode::IoError, e.to_string()))?;
    Ok(home.join(APP_DATA_DIR_NAME))
}

/// 运行期缓存根目录:安装目录(exe 所在目录)下的 `data/` 子目录,
/// 用于编辑器图标提取产物(icons/)与 chinese-days 缓存等可再生的运行期文件。
/// 安装目录不可写时缓存写入静默失败,功能按既有降级语义处理
/// (图标回退 lucide 通用图标、chinese-days 每次重新拉取)。
/// dev 模式(tauri dev)下 exe 位于 target/debug/,缓存随之落在 target/debug/data/。
pub(crate) fn runtime_data_root() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("data")))
        .unwrap_or_default()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // 单实例:必须最先注册,第二次启动时聚焦已有窗口并退出新进程
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        // 开机自启(Windows 注册表 Run 项 / macOS LaunchAgent,由设置页开关控制);
        // 自启时附带 --autostart 参数,用于静默启动(只驻留托盘、不弹主窗口)
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .args(["--autostart"])
                .build(),
        )
        .setup(|app| {
            // 数据库文件: ~/.repomeow/projects.db
            // (Windows: C:\Users\<user>\.repomeow\projects.db)
            let dir = app.path().home_dir()?.join(APP_DATA_DIR_NAME);
            let db = Db::open(&dir.join("projects.db"))?;
            // 清洗入库归一化前的存量登记路径(统一分隔符风格,消除同目录重复登记)
            commands::project::normalize_stored_paths(&db.0.lock().unwrap());
            // AI 用量日志保留期清理(190 天前)
            commands::usage::prune_old_entries(&db.0.lock().unwrap());
            app.manage(db);

            // 调度通知:用于定时任务变更时唤醒后台 scheduler
            let notify = Arc::new(Notify::new());
            app.manage(commands::report::ScheduleNotify(notify));
            let git_monitor_notify = Arc::new(Notify::new());
            app.manage(commands::git::GitMonitorNotify(git_monitor_notify));

            // 系统托盘(图标 + 迷你项目列表弹窗)
            tray::setup(app)?;

            // 主窗口配置为初始不可见(visible: false):正常启动时在这里统一显示;
            // 开机自启(带 --autostart 参数)保持隐藏,静默驻留托盘
            let silent_start = std::env::args().any(|arg| arg == "--autostart");
            if !silent_start {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }

            // 启动日报/周报定时调度器(后台 tokio 任务,仅 App 运行时生效)
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                scheduler::run(handle).await;
            });

            // 统一 Git 检查循环:本地状态、fetch、自动快进与事件发布共用同一入口
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                commands::git::monitor_loop(handle).await;
            });

            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                if window.label() == tray::TRAY_POPUP_LABEL {
                    // 迷你弹窗永不真正关闭,只隐藏
                    api.prevent_close();
                    let _ = window.hide();
                } else if window.label() == tray::MAIN_WINDOW_LABEL {
                    // 关闭主窗口:按设置项决定最小化到托盘还是直接退出(默认托盘)
                    let action = tray::read_setting_string(window.app_handle(), "closeAction")
                        .unwrap_or_else(|| "tray".to_string());
                    if action == "exit" {
                        // 迷你弹窗(隐藏)也属于窗口,会阻止"全部窗口关闭即退出"的默认行为,
                        // 因此直接退出整个应用
                        window.app_handle().exit(0);
                    } else {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                }
            }
            tauri::WindowEvent::Focused(false) => {
                // 迷你弹窗失焦自动收起(类似 JetBrains Toolbox);
                // 显示瞬间的焦点迁移(WebView2 子窗口夺取键盘焦点)不算真正失焦
                if window.label() == tray::TRAY_POPUP_LABEL && !tray::should_ignore_popup_blur() {
                    let _ = window.hide();
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::project::add_project,
            commands::project::list_projects,
            commands::project::get_project,
            commands::project::update_project,
            commands::project::update_project_path,
            commands::project::move_project_dir,
            commands::project::archive_project,
            commands::project::list_archived_projects,
            commands::project::unarchive_project,
            commands::project::set_project_favorite,
            commands::project::set_project_auto_pull,
            commands::project::set_project_wiki_auto_update,
            commands::project::delete_project,
            commands::git::check_git_status,
            commands::git::list_git_remotes,
            commands::git::list_git_branches,
            commands::git::list_git_stashes,
            commands::git::git_stash_files,
            commands::git::git_stash_file_diff,
            commands::git::git_init,
            commands::git::git_checkout,
            commands::git::git_commit,
            commands::git::git_pull,
            commands::git::git_push,
            commands::git::git_branch_delete,
            commands::git::git_remote_branch_delete,
            commands::git::git_stash_push,
            commands::git::git_stash_pop,
            commands::git::git_stash_drop,
            commands::git::git_commit_context,
            commands::git::git_log,
            commands::git::git_graph_log,
            commands::git::git_project_stats,
            commands::git::git_commit_files,
            commands::git::git_commit_file_diff,
            commands::git::git_commit_file_blob,
            commands::git::git_worktree_files,
            commands::git::git_worktree_file_diff,
            commands::git::git_current_user,
            commands::git::git_clone,
            commands::git::cancel_git_clone,
            commands::git::list_project_remote_urls,
            commands::git::list_git_worktrees,
            commands::git::git_worktree_add,
            commands::git::git_worktree_remove,
            commands::git::git_merge,
            commands::git::git_merge_abort,
            commands::git::git_rebase,
            commands::git::git_rebase_abort,
            commands::semantic::semantic_status,
            commands::semantic::semantic_cancel,
            commands::semantic::semantic_file_entities,
            commands::semantic::semantic_find_entities,
            commands::semantic::semantic_entity_callers,
            commands::semantic::semantic_entity_refs,
            commands::semantic::semantic_entity_impact,
            commands::semantic::semantic_file_blame,
            commands::semantic::semantic_entity_log,
            commands::semantic::semantic_worktree_diff,
            commands::semantic::semantic_entity_context,
            commands::account::list_git_accounts,
            commands::account::add_git_account,
            commands::account::update_git_account,
            commands::account::remove_git_account,
            commands::account::list_account_repos,
            commands::account::get_gh_cli_account,
            commands::open::open_with,
            commands::open::open_with_custom_command,
            commands::open::open_in_editor,
            commands::open::detect_editors,
            commands::open::detect_terminal_capabilities,
            commands::editor_icon::get_editor_icons,
            commands::window::show_main_window,
            commands::window::hide_tray_popup,
            commands::mcp::get_mcp_server_info,
            commands::prompt::get_ai_prompts,
            commands::prompt::get_default_ai_prompts,
            commands::prompt::set_ai_prompts,
            commands::prompt::open_prompts_dir,
            commands::tag::list_tags,
            commands::tag::create_tag,
            commands::tag::update_tag,
            commands::tag::delete_tag,
            commands::tag::set_project_tags,
            commands::scan::scan_project_assets,
            commands::overview::get_project_overview,
            commands::script::create_custom_command,
            commands::script::update_custom_command,
            commands::script::delete_custom_command,
            commands::script::run_in_terminal,
            commands::files::save_text_file,
            commands::files::list_project_files,
            commands::files::search_project_files,
            commands::files::read_file_preview,
            commands::files::search_project_text,
            commands::docker::compose_ps_batch,
            commands::docker::compose_export,
            commands::java::detect_jdks,
            commands::java::check_jdk,
            commands::java::list_remote_jdks,
            commands::java::install_jdk,
            commands::toolchain::detect_toolchains,
            commands::toolchain::toolchain_op,
            commands::toolchain::list_toolchain_versions,
            commands::hidden::set_hidden_item,
            commands::pin::list_pinned_commands,
            commands::pin::set_pinned_command,
            commands::report::list_report_history,
            commands::report::get_report_history,
            commands::report::delete_report_history,
            commands::report::get_calendar_meta,
            commands::report::get_holiday_data,
            commands::report::get_reports_by_date,
            commands::report::get_reports_by_range,
            commands::report::get_work_week_ranges,
            commands::report::plan_batch_report_ranges,
            commands::report::list_report_dates,
            commands::report::list_report_schedules,
            commands::report::save_report_schedules,
            commands::report::run_report_schedule_now,
            commands::git::list_system_schedules,
            commands::git::save_system_schedule,
            commands::wiki::get_wiki_dir,
            commands::wiki::load_wiki_config,
            commands::wiki::save_wiki_config,
            commands::wiki::load_wiki,
            commands::wiki::has_wiki,
            commands::wiki::delete_wiki,
            commands::wiki::open_wiki_dir,
            commands::agent::agent_list,
            commands::agent::conflict::resolve_git_conflicts_with_agent,
            commands::agent::acp_start,
            commands::agent::acp_prompt,
            commands::agent::acp_cancel,
            commands::agent::acp_test,
            commands::ai::ai_config_get,
            commands::ai::ai_config_save,
            commands::ai::ai_config_reveal,
            commands::ai::ai_config_builtin_providers,
            commands::ai::ai_cc_switch_providers,
            commands::ai::ai_cc_switch_assets,
            commands::ai::scan_project_ai_assets,
            commands::ai::set_project_cc_skill,
            commands::ai::set_project_cc_mcp,
            commands::ai::create_project_skill,
            commands::ai::delete_project_skill,
            commands::ai::set_project_mcp_server,
            commands::ai::remove_project_mcp_server,
            commands::ai::ai_list_models,
            commands::ai::ai_test_connection,
            commands::ai::ai_translate_markdown,
            commands::ai::ai_generate_commit_message,
            commands::ai::ai_generate_and_save_report,
            commands::ai::ai_generate_batch_reports,
            commands::ai::ai_generate_wiki,
            commands::ai::ai_regenerate_wiki_page,
            commands::ai::ai_update_wiki,
            commands::ai::ai_cancel_run,
            commands::usage::get_ai_usage_summary,
            commands::usage::list_ai_usage_log,
            commands::usage::clear_ai_usage_log,
            commands::chat::chat_send,
            commands::chat::chat_abort,
            commands::chat::chat_new_session,
            commands::chat::chat_tool_permission_respond,
            commands::chat::chat_truncate_last_turn,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, event| {
            // 事件循环退出前(正常关闭 / 退出到托盘 / 系统关机销毁窗口):
            // 杀掉所有仍在运行的 git/agent/sem 子进程,避免其成为孤儿
            if let tauri::RunEvent::Exit = event {
                commands::git::cleanup_on_exit();
                commands::agent::cleanup_on_exit();
                commands::semantic::cleanup_on_exit();
            }
        });
}
