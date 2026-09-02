//! harness 事件总线:对齐 `packages/agent/src/harness/events.ts`。
//!
//! `HarnessEventBus.on(type, listener)` 注册被动监听并返回退订闭包(不回放历史、
//! 不提供快照);`emit` 同步投递到对应类型的监听器与全部 watcher;
//! `watch(capture_snapshot)` 返回带缓冲的 WatchHandle —— start 之前的事件先缓冲,
//! start 时按序冲刷(冲刷期间再入 emit 保持顺序),`unsubscribe` 退订并清空缓冲。
//!
//! 偏差:TS 监听器可返回 Promise(emit 同步发起、不等待);Rust 监听器为同步
//! 回调 `Fn(&HarnessEvent)`(需要异步处理的监听器自行 spawn 任务),emit 语义
//! (同步、不等待)不变。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// run 开始事件(对齐 TS `RunStartEvent`)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStartEvent {
    pub lane: String,
    pub run_id: String,
}

/// run 结束事件(对齐 TS `RunEndEvent`)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunEndEvent {
    pub lane: String,
    pub run_id: String,
    pub outcome: RunEndOutcome,
    pub leaf_id: String,
}

/// run 结束结果(对齐 TS `outcome` 字面量)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunEndOutcome {
    Completed,
    Aborted,
    Failed,
}

/// 消息条目落库事件(本仓库运行时扩展:TS scaffold 未定义;wiki 等消费方
/// 需要正文流式与最终消息,run_start/run_end 不足以表达)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageEvent {
    pub lane: String,
    pub run_id: String,
    pub entry_id: String,
    pub message: crate::agent::types::AgentMessage,
}

/// 消息增量事件(本仓库运行时扩展:assistant 流式期间 core
/// `AgentEvent::MessageUpdate` 的透传,含完整的 `assistant_message_event`;
/// 只经事件总线分发,不落 session,供实时 UI/用量监听方消费)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageUpdateEvent {
    pub lane: String,
    pub run_id: String,
    pub message: crate::agent::types::AgentMessage,
    pub assistant_message_event: crate::agent::llm::types::AssistantMessageEvent,
}

/// 工具执行事件(本仓库运行时扩展;phase 对齐 loop 的 start/update/end)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolEvent {
    pub lane: String,
    pub run_id: String,
    pub phase: ToolEventPhase,
    pub tool_call_id: String,
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_result: Option<crate::agent::types::AgentToolResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<crate::agent::types::AgentToolResult>,
    #[serde(default)]
    pub is_error: bool,
}

/// 工具事件阶段。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolEventPhase {
    Start,
    Update,
    End,
}

/// 用量事件(本仓库运行时扩展;usage 落库后发出)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEvent {
    pub lane: String,
    pub run_id: String,
    pub usage: crate::agent::llm::types::Usage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_id: Option<String>,
    /// 本次 assistant 请求耗时毫秒(message start → message end;
    /// 无 start 计时(如流未按契约先发 start)时缺省)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<i64>,
}

/// harness 事件(对齐 TS `HarnessEvent`,tag 值一致;message/message_update/
/// tool/usage 为本仓库运行时扩展,TS scaffold 未定义)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum HarnessEvent {
    #[serde(rename = "run_start")]
    RunStart(RunStartEvent),
    #[serde(rename = "run_end")]
    RunEnd(RunEndEvent),
    #[serde(rename = "message")]
    Message(MessageEvent),
    #[serde(rename = "message_update")]
    MessageUpdate(MessageUpdateEvent),
    #[serde(rename = "tool")]
    Tool(ToolEvent),
    #[serde(rename = "usage")]
    Usage(UsageEvent),
}

impl HarnessEvent {
    /// 事件判别名(对齐 `HarnessEventType`)。
    pub fn event_type(&self) -> HarnessEventType {
        match self {
            HarnessEvent::RunStart(_) => HarnessEventType::RunStart,
            HarnessEvent::RunEnd(_) => HarnessEventType::RunEnd,
            HarnessEvent::Message(_) => HarnessEventType::Message,
            HarnessEvent::MessageUpdate(_) => HarnessEventType::MessageUpdate,
            HarnessEvent::Tool(_) => HarnessEventType::Tool,
            HarnessEvent::Usage(_) => HarnessEventType::Usage,
        }
    }
}

