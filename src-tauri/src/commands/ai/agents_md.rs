//! AGENTS.md 生成(项目 AI 资产页「指令与规则」区的「生成/重新生成 AGENTS.md」按钮)。
//! 前端可指定内置 Agent 模型与思考强度(空 = 设置页默认模型 / 模型默认档位);
//! 已存在 AGENTS.md 时允许重新生成,以「内容相对生成前发生变化」判定本轮落盘。
//! 由内置 agent 以读写权限执行:RestrictedEnv 收敛为「项目根只读 + 写仅 AGENTS.md」,
//! shell 一律拒绝;读工具不设调用预算,agent 自由探索仓库(对齐 claude init 的模式:
//! prompt 只给生成指令,上下文由 agent 自行收集,首轮未落盘时给同一会话一次修复机会)。
//! CLAUDE.md 不经 agent,由命令本身对齐为 @AGENTS.md 引用:缺失则创建;
//! 已存在且未引用则把引用行置顶补齐(保留既有内容);已引用则不动。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use tokio_util::sync::CancellationToken;

use crate::agent::harness::restricted_env::RestrictedEnv;
use crate::agent::harness::runtime::harness_tool_from_core;
use crate::agent::harness::tools::index::{
    create_edit_tool, create_find_tool, create_grep_tool, create_ls_tool, create_read_tool,
    create_write_tool,
};
use crate::ai::prompts::{language_name, AGENTS_MD_PROMPT};
use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};

use super::harness_support::{
    builtin_stream_fn, collect_usage_events, create_harness, effective_thinking_level,
    load_builtin_agent_model, prompt_with_timeout, record_collected_usage,
};
use super::run::RegisteredRun;

/// 首轮未落盘时给同一会话一次明确修复机会
const MAX_ATTEMPTS: usize = 2;
/// CLAUDE.md 中对 AGENTS.md 的引用行(Claude Code 的 @ 导入语法)
const CLAUDE_REF: &str = "@AGENTS.md";

/// CLAUDE.md 对齐为 @AGENTS.md 引用;返回动作标识供前端提示。
fn align_claude_md(root: &Path) -> AppResult<&'static str> {
    let path = root.join("CLAUDE.md");
    match fs::read_to_string(&path) {
        Ok(content) if content.contains(CLAUDE_REF) => Ok("unchanged"),
        Ok(content) => {
            // 向上边对齐:引用行置顶,既有内容保留在下方
            fs::write(&path, format!("{CLAUDE_REF}\n\n{content}"))?;
            Ok("aligned")
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::write(&path, format!("{CLAUDE_REF}\n"))?;
            Ok("created")
        }
        Err(error) => Err(error.into()),
    }
}

/// 唯一的 prompt:生成指令(改写自 claude init) + 输出语言。
fn agents_md_prompt(language: &str) -> String {
    format!(
        "{}\n\nWrite the file in {}.",
        AGENTS_MD_PROMPT.trim(),
        language_name(language),
    )
}

// ── 命令 ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateAgentsMdRequest {
    project_path: String,
    language: String,
    /// 取消句柄:前端生成独立 runId,取消时经 ai_cancel_run 置位;缺省表示不可取消。
    #[serde(default)]
    run_id: Option<String>,
    /// 复合值 "providerId/modelId";None = 设置页默认模型
    #[serde(default)]
    model: Option<String>,
    /// chat 思考强度档位;None = 模型默认(推理模型中档,其余关闭)
    #[serde(default)]
    thinking: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateAgentsMdResult {
    /// CLAUDE.md 对齐结果:created(新建)/ aligned(引用置顶补齐)/ unchanged(已引用)
    pub claude_action: String,
}

const REPAIR_PROMPT: &str =
    "The AGENTS.md file was not written. Write the complete file now at the repository root.";

