use std::collections::{HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use crate::agent::types::{BeforeToolCallHookFn, BeforeToolCallResult};
use crate::ai::catalog::{ChatPermission};
use super::*;


/// ask 权限下执行前需用户硬确认的工具(有副作用:写入 wiki / 自定义命令 /
/// 生成报告 / 修改 wiki 生成配置)。
pub(super) const CONFIRM_REQUIRED_TOOLS: [&str; 5] = [
    "update_wiki",
    "regenerate_wiki",
    "add_custom_command",
    "generate_report",
    "set_wiki_model",
];

/// 确认等待的安全超时:超时按拒绝处理,避免会话永久挂起。
pub(super) const PERMISSION_WAIT_TIMEOUT: Duration = Duration::from_secs(120);

/// 拒绝/超时/取消的稳定英文内部结果(作为 error 工具结果回传模型;
/// 前端用户文案走 i18n)。
pub(super) const PERMISSION_DENIED_REASON: &str = "Tool execution was denied by the user";
pub(super) const PERMISSION_TIMEOUT_REASON: &str = "Tool permission request timed out";
pub(super) const PERMISSION_CANCELLED_REASON: &str = "Tool permission request was cancelled";

/// 一次工具确认的终局。
#[derive(Debug, PartialEq)]
pub(super) enum PermissionDecision {
    /// 放行,继续执行工具。
    Allow,
    /// 拦截(block),携带稳定英文理由。
    Block(&'static str),
}

/// 等待工具确认决策:允许 / 拒绝 / 取消 / 超时(超时按拒绝)。一次性消费
/// receiver;决策到达前 sender 被丢弃(会话清理)视为取消。
pub(super) async fn await_permission_decision(
    receiver: oneshot::Receiver<bool>,
    signal: Option<&CancellationToken>,
    timeout: Duration,
) -> PermissionDecision {
    let cancelled = async {
        match signal {
            Some(signal) => signal.cancelled().await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        result = receiver => match result {
            Ok(true) => PermissionDecision::Allow,
            Ok(false) => PermissionDecision::Block(PERMISSION_DENIED_REASON),
            Err(_) => PermissionDecision::Block(PERMISSION_CANCELLED_REASON),
        },
        _ = cancelled => PermissionDecision::Block(PERMISSION_CANCELLED_REASON),
        _ = tokio::time::sleep(timeout) => PermissionDecision::Block(PERMISSION_TIMEOUT_REASON),
    }
}

/// 投递一次工具确认决策。幂等:未知 id / 已解决 / 重复响应返回 false 且无
/// 任何副作用(绝不触发工具执行)。
pub(super) fn deliver_permission_decision(
    pending: &Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
    tool_call_id: &str,
    allow: bool,
) -> bool {
    let sender = pending.lock().unwrap().remove(tool_call_id);
    match sender {
        Some(sender) => sender.send(allow).is_ok(),
        None => false,
    }
}

/// ask 权限下的 before_tool_call 门禁:命中确认名单时登记一次性决策通道、
/// 推送 `ToolPermissionRequest` 并等待 `chat_tool_permission_respond` 决策。
/// 权限非 Ask 或工具不在名单时直接放行(返回 None)。通用 agent core 不改动。
pub(super) fn build_permission_hook(
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
    prefs: Arc<Mutex<Option<ResolvedPrefs>>>,
    sink: EventSink,
) -> BeforeToolCallHookFn {
    Arc::new(move |context, signal| {
        let pending = pending.clone();
        let prefs = prefs.clone();
        let sink = sink.clone();
        Box::pin(async move {
            let tool_name = context.tool_call.name.as_str();
            let permission = prefs.lock().unwrap().as_ref().map(|prefs| prefs.permission);
            if permission != Some(ChatPermission::Ask)
                || !CONFIRM_REQUIRED_TOOLS.contains(&tool_name)
            {
                return None;
            }
            let id = context.tool_call.id.clone();
            let args = context.args.clone();
            let (sender, receiver) = oneshot::channel();
            {
                let mut map = pending.lock().unwrap();
                // 同 id 重复登记(异常场景):替换旧发送端,旧决策立即失效。
                map.insert(id.clone(), sender);
            }
            sink_send(
                &sink,
                ChatEvent::ToolPermissionRequest {
                    id: id.clone(),
                    name: tool_name.to_string(),
                    args,
                },
            );
            let decision =
                await_permission_decision(receiver, signal.as_ref(), PERMISSION_WAIT_TIMEOUT).await;
            // 决策已定:清除登记,迟到/重复响应幂等失效,绝不导致执行。
            pending.lock().unwrap().remove(&id);
            match decision {
                PermissionDecision::Allow => None,
                PermissionDecision::Block(reason) => Some(BeforeToolCallResult {
                    block: true,
                    reason: Some(reason.to_string()),
                    terminate: false,
                }),
            }
        })
    })
}

