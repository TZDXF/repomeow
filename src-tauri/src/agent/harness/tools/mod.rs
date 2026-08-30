//! 内置执行工具:对齐 `packages/agent/src/harness/tools/`。
//!
//! 各 `create_*` 构造器接收 `Arc<dyn ExecutionEnv>` 并返回 core
//! [`AgentTool`](crate::agent::types::AgentTool)(execute 闭包捕获 env;蓝本的
//! 每回合 context 解析在 WIP harness 中未接线,见报告偏差)。

pub mod bash;
pub mod edit;
pub mod edit_diff;
pub mod file_mutation_queue;
pub mod image;
pub mod path_utils;
pub mod read;
pub mod tool_context;
pub mod write;

pub mod index {
    //! 对齐蓝本 `tools/index.ts` 的 re-export 门面。

    #[allow(unused_imports)]
    pub use super::bash::{create_bash_tool, BashExecution, BashToolDetails, BashToolOptions};
    #[allow(unused_imports)]
    pub use super::edit::{create_edit_tool, EditToolDetails};
    #[allow(unused_imports)]
    pub use super::read::{create_read_tool, ReadToolDetails, ReadToolOptions};
    #[allow(unused_imports)]
    pub use super::tool_context::ExecutionToolContext;
    #[allow(unused_imports)]
    pub use super::write::create_write_tool;
}
