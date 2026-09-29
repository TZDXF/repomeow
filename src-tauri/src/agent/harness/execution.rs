//! 工具执行收尾与 effect gate:对齐 execution/tools.ts、effect-gate.ts。

use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::agent::harness::hooks::HookError;

/// 参数校验通过、可发布 durable intent 并执行的工具调用。
#[derive(Clone, Debug)]
pub struct ClearedToolCall {
    pub tool_call_id: String,
    pub tool_name: String,
    pub args: Value,
}

/// 阶段二原始工具输出。
#[derive(Clone, Debug)]
pub struct ExecutedToolCall {
    pub result: ToolOutput,
    pub is_error: bool,
}

/// after-tool hook 的字段级补丁。
#[derive(Clone, Debug, Default)]
pub struct AfterToolPatch {
    pub content: Option<Vec<Value>>,
    pub details: Option<Value>,
    pub is_error: Option<bool>,
}

/// 工具输出的 provider 内容块与自定义 details。
#[derive(Clone, Debug)]
pub struct ToolOutput {
    pub content: Vec<Value>,
    pub details: Option<Value>,
}

/// 完成收尾、可转为 durable tool-result message 的工具调用。
#[derive(Clone, Debug)]
pub struct FinalizedToolCall {
    pub tool_call_id: String,
    pub tool_name: String,
    pub args: Value,
    pub result: Option<ToolOutput>,
    pub is_error: bool,
}

#[derive(Clone, Debug)]
enum GateState {
    Open,
    Aborting,
    Closed { error: String },
}

struct GateShared {
    state: Mutex<GateState>,
    signal: CancellationToken,
}

/// procedure-facing admission view。
#[derive(Clone)]
pub struct Gate {
    shared: Arc<GateShared>,
}

/// owner-facing lifecycle view。
#[derive(Clone)]
pub struct GateControl {
    shared: Arc<GateShared>,
}

impl Gate {
    pub fn signal(&self) -> CancellationToken {
        self.shared.signal.clone()
    }

    fn check(&self) -> Result<(), HookError> {
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match &*state {
            GateState::Open => Ok(()),
            GateState::Aborting => Err(HookError::aborting()),
            GateState::Closed { error } => Err(HookError::closed(error.clone())),
        }
    }

    pub async fn admit<T, F, Fut>(&self, invoke: F) -> Result<T, HookError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, HookError>>,
    {
        self.check()?;
        invoke().await
    }
}

impl GateControl {
    pub fn begin_abort(&self, cancellation: CancellationToken) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if matches!(&*state, GateState::Open) {
            *state = GateState::Aborting;
            let signal = self.shared.signal.clone();
            tokio::spawn(async move {
                cancellation.cancelled().await;
                signal.cancel();
            });
        }
    }
    pub fn signal_abort(&self) {
        {
            let state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !matches!(&*state, GateState::Aborting) {
                return;
            }
        }
        if !self.shared.signal.is_cancelled() {
            self.shared.signal.cancel();
        }
    }

    pub fn close(&self, error: impl Into<String>) {
        {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if matches!(&*state, GateState::Closed { .. }) {
                return;
            }
            *state = GateState::Closed {
                error: error.into(),
            };
        }
        if !self.shared.signal.is_cancelled() {
            self.shared.signal.cancel();
        }
    }
}

pub fn create_gate() -> (Gate, GateControl) {
    let shared = Arc::new(GateShared {
        state: Mutex::new(GateState::Open),
        signal: CancellationToken::new(),
    });
    (
        Gate {
            shared: shared.clone(),
        },
        GateControl { shared },
    )
}
pub fn finalize_tool_call(
    call: ClearedToolCall,
    executed: ExecutedToolCall,
    patch: Option<AfterToolPatch>,
) -> FinalizedToolCall {
    let (result, is_error) = match patch {
        Some(patch) => {
            let result = ToolOutput {
                content: patch.content.unwrap_or(executed.result.content),
                details: patch.details.or(executed.result.details),
            };
            (result, patch.is_error.unwrap_or(executed.is_error))
        }
        None => (executed.result, executed.is_error),
    };
    FinalizedToolCall {
        tool_call_id: call.tool_call_id,
        tool_name: call.tool_name,
        args: call.args,
        result: Some(result),
        is_error,
    }
}

pub fn create_tool_result_message(call: &FinalizedToolCall) -> Value {
    serde_json::json!({
        "role": "toolResult", "toolCallId": call.tool_call_id, "toolName": call.tool_name,
        "content": call.result.as_ref().map(|result| result.content.clone()).unwrap_or_default(),
        "isError": call.is_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::harness::hooks::HookErrorKind;
    use serde_json::json;

    #[test]
    fn finalize_applies_after_tool_patch() {
        let call = ClearedToolCall {
            tool_call_id: "call-1".into(),
            tool_name: "read".into(),
            args: json!({}),
        };
        let executed = ExecutedToolCall {
            result: ToolOutput {
                content: vec![json!({"type":"text","text":"old"})],
                details: Some(json!({"old":true})),
            },
            is_error: false,
        };
        let patch = AfterToolPatch {
            content: Some(vec![json!({"type":"text","text":"patched"})]),
            details: None,
            is_error: Some(true),
        };
        let finalized = finalize_tool_call(call, executed, Some(patch));
        assert_eq!(
            finalized.result.as_ref().unwrap().content,
            vec![json!({"type":"text","text":"patched"})]
        );
        assert!(finalized.is_error);
    }

    #[test]
    fn creates_canonical_tool_result_json() {
        let call = FinalizedToolCall {
            tool_call_id: "call-1".into(),
            tool_name: "read".into(),
            args: json!({}),
            result: Some(ToolOutput {
                content: vec![json!({"type":"text","text":"ok"})],
                details: None,
            }),
            is_error: false,
        };
        assert_eq!(
            create_tool_result_message(&call),
            json!({"role":"toolResult","toolCallId":"call-1","toolName":"read","content":[{"type":"text","text":"ok"}],"isError":false})
        );
    }

    #[tokio::test]
    async fn gate_admits_then_aborts_and_closes() {
        let (gate, control) = create_gate();
        assert!(gate.admit(|| async { Ok(1) }).await.unwrap() == 1);
        control.begin_abort(CancellationToken::new());
        assert!(
            matches!(gate.admit(|| async { Ok(()) }).await, Err(error) if error.kind == HookErrorKind::GateAborting)
        );
        control.signal_abort();
        assert!(gate.signal().is_cancelled());
        control.close("done");
        assert!(
            matches!(gate.admit(|| async { Ok(()) }).await, Err(error) if error.kind == HookErrorKind::GateClosed)
        );
    }
}
