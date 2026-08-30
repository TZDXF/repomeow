//! `@earendil-works/pi-ai` 的类型契约子集 + EventStream + OpenAI 兼容 provider。
//!
//! 对齐蓝本:`D:\code\pi\packages\ai`。只实现应用需要的 `openai-completions` 一条 API;
//! 其余 API(anthropic-messages/google-generative-ai/bedrock-converse-stream/...)留作扩展点。
//! 所有 serde 命名与 TS 版 JSON 兼容(camelCase / tag 值一致)。

pub mod event_stream;
pub mod openai_completions;
pub mod types;
pub mod validate;

pub use types::*;
