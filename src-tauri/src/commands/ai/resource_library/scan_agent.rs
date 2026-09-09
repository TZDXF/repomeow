//! 安全扫描语义层的内置 Agent:带只读工具(read/grep/find/ls)的
//! AgentHarness,受限环境只读技能目录——大技能无需全量内联,由 Agent
//! 按需补读;静态层与报告解析保持不变。

use std::path::Path;
use std::sync::Arc;

use crate::agent::harness::restricted_env::RestrictedEnv;
use crate::agent::llm::types::{Model, ModelThinkingLevel};
use crate::commands::ai::harness_support::{
    assistant_text, builtin_stream_fn, collect_usage_events, create_harness, prompt_with_timeout,
    read_tools, record_collected_usage,
};
use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};
use tokio_util::sync::CancellationToken;

/// 单次语义分析的工具调用预算(技能目录远小于整仓,与 wiki 大纲同量级)
const READ_BUDGET: usize = 20;

/// 运行一次语义分析:返回最终 assistant 文本(strip thinking 后),
/// 逐 LLM 请求落库 usage(task_type = "scan");cancel 为 None 时不可取消。
pub(super) async fn run_semantic_scan(
    db: &Db,
    model: Model,
    api_key: &str,
    skill_dir: &Path,
    system_prompt: &str,
    user_prompt: &str,
    cancel: Option<&CancellationToken>,
) -> AppResult<String> {
    let never_cancelled = CancellationToken::new();
    let cancel = cancel.unwrap_or(&never_cancelled);
    let stream_fn = builtin_stream_fn(api_key.to_string(), cancel.child_token());
    // 语义层无写需求:允许写目标指向技能目录内一个不存在的占位路径,
    // 受限环境因此事实上只读(写工具也不注册)。
    let env = RestrictedEnv::for_agent(skill_dir, skill_dir.join(".scan-no-write"))
        .map_err(|error| AppError::coded(ErrorCode::InvalidPath, error.to_string()))?;
    let harness = Arc::new(
        create_harness(
            model.clone(),
            stream_fn,
            read_tools(env, READ_BUDGET),
            system_prompt.to_string(),
            ModelThinkingLevel::Off,
        )
        .await?,
    );
    let usages = collect_usage_events(&harness).await;
    let request_cancel = cancel.child_token();
    let result = prompt_with_timeout(
        harness.clone(),
        user_prompt.to_string(),
        cancel,
        &request_cancel,
    )
    .await;
    record_collected_usage(db, "scan", &model.id, &usages.lock().unwrap());
    let message = result?;
    Ok(crate::ai::sdk::strip_thinking(&assistant_text(&message)))
}
