//! telemetry:对齐 `packages/agent/src/harness/telemetry.ts` 的接口形态。
//!
//! 蓝本依赖 pi-telemetry 的 schema 类型系统(TS 泛型推导);Rust 复刻保留:
//! - [`TelemetryContext`] / [`TelemetrySpan`] trait(startSpan + 属性/事件/状态),
//! - [`InMemoryTelemetryContext`] 内存实现(测试/调试,span 结束后落记录),
//! - [`NoopTelemetryContext`] NOOP 常量,
//! - AI 与 Harness 的 span 名称/属性定义(蓝本 schema 的 JSON 表,名称与键一致),
//! - `start_ai_span` / `start_harness_span` 泛型辅助(span Drop 即结束,对齐
//!   蓝本 callback 返回后 span 收尾)。
//! 不接 OTel(与蓝本 pi-telemetry SDK 的对接留待接入方)。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// 属性值(对齐 TS `AttributeValue`:string | number | boolean | 数组)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttributeValue {
    Text(String),
    Number(f64),
    Integer(i64),
    Boolean(bool),
    Array(Vec<Value>),
}

impl From<&str> for AttributeValue {
    fn from(value: &str) -> Self {
        AttributeValue::Text(value.to_string())
    }
}

impl From<String> for AttributeValue {
    fn from(value: String) -> Self {
        AttributeValue::Text(value)
    }
}

impl From<i64> for AttributeValue {
    fn from(value: i64) -> Self {
        AttributeValue::Integer(value)
    }
}

impl From<f64> for AttributeValue {
    fn from(value: f64) -> Self {
        AttributeValue::Number(value)
    }
}

impl From<bool> for AttributeValue {
    fn from(value: bool) -> Self {
        AttributeValue::Boolean(value)
    }
}

/// span 状态(对齐 TS `SpanStatus`)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SpanStatus {
    Unset,
    Ok,
    Error,
}

/// span 选项(对齐 TS `SpanOptions { name, attributes }`)。
#[derive(Clone, Debug, Default)]
pub struct SpanOptions {
    pub name: String,
    pub attributes: Vec<(String, AttributeValue)>,
}

/// 已结束 span 的记录(InMemory 实现用)。
#[derive(Clone, Debug)]
pub struct RecordedSpan {
    pub name: String,
    pub attributes: HashMap<String, AttributeValue>,
    pub events: Vec<RecordedSpanEvent>,
    pub status: SpanStatus,
    pub start_ms: i64,
    pub end_ms: i64,
}

/// 已记录的 span 事件。
#[derive(Clone, Debug)]
pub struct RecordedSpanEvent {
    pub name: String,
    pub attributes: HashMap<String, AttributeValue>,
}

/// telemetry span(对象安全;Drop 即视为结束,状态缺省 ok)。
pub trait TelemetrySpan: Send + Sync {
    fn name(&self) -> &str;

    /// 设置/覆盖属性。
    fn set_attribute(&self, key: &str, value: AttributeValue);

    /// 追加 span 事件。
    fn add_event(&self, name: &str, attributes: Vec<(String, AttributeValue)>);

    /// 记录错误(等价 setAttribute("pi.error.*") + set_status(Error) 的便捷入口)。
    fn record_error(&self, message: &str) {
        self.add_event(
            "exception",
            vec![(
                "exception.message".to_string(),
                AttributeValue::from(message),
            )],
        );
        self.set_status(SpanStatus::Error);
    }

    /// 设置结束状态。
    fn set_status(&self, status: SpanStatus);
}

/// telemetry 上下文(对齐 TS `TelemetryContext.startSpan`)。
pub trait TelemetryContext: Send + Sync {
    /// 创建并启动一个 span;span 结束由 Drop 语义承担(蓝本为 callback 返回)。
    fn start_span(&self, options: SpanOptions) -> Arc<dyn TelemetrySpan>;
}

// ---------------------------------------------------------------------------
// InMemory 实现
// ---------------------------------------------------------------------------

#[derive(Default)]
struct InMemoryState {
    spans: Mutex<Vec<RecordedSpan>>,
    counter: AtomicU64,
}

