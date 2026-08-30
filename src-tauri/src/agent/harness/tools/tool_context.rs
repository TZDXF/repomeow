//! 工具执行上下文:对齐 `packages/agent/src/harness/tools/tool-context.ts`。

use std::sync::Arc;

use crate::agent::harness::types::{ExecutionEnv, ToolContext};

/// 内置执行工具所需的文件系统与 shell 上下文(对齐 TS `ExecutionToolContext`)。
#[derive(Clone)]
pub struct ExecutionToolContext {
    pub env: Arc<dyn ExecutionEnv>,
}

impl ToolContext for ExecutionToolContext {}

impl std::fmt::Debug for ExecutionToolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutionToolContext")
            .field("cwd", &self.env.cwd())
            .finish()
    }
}

impl ExecutionToolContext {
    /// 从任意 [`ToolContext`] 尝试向下转型(内置工具执行入口用)。
    pub fn from_context<'a>(context: &'a Arc<dyn ToolContext>) -> Option<&'a Self> {
        // ToolContext: Any 超特质的对象安全向下转型(trait upcasting,1.86+)。
        let any: &(dyn std::any::Any + 'static) = context.as_ref();
        any.downcast_ref::<Self>()
    }
}
