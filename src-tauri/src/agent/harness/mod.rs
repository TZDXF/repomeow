//! harness:packages/agent `src/harness/**` 的 Rust 复刻(session/compaction/tools/
//! reducer/skills/prompt-templates/telemetry/执行环境)。
//!
//! 复刻边界与蓝本一致:`AgentHarness` 组合层在上游即为 WIP(运行方法全部
//! NotImplemented),此处同样对齐;完整实现的组件逐文件对齐移植。
//!
//! 蓝本 → Rust 对应:
//! - `types.rs`    ← harness/types.ts(Result 基础件 + FileSystem/Shell/ExecutionEnv)
//! - `result.rs`   ← harness/result.ts(TaggedError 组合器,thiserror 实现)
//! - `errors.rs`   ← agent-harness.ts 顶部的 harness 专用错误族
//! - `messages.rs` ← harness/messages.ts(自定义消息 + convertToLlm)
//! - `events.rs`   ← harness/events.ts
//! - `system_prompt.rs` ← harness/system-prompt.ts
//! - `skills.rs`   ← harness/skills.ts
//! - `prompt_templates.rs` ← harness/prompt-templates.ts
//! - `reducer.rs`  ← harness/reducer.ts
//! - `telemetry.rs` ← harness/telemetry.ts(trait + 内存实现,不接 OTel)
//! - `session/`    ← harness/session/**(types/state/memory/context/session/jsonl)
//! - `compaction/` ← harness/compaction/**(compaction/branch_summarization/utils)
//! - `tools/`      ← harness/tools/**
//! - `utils/`      ← harness/utils/**(truncate/shell-output)
//! - `env.rs`      ← harness/env/nodejs.ts(TokioEnv,tokio::fs + tokio::process)
//! - `agent_harness.rs` ← harness/agent-harness.ts(公开契约;运行方法已接线)
//! - `runtime.rs`   ← 本仓库扩展:AgentHarness 运行时(prompt/abort/队列/事件)
//! - `uuid.rs`     ← 蓝本 `@earendil-works/pi-ai` 的 uuidv7(蓝本依赖,本地补实现)

// 说明:`agent` 模块当前是 crate 内部消费(harness 的公开 API 面尚未被上层
// 接线,与蓝本 WIP 状态一致),dead_code 允许避免对刻意保留的契约面报警。
#[allow(dead_code)]
pub mod agent_harness;
#[allow(dead_code)]
pub mod compaction;
#[allow(dead_code)]
pub mod env;
#[allow(dead_code)]
pub mod errors;
#[allow(dead_code)]
pub mod events;
#[allow(dead_code)]
pub mod messages;
#[allow(dead_code)]
pub mod prompt_templates;
#[allow(dead_code)]
pub mod reducer;
#[allow(dead_code)]
pub mod runtime;
#[allow(dead_code)]
pub mod result;
#[allow(dead_code)]
pub mod session;
#[allow(dead_code)]
pub mod skills;
#[allow(dead_code)]
pub mod system_prompt;
#[allow(dead_code)]
pub mod telemetry;
#[allow(dead_code)]
pub mod tools;
#[allow(dead_code)]
pub mod types;
#[allow(dead_code)]
pub mod utils;
#[allow(dead_code)]
pub mod uuid;

#[allow(unused_imports)]
pub use agent_harness::AgentHarness;
