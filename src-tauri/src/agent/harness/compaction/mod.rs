//! compaction:对齐 `packages/agent/src/harness/compaction/`。

pub mod branch_summarization;
// 子模块与父模块同名是蓝本 `compaction/compaction.ts` 的目录布局,改名会破坏
// 与 TS 逐文件对齐的映射关系(调用方也统一按 `compaction::compaction` 引用)。
#[allow(clippy::module_inception)]
pub mod compaction;
pub mod utils;
