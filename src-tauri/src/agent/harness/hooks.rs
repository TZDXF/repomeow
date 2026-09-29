//! harness 生命周期 hooks:对齐 `packages/agent/src/harness/hooks.ts` 的核心
//! 注册/聚合语义。P4 采用 owned 输入而非 TS 的引用回调,使 `BoxFuture<'static>`
//! 可以安全执行;hook 按注册顺序串行聚合。

use std::sync::Arc;

use futures::future::BoxFuture;

use crate::agent::harness::context::Context;
use crate::agent::llm::types::AssistantMessage;
use crate::agent::types::AgentMessage;

/// 工具执行前回调;返回 `false` 表示阻断本次工具执行。
pub type ToolExecuteHook = Arc<
    dyn Fn(String, String, serde_json::Value, Context) -> BoxFuture<'static, bool>
        + Send
        + Sync,
>;

/// 工具完成后回调;只允许观察结果,不修改 transcript。
pub type ToolResultHook = Arc<
    dyn Fn(String, String, serde_json::Value, Context) -> BoxFuture<'static, ()> + Send + Sync,
>;

/// LLM 请求前回调;返回替换后的消息列表(相当于 TS 的 in-place patch)。
pub type BeforeRequestHook =
    Arc<dyn Fn(Vec<AgentMessage>, Context) -> BoxFuture<'static, Vec<AgentMessage>> + Send + Sync>;

/// 收到 assistant 响应后的观察回调。
pub type AfterResponseHook =
    Arc<dyn Fn(AssistantMessage, Context) -> BoxFuture<'static, ()> + Send + Sync>;

/// harness hook 集合,按 TS `HookRegistry` 的四类高频生命周期收敛。
#[derive(Default)]
pub struct HarnessHooks {
    pub before_tool_execute: Vec<ToolExecuteHook>,
    pub after_tool_result: Vec<ToolResultHook>,
    pub before_request: Vec<BeforeRequestHook>,
    pub after_response: Vec<AfterResponseHook>,
}

impl HarnessHooks {
    pub fn is_empty(&self) -> bool {
        self.before_tool_execute.is_empty()
            && self.after_tool_result.is_empty()
            && self.before_request.is_empty()
            && self.after_response.is_empty()
    }

    /// 串行执行 before-tool hooks;任一 hook 返回 false 即阻断后续 hook。
    pub async fn run_before_tool_execute(
        hooks: &Self,
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
        context: Context,
    ) -> bool {
        for hook in &hooks.before_tool_execute {
            if !hook(
                tool_call_id.clone(),
                tool_name.clone(),
                args.clone(),
                context.clone(),
            )
            .await
            {
                return false;
            }
        }
        true
    }

    /// 按注册顺序通知 after-tool hooks。
    pub async fn run_after_tool_result(
        hooks: &Self,
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
        context: Context,
    ) {
        for hook in &hooks.after_tool_result {
            hook(
                tool_call_id.clone(),
                tool_name.clone(),
                args.clone(),
                context.clone(),
            )
            .await;
        }
    }

    /// 请求消息依次穿过 before-request hooks。
    pub async fn run_before_request(
        hooks: &Self,
        mut messages: Vec<AgentMessage>,
        context: Context,
    ) -> Vec<AgentMessage> {
        for hook in &hooks.before_request {
            messages = hook(messages, context.clone()).await;
        }
        messages
    }

    /// 按注册顺序通知 after-response hooks。
    pub async fn run_after_response(
        hooks: &Self,
        response: AssistantMessage,
        context: Context,
    ) {
        for hook in &hooks.after_response {
            hook(response.clone(), context.clone()).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[tokio::test]
    async fn runs_before_tool_hooks_until_blocked() {
        let calls = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let mut hooks = HarnessHooks::default();
        let first_calls = calls.clone();
        hooks.before_tool_execute.push(Arc::new(move |_, _, _, _| {
            let calls = first_calls.clone();
            Box::pin(async move {
                calls.lock().unwrap().push("first");
                true
            })
        }));
        let second_calls = calls.clone();
        hooks.before_tool_execute.push(Arc::new(move |_, _, _, _| {
            let calls = second_calls.clone();
            Box::pin(async move {
                calls.lock().unwrap().push("second");
                false
            })
        }));
        let third_calls = calls.clone();
        hooks.before_tool_execute.push(Arc::new(move |_, _, _, _| {
            let calls = third_calls.clone();
            Box::pin(async move {
                calls.lock().unwrap().push("third");
                true
            })
        }));

        let allowed = HarnessHooks::run_before_tool_execute(
            &hooks,
            "call-1".to_string(),
            "read".to_string(),
            serde_json::json!({}),
            Context::new(),
        )
        .await;

        assert!(!allowed);
        assert_eq!(*calls.lock().unwrap(), vec!["first", "second"]);
    }

    #[tokio::test]
    async fn chains_before_request_hooks() {
        let mut hooks = HarnessHooks::default();
        hooks.before_request.push(Arc::new(|mut messages, _| {
            Box::pin(async move {
                messages.push(AgentMessage::Custom("first".to_string().into()));
                messages
            })
        }));
        hooks.before_request.push(Arc::new(|mut messages, _| {
            Box::pin(async move {
                messages.push(AgentMessage::Custom("second".to_string().into()));
                messages
            })
        }));

        let messages = HarnessHooks::run_before_request(&hooks, Vec::new(), Context::new()).await;
        assert_eq!(messages.len(), 2);
    }

    #[tokio::test]
    async fn empty_hooks_are_noop() {
        let hooks = HarnessHooks::default();
        assert!(hooks.is_empty());
        let messages = HarnessHooks::run_before_request(&hooks, Vec::new(), Context::new()).await;
        assert!(messages.is_empty());
        assert!(HarnessHooks::run_before_tool_execute(
            &hooks,
            "call".to_string(),
            "tool".to_string(),
            serde_json::Value::Null,
            Context::new()
        )
        .await);
    }
}
