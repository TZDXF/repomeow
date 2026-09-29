//! harness 统一执行上下文:对齐 `packages/agent/src/harness/context.ts`。
//!
//! TS 侧的 chord `Context` 在本仓库收敛为最小可用载体:以
//! [`CancellationToken`] 承载 abort signal,并允许从当前 context 派生 child。
//! P4 先建立统一形状;metadata/telemetry context 待后续接线时再扩展。

use tokio_util::sync::CancellationToken;

/// 传给工具执行、环境操作与 harness 回调的上下文。
#[derive(Clone, Debug, Default)]
pub struct Context {
    pub abort_signal: Option<CancellationToken>,
}

impl Context {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_signal(signal: CancellationToken) -> Self {
        Self {
            abort_signal: Some(signal),
        }
    }

    /// 对齐 TS `context.abortSignal?.throwIfAborted()` 的探测形态。
    pub fn is_cancelled(&self) -> bool {
        self.abort_signal
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
    }

    /// 派生子 context;父 context 取消时子 token 也会取消。
    pub fn child(&self) -> Self {
        Self {
            abort_signal: self.abort_signal.as_ref().map(|token| token.child_token()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_tracks_child_cancellation() {
        let parent = Context::with_signal(CancellationToken::new());
        let child = parent.child();
        assert!(!parent.is_cancelled());
        assert!(!child.is_cancelled());
        if let Some(token) = parent.abort_signal.as_ref() {
            token.cancel();
        }
        assert!(parent.is_cancelled());
        assert!(child.is_cancelled());
    }

    #[test]
    fn empty_context_has_no_signal() {
        let context = Context::new();
        assert!(context.abort_signal.is_none());
        assert!(!context.is_cancelled());
        assert!(context.child().abort_signal.is_none());
    }
}