/// 事件类型名(对齐 TS `HarnessEventType` 字面量;扩展项为运行时新增)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HarnessEventType {
    RunStart,
    RunEnd,
    Message,
    MessageUpdate,
    Tool,
    Usage,
}

impl HarnessEventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            HarnessEventType::RunStart => "run_start",
            HarnessEventType::RunEnd => "run_end",
            HarnessEventType::Message => "message",
            HarnessEventType::MessageUpdate => "message_update",
            HarnessEventType::Tool => "tool",
            HarnessEventType::Usage => "usage",
        }
    }
}

/// 事件监听器(同步回调;异步处理自行 spawn,见模块注释)。
pub type HarnessEventListener = Arc<dyn Fn(&HarnessEvent) + Send + Sync>;

struct WatchShared {
    listener: Mutex<Option<HarnessEventListener>>,
    buffered: Mutex<Vec<HarnessEvent>>,
    receiving: std::sync::atomic::AtomicBool,
    /// 所属总线(弱引用避免环);unsubscribe 时从 watchers map 摘除,
    /// 防长生命周期进程累积失效 watcher。
    bus: std::sync::Weak<Mutex<BusState>>,
    watcher_id: u64,
}

impl WatchShared {
    fn receive(&self, event: &HarnessEvent) {
        if !self.receiving.load(Ordering::SeqCst) {
            return;
        }
        let has_listener = {
            let listener = self
                .listener
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            listener.is_some()
        };
        if has_listener {
            let listener = self
                .listener
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(listener) = listener.as_ref() {
                listener(event);
            }
            return;
        }
        let mut buffered = self
            .buffered
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        buffered.push(event.clone());
    }
}

/// watch 句柄(对齐 TS `WatchHandle<TSnapshot>`)。
pub struct WatchHandle<TSnapshot> {
    /// 创建 watch 时的快照。
    pub snapshot: TSnapshot,
    shared: Arc<WatchShared>,
}

impl<TSnapshot> WatchHandle<TSnapshot> {
    /// 开始消费后续事件;先冲刷 start 之前缓冲的事件(保持顺序)。
    pub fn start(&self, listener: HarnessEventListener) {
        let pending: Vec<HarnessEvent> = {
            let mut buffered = self
                .shared
                .buffered
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut listener_slot = self
                .shared
                .listener
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let pending = buffered.drain(..).collect();
            *listener_slot = Some(listener);
            pending
        };
        for event in pending {
            self.shared.receive(&event);
        }
    }

    /// 退订 watcher 并清空缓冲(对齐 TS `unsubscribe`);同时从总线 watchers
    /// map 摘除本 watcher(本仓库补强:TS 侧 Map.delete,先前复刻遗漏导致泄漏)。
    pub fn unsubscribe(&self) {
        self.shared.receiving.store(false, Ordering::SeqCst);
        {
            let mut buffered = self
                .shared
                .buffered
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            buffered.clear();
        }
        {
            let mut listener = self
                .shared
                .listener
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *listener = None;
        }
        if let Some(bus) = self.shared.bus.upgrade() {
            let mut state = bus.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            state.watchers.remove(&self.shared.watcher_id);
        }
    }
}

struct BusState {
    listeners: HashMap<HarnessEventType, HashMap<u64, HarnessEventListener>>,
    watchers: HashMap<u64, Arc<WatchShared>>,
}

impl Default for BusState {
    fn default() -> Self {
        Self {
            listeners: HashMap::new(),
            watchers: HashMap::new(),
        }
    }
}

/// harness 事件总线(对齐 TS `HarnessEventBus implements Events`)。
#[derive(Clone, Default)]
pub struct HarnessEventBus {
    state: Arc<Mutex<BusState>>,
    id_counter: Arc<AtomicU64>,
}

