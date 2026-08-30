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

/// harness 事件(对齐 TS `HarnessEvent`,tag 值一致)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum HarnessEvent {
    #[serde(rename = "run_start")]
    RunStart(RunStartEvent),
    #[serde(rename = "run_end")]
    RunEnd(RunEndEvent),
}

impl HarnessEvent {
    /// 事件判别名(对齐 `HarnessEventType`)。
    pub fn event_type(&self) -> HarnessEventType {
        match self {
            HarnessEvent::RunStart(_) => HarnessEventType::RunStart,
            HarnessEvent::RunEnd(_) => HarnessEventType::RunEnd,
        }
    }
}

/// 事件类型名(对齐 TS `HarnessEventType` 字面量)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HarnessEventType {
    RunStart,
    RunEnd,
}

impl HarnessEventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            HarnessEventType::RunStart => "run_start",
            HarnessEventType::RunEnd => "run_end",
        }
    }
}

/// 事件监听器(同步回调;异步处理自行 spawn,见模块注释)。
pub type HarnessEventListener = Arc<dyn Fn(&HarnessEvent) + Send + Sync>;

struct WatchShared {
    listener: Mutex<Option<HarnessEventListener>>,
    buffered: Mutex<Vec<HarnessEvent>>,
    receiving: std::sync::atomic::AtomicBool,
}

impl WatchShared {
    fn receive(&self, event: &HarnessEvent) {
        if !self.receiving.load(Ordering::SeqCst) {
            return;
        }
        let has_listener = {
            let listener = self.listener.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            listener.is_some()
        };
        if has_listener {
            let listener = self.listener.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(listener) = listener.as_ref() {
                listener(event);
            }
            return;
        }
        let mut buffered = self.buffered.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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

    /// 退订 watcher 并清空缓冲(对齐 TS `unsubscribe`)。
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
        let mut listener = self
            .shared
            .listener
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *listener = None;
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
    pub fn on(&self, event_type: HarnessEventType, listener: HarnessEventListener) -> Box<dyn FnOnce() + Send> {
        let id = self.id_counter.fetch_add(1, Ordering::SeqCst);
        {
            let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            state.listeners.entry(event_type).or_default().insert(id, listener);
        }
        let state = self.state.clone();
        Box::new(move || {
            let mut state = state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
            let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
    pub fn watch<TSnapshot>(&self, capture_snapshot: impl FnOnce() -> TSnapshot) -> WatchHandle<TSnapshot> {
        let id = self.id_counter.fetch_add(1, Ordering::SeqCst);
        let shared = Arc::new(WatchShared {
            listener: Mutex::new(None),
            buffered: Mutex::new(Vec::new()),
            receiving: std::sync::atomic::AtomicBool::new(true),
        });
        {
            let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
        bus.on(
            HarnessEventType::RunStart,
            Arc::new(move |_| {
                s.fetch_add(1, Ordering::SeqCst);
            }),
        );
        let e = ends.clone();
        bus.on(
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
        assert_eq!(seen.lock().unwrap().len(), 2, "buffered events flush on start");
        bus.emit(&run_start("main"));
        assert_eq!(seen.lock().unwrap().len(), 3);
        assert_eq!(handle.snapshot, "snapshot");

        handle.unsubscribe();
        bus.emit(&run_start("main"));
        assert_eq!(seen.lock().unwrap().len(), 3, "no delivery after unsubscribe");
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
}