struct InMemorySpan {
    name: String,
    start_ms: i64,
    attributes: Mutex<HashMap<String, AttributeValue>>,
    events: Mutex<Vec<RecordedSpanEvent>>,
    status: Mutex<SpanStatus>,
    state: Arc<InMemoryState>,
}

impl TelemetrySpan for InMemorySpan {
    fn name(&self) -> &str {
        &self.name
    }

    fn set_attribute(&self, key: &str, value: AttributeValue) {
        self.attributes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key.to_string(), value);
    }

    fn add_event(&self, name: &str, attributes: Vec<(String, AttributeValue)>) {
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(RecordedSpanEvent {
                name: name.to_string(),
                attributes: attributes.into_iter().collect(),
            });
    }

    fn set_status(&self, status: SpanStatus) {
        *self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = status;
    }
}

impl Drop for InMemorySpan {
    fn drop(&mut self) {
        let end_ms = crate::agent::agent_loop::now_ms();
        let attributes = std::mem::take(
            &mut *self
                .attributes
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        let events = std::mem::take(
            &mut *self
                .events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        let status = *self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.state
            .spans
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(RecordedSpan {
                name: self.name.clone(),
                attributes,
                events,
                status,
                start_ms: self.start_ms,
                end_ms,
            });
    }
}

/// 内存 telemetry 上下文:记录所有已结束 span(按结束顺序)。
#[derive(Default)]
pub struct InMemoryTelemetryContext {
    state: Arc<InMemoryState>,
}

impl InMemoryTelemetryContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// 已结束 span 记录(快照)。
    pub fn recorded_spans(&self) -> Vec<RecordedSpan> {
        self.state
            .spans
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// 清空记录。
    pub fn clear(&self) {
        self.state
            .spans
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }
}

impl TelemetryContext for InMemoryTelemetryContext {
    fn start_span(&self, options: SpanOptions) -> Arc<dyn TelemetrySpan> {
        let mut attributes: HashMap<String, AttributeValue> =
            options.attributes.into_iter().collect();
        let sequence = self.state.counter.fetch_add(1, Ordering::SeqCst);
        attributes.insert(
            "pi.span.sequence".to_string(),
            AttributeValue::Integer(sequence as i64),
        );
        Arc::new(InMemorySpan {
            name: options.name,
            start_ms: crate::agent::agent_loop::now_ms(),
            attributes: Mutex::new(attributes),
            events: Mutex::new(Vec::new()),
            status: Mutex::new(SpanStatus::Ok),
            state: self.state.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// NOOP
// ---------------------------------------------------------------------------

/// NOOP span。
pub struct NoopTelemetrySpan;

impl TelemetrySpan for NoopTelemetrySpan {
    fn name(&self) -> &str {
        "noop"
    }

    fn set_attribute(&self, _key: &str, _value: AttributeValue) {}

    fn add_event(&self, _name: &str, _attributes: Vec<(String, AttributeValue)>) {}

    fn set_status(&self, _status: SpanStatus) {}
}

/// NOOP telemetry 上下文(蓝本 `NOOP_TELEMETRY_CONTEXT`)。
pub struct NoopTelemetryContext;

impl TelemetryContext for NoopTelemetryContext {
    fn start_span(&self, _options: SpanOptions) -> Arc<dyn TelemetrySpan> {
        Arc::new(NoopTelemetrySpan)
    }
}

/// NOOP 上下文常量(`NOOP`)。
pub const NOOP: NoopTelemetryContext = NoopTelemetryContext;

// ---------------------------------------------------------------------------
// AI / Harness span 定义(蓝本 AI_TELEMETRY_SCHEMA / HARNESS_TELEMETRY_SCHEMA)
// ---------------------------------------------------------------------------

/// AI 遥测 schema(JSON 表;键与蓝本一致,供接入方校验/文档使用)。
/// (json! 不支持 const,改为函数;蓝本为 const 对象。)
pub fn ai_telemetry_schema() -> Value {
    json!({
        "version": 1,
        "spans": {
            "pi.ai.request": {
                "description": "One logical request to an AI provider",
                "parents": { "kind": "any" },
                "startAttributes": {
                    "pi.ai.operation": { "type": "string", "required": true, "values": ["stream", "fetch_deferred", "cancel_deferred", "generate_images"] },
                    "pi.ai.provider": { "type": "string", "required": true },
                    "pi.ai.model": { "type": "string", "required": true },
                    "pi.ai.api": { "type": "string", "required": true },
                    "pi.ai.streaming": { "type": "boolean", "required": true },
                    "pi.ai.deferred": { "type": "boolean", "required": false }
                },
                "endAttributes": {
                    "pi.ai.response.model": { "type": "string" },
                    "pi.ai.response.id": { "type": "string", "cardinality": "high" },
                    "pi.ai.response.stop_reason": { "type": "string", "values": ["stop", "length", "tool_use", "error", "aborted", "deferred"] },
                    "pi.ai.http.status_code": { "type": "number" },
                    "pi.ai.usage.input_tokens": { "type": "number" },
                    "pi.ai.usage.output_tokens": { "type": "number" },
                    "pi.ai.usage.cache_read_tokens": { "type": "number" },
                    "pi.ai.usage.cache_write_tokens": { "type": "number" },
                    "pi.ai.usage.reasoning_tokens": { "type": "number" },
                    "pi.ai.usage.total_tokens": { "type": "number" },
                    "pi.ai.usage.cost": { "type": "number" },
                    "pi.ai.stream.chunk_count": { "type": "number" },
                    "pi.ai.stream.time_to_first_chunk_ms": { "type": "number" },
                    "pi.ai.error.type": { "type": "string", "cardinality": "low" }
                },
                "status": { "default": "ok" }
            }
        }
    })
}

/// harness 钩子名(蓝本 HOOK_NAMES)。
pub const HOOK_NAMES: [&str; 11] = [
    "before_run",
    "before_resume",
    "before_run_end",
    "transform_context",
    "before_request",
    "before_payload",
    "after_response",
    "before_tool",
    "after_tool",
    "before_compaction",
    "before_navigation",
];

/// harness 事件类型名(蓝本 EVENT_TYPES)。
pub const EVENT_TYPES: [&str; 29] = [
    "run_start",
    "run_resume",
    "run_suspend",
    "run_abort",
    "run_end",
    "fault",
    "handler_error",
    "turn_start",
    "turn_end",
    "retry_scheduled",
    "retry_start",
    "retry_end",
    "message_start",
    "message_update",
    "message_end",
    "tool_start",
    "tool_update",
    "tool_end",
    "entry_added",
    "write_pending",
    "queue_update",
    "fact_update",
    "config_update",
    "compaction_start",
    "compaction_end",
    "navigation_start",
    "navigation_end",
    "lane_created",
    "usage",
];

/// harness 遥测 schema(JSON 表;键与蓝本一致)。
pub fn harness_telemetry_schema() -> Value {
    json!({
        "version": 1,
        "spans": {
            "pi.harness.run": {
                "parents": { "kind": "root_or_external" },
                "startAttributes": {
                    "pi.session.id": { "type": "string", "required": true },
                    "pi.lane.name": { "type": "string", "required": true },
                    "pi.operation.id": { "type": "string", "required": true },
                    "pi.operation.recovery": { "type": "boolean", "required": true },
                    "pi.operation.kind": { "type": "string", "required": true, "values": ["run"] }
                },
                "endAttributes": {
                    "pi.operation.outcome": { "type": "string", "values": ["completed", "aborted", "failed", "suspended"] },
                    "pi.error.code": { "type": "string", "cardinality": "low" },
                    "pi.error.type": { "type": "string", "cardinality": "low" }
                },
                "status": { "default": "ok" }
            },
            "pi.harness.compaction": {
                "parents": { "kind": "root_or_external" },
                "startAttributes": {
                    "pi.session.id": { "type": "string", "required": true },
                    "pi.lane.name": { "type": "string", "required": true },
                    "pi.operation.id": { "type": "string", "required": true },
                    "pi.operation.recovery": { "type": "boolean", "required": true },
                    "pi.operation.kind": { "type": "string", "required": true, "values": ["compaction"] }
                },
                "endAttributes": {
                    "pi.operation.outcome": { "type": "string", "values": ["completed", "declined", "aborted", "failed"] },
                    "pi.error.code": { "type": "string", "cardinality": "low" },
                    "pi.error.type": { "type": "string", "cardinality": "low" }
                },
                "status": { "default": "ok" }
            },
            "pi.harness.navigation": {
                "parents": { "kind": "root_or_external" },
                "startAttributes": {
                    "pi.session.id": { "type": "string", "required": true },
                    "pi.lane.name": { "type": "string", "required": true },
                    "pi.operation.id": { "type": "string", "required": true },
                    "pi.operation.recovery": { "type": "boolean", "required": true },
                    "pi.operation.kind": { "type": "string", "required": true, "values": ["navigation"] }
                },
                "endAttributes": {
                    "pi.operation.outcome": { "type": "string", "values": ["completed", "declined", "aborted", "failed"] },
                    "pi.error.code": { "type": "string", "cardinality": "low" },
                    "pi.error.type": { "type": "string", "cardinality": "low" }
                },
                "status": { "default": "ok" }
            },
            "pi.harness.checkpoint": {
                "parents": { "kind": "spans", "spans": ["pi.harness.run"] },
                "startAttributes": {
                    "pi.lane.name": { "type": "string", "required": true },
                    "pi.operation.id": { "type": "string", "required": true },
                    "pi.checkpoint.kind": { "type": "string", "required": true, "values": ["normal", "failure_drain", "abort_reconcile"] }
                },
                "endAttributes": {},
                "status": { "default": "ok" }
            },
            "pi.harness.turn": {
                "parents": { "kind": "spans", "spans": ["pi.harness.run"] },
                "startAttributes": {
                    "pi.lane.name": { "type": "string", "required": true },
                    "pi.operation.id": { "type": "string", "required": true },
                    "pi.turn.id": { "type": "string", "required": true }
                },
                "endAttributes": {},
                "status": { "default": "ok" }
            },
            "pi.harness.step": {
                "parents": { "kind": "spans", "spans": ["pi.harness.turn", "pi.harness.checkpoint", "pi.harness.compaction", "pi.harness.navigation"] },
                "startAttributes": {
                    "pi.lane.name": { "type": "string", "required": true },
                    "pi.operation.id": { "type": "string", "required": true },
                    "pi.step.kind": { "type": "string", "required": true, "values": ["assistant", "compaction", "branch_summary"] },
                    "pi.step.attempt": { "type": "number", "required": true },
                    "pi.compaction.reason": { "type": "string", "required": false, "values": ["manual", "threshold", "overflow"] }
                },
                "endAttributes": {
                    "pi.step.outcome": { "type": "string", "values": ["succeeded", "retry", "failed", "aborted", "deferred", "overflow"] }
                },
                "status": { "default": "ok" }
            },
            "pi.harness.tool": {
                "parents": { "kind": "spans", "spans": ["pi.harness.turn", "pi.harness.run"] },
                "startAttributes": {
                    "pi.lane.name": { "type": "string", "required": true },
                    "pi.operation.id": { "type": "string", "required": true },
                    "pi.turn.id": { "type": "string", "required": false },
                    "pi.tool.name": { "type": "string", "required": true },
                    "pi.tool.call_id": { "type": "string", "required": true },
                    "pi.tool.replay": { "type": "string", "required": true, "values": ["never", "safe"] },
                    "pi.tool.recovery": { "type": "boolean", "required": true }
                },
                "endAttributes": {
                    "pi.tool.is_error": { "type": "boolean" }
                },
                "status": { "default": "ok" }
            },
            "pi.harness.hook": {
                "parents": { "kind": "any" },
                "startAttributes": {
                    "pi.lane.name": { "type": "string", "required": true },
                    "pi.operation.id": { "type": "string", "required": false },
                    "pi.hook.name": { "type": "string", "required": true, "values": ["before_run", "before_resume", "before_run_end", "transform_context", "before_request", "before_payload", "after_response", "before_tool", "after_tool", "before_compaction", "before_navigation"] },
                    "pi.hook.registration_id": { "type": "string", "required": false }
                },
                "endAttributes": {
                    "pi.hook.outcome": { "type": "string", "values": ["completed", "skipped", "blocked", "failed"] }
                },
                "status": { "default": "ok" }
            },
            "pi.harness.sleep": {
                "parents": { "kind": "spans", "spans": ["pi.harness.step", "pi.harness.run"] },
                "startAttributes": {
                    "pi.operation.id": { "type": "string", "required": true },
                    "pi.sleep.delay_ms": { "type": "number", "required": true }
                },
                "endAttributes": {
                    "pi.sleep.outcome": { "type": "string", "values": ["elapsed", "aborted"] }
                },
                "status": { "default": "ok" }
            },
            "pi.harness.event_handler": {
                "parents": { "kind": "any" },
                "startAttributes": {
                    "pi.event.type": { "type": "string", "required": true, "values": ["run_start", "run_resume", "run_suspend", "run_abort", "run_end", "fault", "handler_error", "turn_start", "turn_end", "retry_scheduled", "retry_start", "retry_end", "message_start", "message_update", "message_end", "tool_start", "tool_update", "tool_end", "entry_added", "write_pending", "queue_update", "fact_update", "config_update", "compaction_start", "compaction_end", "navigation_start", "navigation_end", "lane_created", "usage"] },
                    "pi.lane.name": { "type": "string", "required": false }
                },
                "endAttributes": {},
                "status": { "default": "ok" }
            },
            "pi.session.write": {
                "parents": { "kind": "any" },
                "startAttributes": {
                    "pi.lane.name": { "type": "string", "required": true },
                    "pi.operation.id": { "type": "string", "required": false },
                    "pi.session.mutation": { "type": "string", "required": true, "values": ["entry", "record", "lane", "fact"] },
                    "pi.session.item_type": { "type": "string", "required": false }
                },
                "endAttributes": {
                    "pi.session.seq": { "type": "number" }
                },
                "status": { "default": "ok" }
            }
        }
    })
}

/// 组合 schema(对齐 TS `AGENT_TELEMETRY_SCHEMAS`)。
pub fn agent_telemetry_schemas() -> Vec<Value> {
    vec![ai_telemetry_schema(), harness_telemetry_schema()]
}

// ---------------------------------------------------------------------------
// start_* 辅助
// ---------------------------------------------------------------------------

/// 通用 span 包装:callback 期间持有 span,返回后 Drop 结束(对齐蓝本
/// `telemetryContext.startSpan({...}, callback)`)。
pub async fn start_span<T, F, Fut>(
    context: &dyn TelemetryContext,
    name: &str,
    attributes: Vec<(String, AttributeValue)>,
    callback: F,
) -> T
where
    F: FnOnce(Arc<dyn TelemetrySpan>) -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let span = context.start_span(SpanOptions {
        name: name.to_string(),
        attributes,
    });
    callback(span).await
}

/// AI 请求 span(名称对齐蓝本 `startAiSpan` 的 "pi.ai.request")。
pub async fn start_ai_span<T, F, Fut>(
    context: &dyn TelemetryContext,
    operation: &str,
    provider: &str,
    model_id: &str,
    api: &str,
    streaming: bool,
    callback: F,
) -> T
where
    F: FnOnce(Arc<dyn TelemetrySpan>) -> Fut,
    Fut: std::future::Future<Output = T>,
{
    start_span(
        context,
        "pi.ai.request",
        vec![
            (
                "pi.ai.operation".to_string(),
                AttributeValue::from(operation),
            ),
            ("pi.ai.provider".to_string(), AttributeValue::from(provider)),
            ("pi.ai.model".to_string(), AttributeValue::from(model_id)),
            ("pi.ai.api".to_string(), AttributeValue::from(api)),
            (
                "pi.ai.streaming".to_string(),
                AttributeValue::from(streaming),
            ),
        ],
        callback,
    )
    .await
}

/// harness run span("pi.harness.run")。
pub async fn start_harness_run_span<T, F, Fut>(
    context: &dyn TelemetryContext,
    session_id: &str,
    lane: &str,
    operation_id: &str,
    recovery: bool,
    callback: F,
) -> T
where
    F: FnOnce(Arc<dyn TelemetrySpan>) -> Fut,
    Fut: std::future::Future<Output = T>,
{
    start_span(
        context,
        "pi.harness.run",
        vec![
            (
                "pi.session.id".to_string(),
                AttributeValue::from(session_id),
            ),
            ("pi.lane.name".to_string(), AttributeValue::from(lane)),
            (
                "pi.operation.id".to_string(),
                AttributeValue::from(operation_id),
            ),
            (
                "pi.operation.recovery".to_string(),
                AttributeValue::from(recovery),
            ),
            ("pi.operation.kind".to_string(), AttributeValue::from("run")),
        ],
        callback,
    )
    .await
}

/// harness compaction span("pi.harness.compaction")。
pub async fn start_harness_compaction_span<T, F, Fut>(
    context: &dyn TelemetryContext,
    session_id: &str,
    lane: &str,
    operation_id: &str,
    recovery: bool,
    callback: F,
) -> T
where
    F: FnOnce(Arc<dyn TelemetrySpan>) -> Fut,
    Fut: std::future::Future<Output = T>,
{
    start_span(
        context,
        "pi.harness.compaction",
        vec![
            (
                "pi.session.id".to_string(),
                AttributeValue::from(session_id),
            ),
            ("pi.lane.name".to_string(), AttributeValue::from(lane)),
            (
                "pi.operation.id".to_string(),
                AttributeValue::from(operation_id),
            ),
            (
                "pi.operation.recovery".to_string(),
                AttributeValue::from(recovery),
            ),
            (
                "pi.operation.kind".to_string(),
                AttributeValue::from("compaction"),
            ),
        ],
        callback,
    )
    .await
}

/// harness navigation span("pi.harness.navigation")。
pub async fn start_harness_navigation_span<T, F, Fut>(
    context: &dyn TelemetryContext,
    session_id: &str,
    lane: &str,
    operation_id: &str,
    recovery: bool,
    callback: F,
) -> T
where
    F: FnOnce(Arc<dyn TelemetrySpan>) -> Fut,
    Fut: std::future::Future<Output = T>,
{
    start_span(
        context,
        "pi.harness.navigation",
        vec![
            (
                "pi.session.id".to_string(),
                AttributeValue::from(session_id),
            ),
            ("pi.lane.name".to_string(), AttributeValue::from(lane)),
            (
                "pi.operation.id".to_string(),
                AttributeValue::from(operation_id),
            ),
            (
                "pi.operation.recovery".to_string(),
                AttributeValue::from(recovery),
            ),
            (
                "pi.operation.kind".to_string(),
                AttributeValue::from("navigation"),
            ),
        ],
        callback,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_records_attributes_events_and_status() {
        let context = InMemoryTelemetryContext::new();
        let result = start_ai_span(
            &context,
            "stream",
            "custom",
            "test-model",
            "openai-completions",
            true,
            |span| async move {
                span.set_attribute("pi.ai.usage.total_tokens", AttributeValue::Integer(42));
                span.add_event("retry", vec![]);
                span.set_status(SpanStatus::Error);
                "done"
            },
        )
        .await;
        assert_eq!(result, "done");
        let spans = context.recorded_spans();
        assert_eq!(spans.len(), 1);
        let span = &spans[0];
        assert_eq!(span.name, "pi.ai.request");
        assert!(matches!(
            span.attributes.get("pi.ai.operation"),
            Some(AttributeValue::Text(operation)) if operation == "stream"
        ));
        assert_eq!(span.events.len(), 1);
        assert_eq!(span.status, SpanStatus::Error);
        assert!(span.end_ms >= span.start_ms);
    }

    #[tokio::test]
    async fn noop_is_safe() {
        let result = start_span(&NOOP, "noop.span", vec![], |span| async move {
            span.set_attribute("k", AttributeValue::from("v"));
            span.record_error("boom");
            7
        })
        .await;
        assert_eq!(result, 7);
    }

    #[test]
    fn schemas_keep_blueprint_names() {
        let schemas = agent_telemetry_schemas();
        assert_eq!(schemas.len(), 2);
        assert!(schemas[0]["spans"].get("pi.ai.request").is_some());
        assert!(schemas[1]["spans"].get("pi.harness.run").is_some());
        assert!(schemas[1]["spans"].get("pi.session.write").is_some());
        assert_eq!(HOOK_NAMES.len(), 11);
        assert_eq!(EVENT_TYPES.len(), 29);
    }
}
