//! `agent`:对 `@earendil-works/pi-agent-core` 的 Rust 复刻。
//! 以 0.87.1 为基线，选择性同步 1.4.0 及 main 的本地适用修复；并非上游全量版本声明。
//!
//! 蓝本仓库:`D:\code\pi`(earendil-works/pi),类型与事件序列化格式保持 camelCase
//! 以兼容 TS 版 JSONL 会话存储与前端消费。模块划分与蓝本对应:
//! - `llm/`:`@earendil-works/pi-ai` 的类型契约子集 + EventStream + 四个 provider
//!   adapter(openai-completions / openai-responses / anthropic-messages / google-generative-ai)
//! - 本层 `types.rs` / `agent_loop.rs` / `agent.rs` / `stream_fn.rs`:core 三件套
//! - `harness/`:packages/agent/src/harness/** 逐文件对齐(session/compaction/tools/...)
//!
//! 有意差异:模型目录/auth/oauth/images 不复刻(Model 由应用 AI 配置构造)，
//! 不迁移已独立为 pi-durable 的新 Harness；保留并维护当前 Rust Harness。

// 完整复刻的 API 面:部分类型/函数(如 harness 内置工具、thinking 内容块、telemetry)
// 对齐蓝本存在,但当前 chat 链路尚未消费,避免大面积 dead_code 噪音。
#![allow(dead_code)]

pub mod harness;
pub mod llm;
pub mod types;

pub mod agent;
pub mod agent_loop;
pub mod chat_tools;
pub mod stream_fn;

/// chat 等调用方约定从本模块根导入 `Agent`。
pub use agent::Agent;