impl HarnessEventBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册某类型未来事件的被动监听,返回退订闭包(对齐 TS `on`)。
    ///
    /// 闭包只应调用一次;不调用则监听保持注册(与 TS 一致)。
    pub fn on(
        &self,
        event_type: HarnessEventType,
        listener: HarnessEventListener,
    ) -> Box<dyn FnOnce() + Send> {
        let id = self.id_counter.fetch_add(1, Ordering::SeqCst);
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state
                .listeners
                .entry(event_type)
                .or_default()
                .insert(id, listener);
        }
        let state = self.state.clone();
        Box::new(move || {
            let mut state = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(listeners) = state.listeners.get_mut(&event_type) {
                listeners.remove(&id);
                if listeners.is_empty() {
                    state.listeners.remove(&event_type);
                }
            }
        })
    }

    /// 投递事件到当前事件订阅与 watch 订阅(对齐 TS `emit`)。
    pub fn emit(&self, event: &HarnessEvent) {
        let event_type = event.event_type();
        let targets: Vec<HarnessEventListener>;
        let watchers: Vec<Arc<WatchShared>>;
        {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            targets = state
                .listeners
                .get(&event_type)
                .map(|listeners| listeners.values().cloned().collect())
                .unwrap_or_default();
            watchers = state.watchers.values().cloned().collect();
        }
        // 只投递给直接注册该类型的监听器;异步结果不等待(emit 同步)。
        for listener in targets {
            listener(event);
        }
        // 每个事件都投给 watcher;watch() 负责缓冲直到 start()。
        for watcher in watchers {
            watcher.receive(event);
        }
    }

    /// 创建带快照与缓冲的 watch 句柄(对齐 TS `watch`)。
    pub fn watch<TSnapshot>(
        &self,
        capture_snapshot: impl FnOnce() -> TSnapshot,
    ) -> WatchHandle<TSnapshot> {
        let id = self.id_counter.fetch_add(1, Ordering::SeqCst);
        let shared = Arc::new(WatchShared {
            listener: Mutex::new(None),
            buffered: Mutex::new(Vec::new()),
            receiving: std::sync::atomic::AtomicBool::new(true),
            bus: Arc::downgrade(&self.state),
            watcher_id: id,
        });
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.watchers.insert(id, shared.clone());
        }
        let snapshot = capture_snapshot();
        WatchHandle { snapshot, shared }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn run_start(lane: &str) -> HarnessEvent {
        HarnessEvent::RunStart(RunStartEvent {
            lane: lane.to_string(),
            run_id: "run-1".to_string(),
        })
    }

    #[test]
    fn on_receives_only_matching_type() {
        let bus = HarnessEventBus::new();
        let starts = Arc::new(AtomicUsize::new(0));
        let ends = Arc::new(AtomicUsize::new(0));
        let s = starts.clone();
        let _ = bus.on(
            HarnessEventType::RunStart,
            Arc::new(move |_| {
                s.fetch_add(1, Ordering::SeqCst);
            }),
        );
        let e = ends.clone();
        let _ = bus.on(
            HarnessEventType::RunEnd,
            Arc::new(move |_| {
                e.fetch_add(1, Ordering::SeqCst);
            }),
        );
        bus.emit(&run_start("main"));
        bus.emit(&HarnessEvent::RunEnd(RunEndEvent {
            lane: "main".into(),
            run_id: "run-1".into(),
            outcome: RunEndOutcome::Completed,
            leaf_id: "leaf".into(),
        }));
        bus.emit(&run_start("main"));
        assert_eq!(starts.load(Ordering::SeqCst), 2);
        assert_eq!(ends.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn unsubscribe_stops_delivery() {
        let bus = HarnessEventBus::new();
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let unsubscribe = bus.on(
            HarnessEventType::RunStart,
            Arc::new(move |_| {
                c.fetch_add(1, Ordering::SeqCst);
            }),
        );
        bus.emit(&run_start("main"));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        unsubscribe();
        bus.emit(&run_start("main"));
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn watch_buffers_until_start_then_streams() {
        let bus = HarnessEventBus::new();
        let handle = bus.watch(|| "snapshot");
        bus.emit(&run_start("main"));
        bus.emit(&run_start("lane"));

        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        handle.start(Arc::new(move |event| {
            sink.lock().unwrap().push(event.event_type());
        }));
        assert_eq!(
            seen.lock().unwrap().len(),
            2,
            "buffered events flush on start"
        );
        bus.emit(&run_start("main"));
        assert_eq!(seen.lock().unwrap().len(), 3);
        assert_eq!(handle.snapshot, "snapshot");

        handle.unsubscribe();
        bus.emit(&run_start("main"));
        assert_eq!(
            seen.lock().unwrap().len(),
            3,
            "no delivery after unsubscribe"
        );
    }

    #[test]
    fn unsubscribe_removes_watcher_from_bus() {
        let bus = HarnessEventBus::new();
        let handle = bus.watch(|| ());
        {
            let state = bus
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert_eq!(state.watchers.len(), 1);
        }
        handle.unsubscribe();
        let state = bus
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(
            state.watchers.is_empty(),
            "unsubscribed watcher must be removed from the bus map"
        );
    }

    #[test]
    fn event_json_shape_matches_ts() {
        let value = serde_json::to_value(run_start("main")).unwrap();
        assert_eq!(value["type"], "run_start");
        assert_eq!(value["lane"], "main");
        assert_eq!(value["runId"], "run-1");
        let parsed: HarnessEvent = serde_json::from_value(value).unwrap();
        assert_eq!(parsed, run_start("main"));
    }

    fn message_update_event() -> HarnessEvent {
        HarnessEvent::MessageUpdate(MessageUpdateEvent {
            lane: "main".to_string(),
            run_id: "run-1".to_string(),
            message: crate::agent::types::AgentMessage::user_text("hi", 1),
            assistant_message_event: crate::agent::llm::types::AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "hel".to_string(),
                partial: crate::agent::llm::types::AssistantMessage {
                    role: "assistant".to_string(),
                    content: Vec::new(),
                    api: String::new(),
                    provider: String::new(),
                    model: String::new(),
                    response_model: None,
                    response_id: None,
                    usage: crate::agent::llm::types::Usage::default(),
                    stop_reason: crate::agent::llm::types::StopReason::Pending,
                    error_message: None,
                    raw_stop_reason: None,
                    end_turn: None,
                    timestamp: 1,
                },
            },
        })
    }

    #[test]
    fn message_update_json_shape_matches_ts() {
        let value = serde_json::to_value(message_update_event()).unwrap();
        assert_eq!(value["type"], "message_update");
        assert_eq!(value["runId"], "run-1");
        assert_eq!(value["message"]["role"], "user");
        assert_eq!(value["assistantMessageEvent"]["type"], "text_delta");
        assert_eq!(value["assistantMessageEvent"]["delta"], "hel");
        assert_eq!(
            message_update_event().event_type(),
            HarnessEventType::MessageUpdate
        );
        // 注:与 HarnessEvent::Message 相同,internally-tagged 枚举内嵌套的
        // untagged AgentMessage 反序列化会落入 Custom 回退(TypedMessage 的
        // role tag 与消息结构体自身的 role 字段冲突,types.rs 既有形状)。
        // 事件总线消费方使用类型化 Rust 枚举,不依赖 JSON 回读,此处仅校验
        // 线上形状(与 run_start 用例同口径)。
    }

    #[test]
    fn usage_elapsed_ms_is_optional_on_wire() {
        let with_elapsed = HarnessEvent::Usage(UsageEvent {
            lane: "main".to_string(),
            run_id: "run-1".to_string(),
            usage: crate::agent::llm::types::Usage::default(),
            entry_id: Some("entry-1".to_string()),
            elapsed_ms: Some(42),
        });
        let value = serde_json::to_value(&with_elapsed).unwrap();
        assert_eq!(value["type"], "usage");
        assert_eq!(value["elapsedMs"], 42);
        let parsed: HarnessEvent = serde_json::from_value(value).unwrap();
        assert_eq!(parsed, with_elapsed);

        // 旧消费者写入的无 elapsed_ms 事件仍可解析(serde default 兼容)。
        let legacy = serde_json::json!({
            "type": "usage",
            "lane": "main",
            "runId": "run-1",
            "usage": {},
        });
        let parsed: HarnessEvent = serde_json::from_value(legacy).unwrap();
        let HarnessEvent::Usage(usage) = parsed else {
            panic!("expected usage event");
        };
        assert_eq!(usage.elapsed_ms, None);
        assert_eq!(usage.entry_id, None);
    }
}