#[tauri::command]
pub async fn ai_generate_agents_md(
    app: AppHandle,
    db: State<'_, Db>,
    request: GenerateAgentsMdRequest,
) -> AppResult<Option<GenerateAgentsMdResult>> {
    let root = PathBuf::from(&request.project_path);
    let target = root.join("AGENTS.md");
    // 重新生成场景:记录生成前内容,以变化判定本轮落盘(旧内容不被误认为产出)
    let initial = fs::read_to_string(&target).unwrap_or_default();
    let run = request
        .run_id
        .as_deref()
        .map(|id| RegisteredRun::new(id.to_string()));
    let cancel = run
        .as_ref()
        .map(|run| run.token.clone())
        .unwrap_or_else(CancellationToken::new);
    let configured = load_builtin_agent_model(&app, request.model.as_deref())?;
    let usage_model = configured.model.id.clone();
    let thinking = effective_thinking_level(&configured.model, request.thinking.as_deref());
    let env = RestrictedEnv::for_agent(&request.project_path, &target)
        .map_err(|error| AppError::coded(ErrorCode::InvalidPath, error.to_string()))?;
    // 读写权限:读工具(read/grep/find/ls)不设预算上限,写仅 AGENTS.md(由 RestrictedEnv 强制)
    let mut tools: Vec<_> = [
        create_read_tool(env.clone(), None),
        create_grep_tool(env.clone()),
        create_find_tool(env.clone()),
        create_ls_tool(env.clone()),
    ]
    .into_iter()
    .map(harness_tool_from_core)
    .collect();
    tools.push(harness_tool_from_core(create_write_tool(env.clone())));
    tools.push(harness_tool_from_core(create_edit_tool(env)));
    let harness = Arc::new(
        create_harness(
            configured.model,
            builtin_stream_fn(configured.api_key, cancel.child_token()),
            tools,
            "You are RepoMeow's built-in coding agent for writing the repository's AGENTS.md."
                .to_string(),
            thinking,
        )
        .await?,
    );
    let usages = collect_usage_events(&harness).await;
    let request_cancel = cancel.child_token();

    let result: AppResult<()> = async {
        let mut prompt = agents_md_prompt(&request.language);
        for attempt in 1..=MAX_ATTEMPTS {
            prompt_with_timeout(harness.clone(), prompt.clone(), &cancel, &request_cancel).await?;
            let written = fs::read_to_string(&target).unwrap_or_default();
            if !written.trim().is_empty() && written != initial {
                return Ok(());
            }
            if attempt < MAX_ATTEMPTS {
                prompt = REPAIR_PROMPT.to_string();
            }
        }
        Err(AppError::coded(
            ErrorCode::AiRequestFailed,
            "AGENTS.md was not written",
        ))
    }
    .await;
    record_collected_usage(&db, "agents-md", &usage_model, &usages.lock().unwrap());
    // 取消不算错误:与 commit/translate 一致返回 None,前端静默收场;已写出的内容保留可见
    if cancel.is_cancelled() {
        return Ok(None);
    }
    result?;
    let claude_action = align_claude_md(&root)?;
    Ok(Some(GenerateAgentsMdResult {
        claude_action: claude_action.to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_project_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "repomeow-agents-md-{tag}-{}-{}",
            std::process::id(),
            crate::time_util::now_ts_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn creates_claude_md_with_reference_only() {
        let dir = temp_project_dir("create");
        assert_eq!(align_claude_md(&dir).unwrap(), "created");
        assert_eq!(
            fs::read_to_string(dir.join("CLAUDE.md")).unwrap(),
            "@AGENTS.md\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prepends_reference_to_existing_claude_md() {
        let dir = temp_project_dir("align");
        fs::write(dir.join("CLAUDE.md"), "# Existing notes\n").unwrap();
        assert_eq!(align_claude_md(&dir).unwrap(), "aligned");
        assert_eq!(
            fs::read_to_string(dir.join("CLAUDE.md")).unwrap(),
            "@AGENTS.md\n\n# Existing notes\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn leaves_referencing_claude_md_untouched() {
        let dir = temp_project_dir("keep");
        fs::write(dir.join("CLAUDE.md"), "See @AGENTS.md for details.\n").unwrap();
        assert_eq!(align_claude_md(&dir).unwrap(), "unchanged");
        assert_eq!(
            fs::read_to_string(dir.join("CLAUDE.md")).unwrap(),
            "See @AGENTS.md for details.\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prompt_is_claude_init_style_instruction_only() {
        assert!(AGENTS_MD_PROMPT.contains("analyze this codebase and create an AGENTS.md file"));
        assert!(AGENTS_MD_PROMPT.contains("Do not create or modify any other file"));
        // 只给生成指令:不含工作模式/预算/预注上下文类约束
        for banned in [
            "at most",
            "Working mode",
            "file tree",
            "README:",
            "Manifest",
        ] {
            assert!(
                !AGENTS_MD_PROMPT.contains(banned),
                "prompt contains {banned}"
            );
        }
        // 语言指令随请求注入
        assert!(agents_md_prompt("zh-CN").contains("Write the file in 中文."));
        assert!(agents_md_prompt("en-US").contains("Write the file in English."));
    }
}
