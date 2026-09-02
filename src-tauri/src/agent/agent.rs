//! 有状态 Agent 封装:对齐 `packages/agent/src/agent.ts`(pi-agent-core 0.84.4)。
//!
//! 蓝本语义要点(逐条对齐):
//! - 构造即注入初始状态(AgentState)+ 运行配置(AgentLoopConfig)+ StreamFn;
//!   TS 的 AgentOptions 全字段分别落位于 state(初始上下文)、config(钩子/队列/
//!   流选项)与 stream_fn,另提供 steering/follow-up 队列模式设置器。
//! - `prompt` 忙时 Err;`continue` 末尾 assistant 时先 drain steering →
//!   runPromptMessages(skipInitialSteeringPoll),否则 drain followUp,都没有则 Err;
//!   末尾 user/toolResult → runContinuation。
//! - 事件经 processEvents 归约内部状态(message_end 推入 messages、
//!   tool_execution_start/end 增删 pendingToolCalls、turn_end 记录 errorMessage、
//!   agent_end 清 streamingMessage),然后按订阅顺序 await listeners(带当前
//!   run 的 abort signal)。
//! - 失败路径(runner Err)由 handleRunFailure 合成 assistant 空文本消息
//!   (stopReason aborted/error + errorMessage)并走 message_start/end/turn_end/
//!   agent_end,finally finishRun。
//! - 运行期状态:isStreaming/streamingMessage/pendingToolCalls/errorMessage;
//!   waitForIdle 在 agent_end listeners 全部落定后解决。
//!
//! Rust 侧映射说明:
//! - `Agent` 是 `Arc<AgentInner>` 的克隆句柄;state/listeners/queues/active_run
//!   各自持 `std::sync::Mutex`,约定"绝不在持锁期间 await"。
//! - `subscribe` 返回自增订阅 id,`unsubscribe(id)` 退订(对齐 TS 返回的退订函数)。
//! - 构造注入的 AgentLoopConfig 在每次运行前被克隆并叠加运行期状态
//!   (model/reasoning 来自 state)与 Agent 自身的 steering/follow-up 队列
//!   (队列 drain 优先,队列为空时回退注入 config 自带的 getter)。

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use futures::future::BoxFuture;
use serde_json::Value;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::agent::agent_loop::{
    assistant_message_of, now_ms, reasoning_from_thinking_level, run_agent_loop,
    run_agent_loop_continue,
};
use crate::agent::llm::{
    AssistantContent, AssistantMessage, Message, Model, ModelThinkingLevel, OnPayloadFn,
    OnResponseFn, ProviderResponse, SimpleStreamOptions, StopReason, TextOrImageContent, Usage,
    UserContent, UserMessage,
};
use crate::agent::stream_fn::default_stream_fn;
use crate::agent::types::{
    AbortSignal, AgentContext, AgentEvent, AgentEventSink, AgentListener, AgentLoopConfig,
    AgentMessage, AgentState, GetQueuedMessagesFn, QueueMode, StreamFn, TypedMessage,
};

/// Agent 持有的可克隆 payload 观测回调(构造 SimpleStreamOptions 时装箱)。
pub type OnPayloadCallback = Arc<dyn Fn(Value) -> BoxFuture<'static, Option<Value>> + Send + Sync>;
/// Agent 持有的可克隆响应观测回调(构造 SimpleStreamOptions 时装箱)。
pub type OnResponseCallback = Arc<dyn Fn(&ProviderResponse) + Send + Sync>;

/// TS `prepareNextTurn(signal)`(无上下文变体)的 Rust 形状;注入的
/// AgentLoopConfig.prepare_next_turn 直接承载上下文变体。
pub type PrepareNextTurnSimpleFn = Arc<
    dyn Fn(
            Option<AbortSignal>,
        ) -> BoxFuture<'static, Option<crate::agent::types::AgentLoopTurnUpdate>>
        + Send
        + Sync,
>;

/// TS 蓝本的 `defaultConvertToLlm`:已知 role 原样转换,自定义消息全部过滤。
pub fn default_convert_to_llm(messages: Vec<AgentMessage>) -> Vec<Message> {
    messages
        .into_iter()
        .filter_map(|message| match message {
            AgentMessage::Message(TypedMessage::User(user)) => Some(Message::User(user)),
            AgentMessage::Message(TypedMessage::Assistant(assistant)) => {
                Some(Message::Assistant(assistant))
            }
            AgentMessage::Message(TypedMessage::ToolResult(tool_result)) => {
                Some(Message::ToolResult(tool_result))
            }
            AgentMessage::Custom(_) => None,
        })
        .collect()
}

/// [`default_convert_to_llm`] 的 ConvertToLlmFn 包装。
pub fn default_convert_to_llm_fn() -> crate::agent::types::ConvertToLlmFn {
    Arc::new(move |messages| Box::pin(async move { default_convert_to_llm(messages) }))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 排队消息队列(mode 决定 drain 数量)。
#[derive(Debug)]
struct PendingMessageQueue {
    mode: QueueMode,
    messages: Vec<AgentMessage>,
}

impl PendingMessageQueue {
    fn new(mode: QueueMode) -> Self {
        Self {
            mode,
            messages: Vec::new(),
        }
    }

    fn enqueue(&mut self, message: AgentMessage) {
        self.messages.push(message);
    }

    fn has_items(&self) -> bool {
        !self.messages.is_empty()
    }

    fn drain(&mut self) -> Vec<AgentMessage> {
        match self.mode {
            QueueMode::All => std::mem::take(&mut self.messages),
            QueueMode::OneAtATime => {
                if self.messages.is_empty() {
                    Vec::new()
                } else {
                    vec![self.messages.remove(0)]
                }
            }
        }
    }

    fn clear(&mut self) {
        self.messages.clear();
    }
}

struct ActiveRun {
    abort: AbortSignal,
    done_tx: watch::Sender<bool>,
    done_rx: watch::Receiver<bool>,
}

#[derive(Default)]
struct Listeners {
    items: Vec<(u64, AgentListener)>,
    next_id: u64,
}

pub(crate) struct AgentInner {
    state: Mutex<AgentState>,
    /// 构造时注入的运行配置;每次运行克隆并叠加运行期状态。
    base_config: Mutex<AgentLoopConfig>,
    stream_function: Mutex<StreamFn>,
    listeners: Mutex<Listeners>,
    steering_queue: Arc<Mutex<PendingMessageQueue>>,
    follow_up_queue: Arc<Mutex<PendingMessageQueue>>,
    active_run: Mutex<Option<ActiveRun>>,
}

impl AgentInner {
    fn create_context_snapshot(&self) -> AgentContext {
        let state = lock(&self.state);
        AgentContext {
            system_prompt: state.system_prompt.clone(),
            messages: state.messages.clone(),
            tools: state.tools.clone(),
        }
    }

    /// 组装本次运行的 AgentLoopConfig(对齐 TS `createLoopConfig`):
    /// model/reasoning 取自当前 state,叠加 steering/follow-up 队列 getter。
    fn build_run_config(&self, skip_initial_steering_poll: bool) -> AgentLoopConfig {
        let mut config = lock(&self.base_config).clone();
        {
            let state = lock(&self.state);
            config.model = state.model.clone();
            config.stream.reasoning = reasoning_from_thinking_level(state.thinking_level);
        }

        // Agent 队列优先 drain;为空时回退注入 config 自带 getter(保持低层配置可用)。
        let steering_queue = self.steering_queue.clone();
        let skip = Arc::new(AtomicBool::new(skip_initial_steering_poll));
        let base_steering = config.get_steering_messages.clone();
        let get_steering_messages: GetQueuedMessagesFn = Arc::new(move || {
            let queue = steering_queue.clone();
            let skip = skip.clone();
            let base = base_steering.clone();
            Box::pin(async move {
                if skip.swap(false, Ordering::SeqCst) {
                    return Vec::new();
                }
                let drained = queue.lock().unwrap().drain();
                if !drained.is_empty() {
                    return drained;
                }
                match base {
                    Some(get) => get().await,
                    None => Vec::new(),
                }
            })
        });
        config.get_steering_messages = Some(get_steering_messages);

        let follow_up_queue = self.follow_up_queue.clone();
        let base_follow_up = config.get_follow_up_messages.clone();
        let get_follow_up_messages: GetQueuedMessagesFn = Arc::new(move || {
            let queue = follow_up_queue.clone();
            let base = base_follow_up.clone();
            Box::pin(async move {
                let drained = queue.lock().unwrap().drain();
                if !drained.is_empty() {
                    return drained;
                }
                match base {
                    Some(get) => get().await,
                    None => Vec::new(),
                }
            })
        });
        config.get_follow_up_messages = Some(get_follow_up_messages);

        config
    }

    fn emit_sink(self: &Arc<Self>) -> AgentEventSink {
        let inner = self.clone();
        Arc::new(move |event| {
            let inner = inner.clone();
            Box::pin(async move { inner.process_events(event).await })
        })
    }

    /// 归约内部状态后按订阅顺序 await listeners(TS `processEvents`)。
    async fn process_events(&self, event: AgentEvent) {
        {
            let mut state = lock(&self.state);
            match &event {
                AgentEvent::MessageStart { message } => {
                    state.streaming_message = Some(message.clone());
                }
                AgentEvent::MessageUpdate { message, .. } => {
                    state.streaming_message = Some(message.clone());
                }
                AgentEvent::MessageEnd { message } => {
                    state.streaming_message = None;
                    state.messages.push(message.clone());
                }
                AgentEvent::ToolExecutionStart { tool_call_id, .. } => {
                    state.pending_tool_calls.insert(tool_call_id.clone());
                }
                AgentEvent::ToolExecutionEnd { tool_call_id, .. } => {
                    state.pending_tool_calls.remove(tool_call_id);
                }
                AgentEvent::TurnEnd { message, .. } => {
                    if let AgentMessage::Message(TypedMessage::Assistant(assistant)) = message {
                        if let Some(error) = &assistant.error_message {
                            state.error_message = Some(error.clone());
                        }
                    }
                }
                AgentEvent::AgentEnd { .. } => {
                    state.streaming_message = None;
                }
                // AgentStart / TurnStart / ToolExecutionUpdate 不改内部状态。
                AgentEvent::AgentStart
                | AgentEvent::TurnStart
                | AgentEvent::ToolExecutionUpdate { .. } => {}
            }
        }

        let signal = { lock(&self.active_run).as_ref().map(|run| run.abort.clone()) }
            .expect("Agent listener invoked outside active run");
        let listeners: Vec<AgentListener> = {
            lock(&self.listeners)
                .items
                .iter()
                .map(|(_, listener)| listener.clone())
                .collect()
        };
        for listener in listeners {
            listener(event.clone(), signal.clone()).await;
        }
    }

    /// 运行失败:合成 assistant 空文本消息并走完整事件序列(TS `handleRunFailure`)。
    async fn handle_run_failure(&self, error: String, aborted: bool) {
        let (api, provider, model) = {
            let state = lock(&self.state);
            (
                state.model.api.clone(),
                state.model.provider.clone(),
                state.model.id.clone(),
            )
        };
        let failure = AssistantMessage {
            role: "assistant".to_string(),
            content: vec![AssistantContent::text("")],
            api,
            provider,
            model,
            response_model: None,
            response_id: None,
            usage: Usage::zero(),
            stop_reason: if aborted {
                StopReason::Aborted
            } else {
                StopReason::Error
            },
            error_message: Some(error),
            raw_stop_reason: None,
            end_turn: None,
            timestamp: now_ms(),
        };
        let message = assistant_message_of(failure);
        self.process_events(AgentEvent::MessageStart {
            message: message.clone(),
        })
        .await;
        self.process_events(AgentEvent::MessageEnd {
            message: message.clone(),
        })
        .await;
        self.process_events(AgentEvent::TurnEnd {
            message: message.clone(),
            tool_results: Vec::new(),
        })
        .await;
        self.process_events(AgentEvent::AgentEnd {
            messages: vec![message],
        })
        .await;
    }

    fn finish_run(&self) {
        {
            let mut state = lock(&self.state);
            state.is_streaming = false;
            state.streaming_message = None;
            state.pending_tool_calls.clear();
        }
        let run = lock(&self.active_run).take();
        if let Some(run) = run {
            let _ = run.done_tx.send(true);
        }
    }
}

/// Stateful wrapper around the low-level agent loop。
///
/// `Agent` owns the current transcript, emits lifecycle events, executes tools,
/// and exposes queueing APIs for steering and follow-up messages.
#[derive(Clone)]
pub struct Agent {
    pub(crate) inner: Arc<AgentInner>,
}

/// `prompt` 的输入形态(对齐 TS `prompt(string | AgentMessage | AgentMessage[])`)。
#[derive(Clone, Debug)]
pub enum PromptInput {
    /// 纯文本(可附图片,经 [`Agent::prompt_text`])。
    Text(String),
    /// 单条或批量 AgentMessage。
    Messages(Vec<AgentMessage>),
}

impl From<&str> for PromptInput {
    fn from(text: &str) -> Self {
        PromptInput::Text(text.to_string())
    }
}

impl From<String> for PromptInput {
    fn from(text: String) -> Self {
        PromptInput::Text(text)
    }
}

impl From<AgentMessage> for PromptInput {
    fn from(message: AgentMessage) -> Self {
        PromptInput::Messages(vec![message])
    }
}

impl From<Vec<AgentMessage>> for PromptInput {
    fn from(messages: Vec<AgentMessage>) -> Self {
        PromptInput::Messages(messages)
    }
}

impl Agent {
    /// 创建 Agent:初始状态 + 运行配置 + 流函数(缺省回退全局默认,见
    /// [`crate::agent::stream_fn`],未配置时 panic,对齐 TS `?? getDefaultStreamFn()`)。
    ///
    /// 运行期状态字段(isStreaming/streamingMessage/pendingToolCalls/errorMessage)
    /// 在此归零,对齐 TS `createMutableAgentState` 的 initialState 白名单。
    pub fn new(state: AgentState, config: AgentLoopConfig, stream_fn: StreamFn) -> Agent {
        Self::build(state, config, stream_fn)
    }

    /// TS `streamFn ?? getDefaultStreamFn()` 回退语义的构造入口:未显式提供
    /// stream_fn 时取全局默认(未安装则 panic,见 [`crate::agent::stream_fn`])。
    pub fn new_with_default_stream_fn(state: AgentState, config: AgentLoopConfig) -> Agent {
        Self::build(state, config, default_stream_fn())
    }

    fn build(state: AgentState, config: AgentLoopConfig, stream_fn: StreamFn) -> Agent {
        let mut state = state;
        state.is_streaming = false;
        state.streaming_message = None;
        state.pending_tool_calls = HashSet::new();
        state.error_message = None;
        Agent {
            inner: Arc::new(AgentInner {
                state: Mutex::new(state),
                base_config: Mutex::new(config),
                stream_function: Mutex::new(stream_fn),
                listeners: Mutex::new(Listeners::default()),
                steering_queue: Arc::new(Mutex::new(PendingMessageQueue::new(
                    QueueMode::OneAtATime,
                ))),
                follow_up_queue: Arc::new(Mutex::new(PendingMessageQueue::new(
                    QueueMode::OneAtATime,
                ))),
                active_run: Mutex::new(None),
            }),
        }
    }

    /// 订阅生命周期事件(按订阅顺序 await);返回订阅 id,经 [`Agent::unsubscribe`] 退订。
    pub fn subscribe(&self, listener: AgentListener) -> u64 {
        let mut listeners = lock(&self.inner.listeners);
        let id = listeners.next_id;
        listeners.next_id += 1;
        listeners.items.push((id, listener));
        id
    }

    /// 退订事件监听器。
    pub fn unsubscribe(&self, id: u64) {
        lock(&self.inner.listeners)
            .items
            .retain(|(listener_id, _)| *listener_id != id);
    }

    /// 当前状态快照(克隆)。
    pub fn state(&self) -> AgentState {
        lock(&self.inner.state).clone()
    }

    /// 当前 system prompt。
    pub fn system_prompt(&self) -> String {
        lock(&self.inner.state).system_prompt.clone()
    }

    pub fn set_system_prompt(&self, system_prompt: impl Into<String>) {
        lock(&self.inner.state).system_prompt = system_prompt.into();
    }

    pub fn model(&self) -> Model {
        lock(&self.inner.state).model.clone()
    }

    pub fn set_model(&self, model: Model) {
        lock(&self.inner.state).model = model;
    }

    pub fn thinking_level(&self) -> ModelThinkingLevel {
        lock(&self.inner.state).thinking_level
    }

    pub fn set_thinking_level(&self, thinking_level: ModelThinkingLevel) {
        lock(&self.inner.state).thinking_level = thinking_level;
    }

    pub fn tools(&self) -> Vec<crate::agent::types::AgentTool> {
        lock(&self.inner.state).tools.clone()
    }

    pub fn set_tools(&self, tools: Vec<crate::agent::types::AgentTool>) {
        lock(&self.inner.state).tools = tools;
    }

    pub fn messages(&self) -> Vec<AgentMessage> {
        lock(&self.inner.state).messages.clone()
    }

    pub fn set_messages(&self, messages: Vec<AgentMessage>) {
        lock(&self.inner.state).messages = messages;
    }

    /// 当前流函数。
    pub fn stream_fn(&self) -> StreamFn {
        lock(&self.inner.stream_function).clone()
    }

    pub fn set_stream_fn(&self, stream_fn: StreamFn) {
        *lock(&self.inner.stream_function) = stream_fn;
    }

    /// steering 队列 drain 模式。
    pub fn steering_mode(&self) -> QueueMode {
        self.inner.steering_queue.lock().unwrap().mode
    }

    pub fn set_steering_mode(&self, mode: QueueMode) {
        self.inner.steering_queue.lock().unwrap().mode = mode;
    }

    /// follow-up 队列 drain 模式。
    pub fn follow_up_mode(&self) -> QueueMode {
        self.inner.follow_up_queue.lock().unwrap().mode
    }

    pub fn set_follow_up_mode(&self, mode: QueueMode) {
        self.inner.follow_up_queue.lock().unwrap().mode = mode;
    }

    /// 入队一条消息,在当前 assistant 回合结束后注入。
    pub fn steer(&self, message: AgentMessage) {
        self.inner.steering_queue.lock().unwrap().enqueue(message);
    }

    /// 入队一条消息,仅在 agent 本应停止时运行。
    pub fn follow_up(&self, message: AgentMessage) {
        self.inner.follow_up_queue.lock().unwrap().enqueue(message);
    }

    /// 清空 steering 队列。
    pub fn clear_steering_queue(&self) {
        self.inner.steering_queue.lock().unwrap().clear();
    }

    /// 清空 follow-up 队列。
    pub fn clear_follow_up_queue(&self) {
        self.inner.follow_up_queue.lock().unwrap().clear();
    }

    /// 清空全部队列。
    pub fn clear_all_queues(&self) {
        self.clear_steering_queue();
        self.clear_follow_up_queue();
    }

    /// 任一队列仍有待处理消息时为 true。
    pub fn has_queued_messages(&self) -> bool {
        self.inner.steering_queue.lock().unwrap().has_items()
            || self.inner.follow_up_queue.lock().unwrap().has_items()
    }

    /// 当前 run 的 abort signal(无运行时 None)。
    pub fn signal(&self) -> Option<AbortSignal> {
        lock(&self.inner.active_run)
            .as_ref()
            .map(|run| run.abort.clone())
    }

    /// 中止当前运行(无运行时空操作)。
    pub fn abort(&self) {
        if let Some(run) = lock(&self.inner.active_run).as_ref() {
            run.abort.cancel();
        }
    }

    /// 等待当前 run 与全部 agent_end listeners 完成(对齐 TS `waitForIdle`)。
    pub async fn wait_for_idle(&self) {
        let mut done = lock(&self.inner.active_run)
            .as_ref()
            .map(|run| run.done_rx.clone());
        if let Some(done) = done.as_mut() {
            while !*done.borrow_and_update() {
                if done.changed().await.is_err() {
                    break;
                }
            }
        }
    }

    /// 清空 transcript、运行状态与队列;运行中返回 Err(对齐 TS throw)。
    pub fn reset(&self) -> Result<(), String> {
        if lock(&self.inner.active_run).is_some() {
            return Err(
                "Agent is already processing. Wait for completion before resetting.".to_string(),
            );
        }
        {
            let mut state = lock(&self.inner.state);
            state.messages.clear();
            state.is_streaming = false;
            state.streaming_message = None;
            state.pending_tool_calls.clear();
            state.error_message = None;
        }
        self.clear_follow_up_queue();
        self.clear_steering_queue();
        Ok(())
    }

    /// 启动新 prompt(文本 / 单条消息 / 批量消息);忙时 Err。
    pub async fn prompt(&self, message: impl Into<PromptInput>) -> Result<(), String> {
        if lock(&self.inner.active_run).is_some() {
            return Err(
                "Agent is already processing a prompt. Use steer() or followUp() to queue messages, or wait for completion."
                    .to_string(),
            );
        }
        match message.into() {
            PromptInput::Text(text) => {
                self.run_prompt_messages(vec![self.text_user_message(text, Vec::new())], false)
                    .await
            }
            PromptInput::Messages(messages) => self.run_prompt_messages(messages, false).await,
        }
    }

    /// 文本 + 可选图片的 prompt 便捷入口。
    pub async fn prompt_text(
        &self,
        text: impl Into<String>,
        images: Vec<TextOrImageContent>,
    ) -> Result<(), String> {
        if lock(&self.inner.active_run).is_some() {
            return Err(
                "Agent is already processing a prompt. Use steer() or followUp() to queue messages, or wait for completion."
                    .to_string(),
            );
        }
        let message = self.text_user_message(text.into(), images);
        self.run_prompt_messages(vec![message], false).await
    }

    /// 从当前 transcript 续跑;末尾 assistant 时先 drain 队列,否则直接续跑。
    pub async fn continue_run(&self) -> Result<(), String> {
        if lock(&self.inner.active_run).is_some() {
            return Err(
                "Agent is already processing. Wait for completion before continuing.".to_string(),
            );
        }

        let last_is_assistant = {
            let state = lock(&self.inner.state);
            match state.messages.last() {
                None => return Err("No messages to continue from".to_string()),
                Some(message) => {
                    matches!(message, AgentMessage::Message(TypedMessage::Assistant(_)))
                }
            }
        };

        if last_is_assistant {
            let queued_steering = self.inner.steering_queue.lock().unwrap().drain();
            if !queued_steering.is_empty() {
                return self.run_prompt_messages(queued_steering, true).await;
            }
            let queued_follow_ups = self.inner.follow_up_queue.lock().unwrap().drain();
            if !queued_follow_ups.is_empty() {
                return self.run_prompt_messages(queued_follow_ups, false).await;
            }
            return Err("Cannot continue from message role: assistant".to_string());
        }

        self.run_continuation().await
    }

    fn text_user_message(&self, text: String, images: Vec<TextOrImageContent>) -> AgentMessage {
        let mut content = vec![TextOrImageContent::text(text)];
        content.extend(images);
        AgentMessage::Message(TypedMessage::User(UserMessage {
            role: "user".to_string(),
            content: UserContent::Blocks(content),
            timestamp: now_ms(),
        }))
    }

    async fn run_prompt_messages(
        &self,
        messages: Vec<AgentMessage>,
        skip_initial_steering_poll: bool,
    ) -> Result<(), String> {
        self.run_with_life_cycle(move |inner, signal| {
            Box::pin(async move {
                let context = inner.create_context_snapshot();
                let config = inner.build_run_config(skip_initial_steering_poll);
                let emit = inner.emit_sink();
                let stream_fn = lock(&inner.stream_function).clone();
                run_agent_loop(messages, context, config, emit, Some(signal), stream_fn).await;
                Ok(())
            })
        })
        .await
    }

    async fn run_continuation(&self) -> Result<(), String> {
        self.run_with_life_cycle(|inner, signal| {
            Box::pin(async move {
                let context = inner.create_context_snapshot();
                let config = inner.build_run_config(false);
                let emit = inner.emit_sink();
                let stream_fn = lock(&inner.stream_function).clone();
                run_agent_loop_continue(context, config, emit, Some(signal), stream_fn).await?;
                Ok(())
            })
        })
        .await
    }

    async fn run_with_life_cycle(
        &self,
        executor: impl FnOnce(Arc<AgentInner>, AbortSignal) -> BoxFuture<'static, Result<(), String>>,
    ) -> Result<(), String> {
        let signal = {
            let mut active = lock(&self.inner.active_run);
            if active.is_some() {
                return Err("Agent is already processing.".to_string());
            }
            let (done_tx, done_rx) = watch::channel(false);
            let abort = CancellationToken::new();
            *active = Some(ActiveRun {
                abort: abort.clone(),
                done_tx,
                done_rx,
            });
            abort
        };
        {
            let mut state = lock(&self.inner.state);
            state.is_streaming = true;
            state.streaming_message = None;
            state.error_message = None;
        }

        let outcome = executor(self.inner.clone(), signal.clone()).await;
        if let Err(error) = outcome {
            self.inner
                .handle_run_failure(error, signal.is_cancelled())
                .await;
        }
        self.inner.finish_run();
        Ok(())
    }
}

/// 将 on_payload/on_response 回调装箱进 SimpleStreamOptions(供应用侧构造 config 用)。
pub fn wrap_on_payload_callback(callback: &OnPayloadCallback) -> OnPayloadFn {
    let callback = callback.clone();
    Box::new(move |value: Value| {
        let callback = callback.clone();
        Box::pin(async move { callback(value).await })
    })
}

/// 将 on_response 回调装箱进 SimpleStreamOptions(供应用侧构造 config 用)。
pub fn wrap_on_response_callback(callback: &OnResponseCallback) -> OnResponseFn {
    let callback = callback.clone();
    Box::new(move |response: &ProviderResponse| callback(response))
}

/// SimpleStreamOptions 便捷构造:payload/响应观测回调按 Arc 装箱。
pub fn stream_options_with_callbacks(
    base: SimpleStreamOptions,
    on_payload: Option<OnPayloadCallback>,
    on_response: Option<OnResponseCallback>,
) -> SimpleStreamOptions {
    let mut options = base;
    options.on_payload = on_payload.as_ref().map(wrap_on_payload_callback);
    options.on_response = on_response.as_ref().map(wrap_on_response_callback);
    options
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::agent_loop::testing::{
        error_script, scripted_stream_fn, test_assistant, test_loop_config, test_model,
        text_script, user_message,
    };
    use crate::agent::llm::event_stream::event_stream;
    use crate::agent::llm::{AssistantMessageEvent, AssistantMessageEventStream};

    fn test_state() -> AgentState {
        AgentState {
            system_prompt: String::new(),
            model: test_model(),
            thinking_level: ModelThinkingLevel::Off,
            tools: Vec::new(),
            messages: Vec::new(),
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: HashSet::new(),
            error_message: None,
        }
    }

    fn test_agent(stream_fn: StreamFn) -> Agent {
        Agent::new(test_state(), test_loop_config(test_model()), stream_fn)
    }

    /// 直构单次文本响应的 AssistantMessageEventStream(gate 测试用)。
    fn text_response_stream(text: &str) -> AssistantMessageEventStream {
        let (stream, writer) = event_stream();
        writer.push(AssistantMessageEvent::Start {
            partial: test_assistant(vec![], StopReason::Pending),
        });
        writer.push(AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: text.to_string(),
            partial: test_assistant(vec![AssistantContent::text(text)], StopReason::Pending),
        });
        let final_message = test_assistant(vec![AssistantContent::text(text)], StopReason::Stop);
        writer.push(AssistantMessageEvent::Done {
            reason: StopReason::Stop,
            message: final_message.clone(),
        });
        writer.end(final_message);
        stream
    }

    #[tokio::test]
    async fn agent_prompt_accumulates_state() {
        let (stream_fn, _calls) = scripted_stream_fn(vec![text_script("hello")]);
        let agent = test_agent(stream_fn);

        // 文本入口。
        agent.prompt_text("hi", vec![]).await.unwrap();

        let state = agent.state();
        assert!(!state.is_streaming);
        assert!(state.streaming_message.is_none());
        assert!(state.pending_tool_calls.is_empty());
        assert_eq!(state.messages.len(), 2);
        assert_eq!(state.messages[0].role_name(), "user");
        assert_eq!(state.messages[1].role_name(), "assistant");
        // run 已结束,waitForIdle 立即解决。
        agent.wait_for_idle().await;

        // 单条消息入口(PromptInput::From<AgentMessage>)与批量入口。
        let (stream_fn, _calls) = scripted_stream_fn(vec![text_script("a"), text_script("b")]);
        let agent = test_agent(stream_fn);
        agent.prompt(user_message("one", 1)).await.unwrap();
        agent
            .prompt(vec![user_message("two", 2), user_message("three", 3)])
            .await
            .unwrap();
        // one + a + two + three + b
        assert_eq!(agent.messages().len(), 5);
    }

    #[tokio::test]
    async fn prompt_while_busy_returns_err() {
        let gate = Arc::new(tokio::sync::Notify::new());
        let stream_fn: StreamFn = {
            let gate = gate.clone();
            Arc::new(move |_model, _context, _options| {
                let gate = gate.clone();
                Box::pin(async move {
                    gate.notified().await;
                    text_response_stream("hi")
                })
            })
        };
        let agent = test_agent(stream_fn);

        let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
        {
            let started_tx = Arc::new(Mutex::new(Some(started_tx)));
            agent.subscribe(Arc::new(move |event, _signal| {
                let started_tx = started_tx.clone();
                Box::pin(async move {
                    if matches!(event, AgentEvent::AgentStart) {
                        if let Some(started_tx) = started_tx.lock().unwrap().take() {
                            let _ = started_tx.send(());
                        }
                    }
                })
            }));
        }

        let runner = {
            let agent = agent.clone();
            tokio::spawn(async move { agent.prompt_text("hi", vec![]).await })
        };
        started_rx.await.unwrap();

        // 忙时 prompt 与 prompt_text 都报错,消息与蓝本一致。
        let error = agent.prompt(user_message("second", 2)).await.unwrap_err();
        assert!(error.contains("already processing a prompt"));
        let error = agent.prompt_text("second", vec![]).await.unwrap_err();
        assert!(error.contains("already processing a prompt"));
        // 忙时 reset 也报错。
        let error = agent.reset().unwrap_err();
        assert!(error.contains("already processing"));

        gate.notify_one();
        runner.await.unwrap().unwrap();
        agent.wait_for_idle().await;
        assert!(!agent.state().is_streaming);
    }

    #[tokio::test]
    async fn reset_after_run_clears_state_and_queues() {
        let (stream_fn, _calls) = scripted_stream_fn(vec![text_script("hello")]);
        let agent = test_agent(stream_fn);
        agent.prompt_text("hi", vec![]).await.unwrap();
        assert!(!agent.messages().is_empty());
        // 运行结束后入队,reset 应连同队列一起清空。
        agent.steer(user_message("queued", 1));
        assert!(agent.has_queued_messages());

        agent.reset().unwrap();
        assert!(agent.messages().is_empty());
        assert!(!agent.has_queued_messages());
        assert!(agent.state().streaming_message.is_none());
        assert!(agent.state().error_message.is_none());
    }

    #[tokio::test]
    async fn steering_queue_mode_all_drains_everything_at_once() {
        let (stream_fn, calls) = scripted_stream_fn(vec![text_script("hello")]);
        let agent = test_agent(stream_fn);
        agent.set_steering_mode(QueueMode::All);
        agent.steer(user_message("s1", 1));
        agent.steer(user_message("s2", 2));

        agent.prompt_text("hi", vec![]).await.unwrap();

        let captured = calls.lock().unwrap();
        assert_eq!(captured.len(), 1);
        // All 模式:两条 steering 都在第一次 LLM 调用前注入。
        assert_eq!(captured[0].context.messages.len(), 3);
        assert!(captured[0]
            .context
            .messages
            .iter()
            .all(|m| matches!(m, crate::agent::llm::Message::User(_))));
        assert_eq!(agent.messages().len(), 4);
    }

    #[tokio::test]
    async fn steering_queue_mode_one_at_a_time_drains_one_per_point() {
        let (stream_fn, calls) = scripted_stream_fn(vec![text_script("a"), text_script("b")]);
        let agent = test_agent(stream_fn);
        // 默认 OneAtATime。
        assert_eq!(agent.steering_mode(), QueueMode::OneAtATime);
        agent.steer(user_message("s1", 1));
        agent.steer(user_message("s2", 2));

        agent.prompt_text("hi", vec![]).await.unwrap();

        let captured = calls.lock().unwrap();
        assert_eq!(captured.len(), 2);
        // 第 1 次调用只有一条 steering;第 2 次调用前注入第二条。
        assert_eq!(captured[0].context.messages.len(), 2);
        assert_eq!(captured[1].context.messages.len(), 4);
        assert!(matches!(
            captured[1].context.messages[3],
            crate::agent::llm::Message::User(_)
        ));
    }

    #[tokio::test]
    async fn continue_from_user_message_reruns_llm_without_new_prompt() {
        let (stream_fn, calls) = scripted_stream_fn(vec![text_script("one"), text_script("two")]);
        let mut state = test_state();
        state.messages = vec![user_message("hi", 1)];
        let agent = Agent::new(state, test_loop_config(test_model()), stream_fn);

        agent.continue_run().await.unwrap();

        let captured = calls.lock().unwrap();
        // 续跑只发起一次 LLM 调用(无工具/steering/follow-up)。
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].context.messages.len(), 1);
        assert_eq!(agent.messages().len(), 2); // user + assistant
    }

    #[tokio::test]
    async fn continue_from_assistant_drains_steering_with_skip_initial_poll() {
        let (stream_fn, calls) = scripted_stream_fn(vec![text_script("one"), text_script("two")]);
        let agent = test_agent(stream_fn);
        agent.prompt_text("hi", vec![]).await.unwrap();
        agent.steer(user_message("s1", 2));

        agent.continue_run().await.unwrap();

        // steering 作为新 prompt 消息运行,且首轮 steering poll 被跳过。
        let captured = calls.lock().unwrap();
        assert_eq!(captured.len(), 2);
        assert_eq!(captured[1].context.messages.len(), 3);
        assert!(matches!(
            captured[1].context.messages[2],
            crate::agent::llm::Message::User(_)
        ));
        assert_eq!(agent.messages().len(), 4);
        assert!(!agent.has_queued_messages());
    }

    #[tokio::test]
    async fn continue_from_assistant_drains_follow_up_when_no_steering() {
        let (stream_fn, calls) = scripted_stream_fn(vec![text_script("one"), text_script("two")]);
        let agent = test_agent(stream_fn);
        agent.prompt_text("hi", vec![]).await.unwrap();
        agent.follow_up(user_message("f1", 2));

        agent.continue_run().await.unwrap();

        let captured = calls.lock().unwrap();
        assert_eq!(captured.len(), 2);
        assert_eq!(captured[1].context.messages.len(), 3);
        assert!(matches!(
            captured[1].context.messages[2],
            crate::agent::llm::Message::User(_)
        ));
    }

    #[tokio::test]
    async fn continue_errors_without_messages_or_queue_drain() {
        let (stream_fn, _calls) = scripted_stream_fn(vec![]);
        let agent = test_agent(stream_fn);
        assert_eq!(
            agent.continue_run().await.unwrap_err(),
            "No messages to continue from"
        );

        // 末尾 assistant 且无队列可 drain。
        let (stream_fn, _calls) = scripted_stream_fn(vec![text_script("one")]);
        let agent = test_agent(stream_fn);
        agent.prompt_text("hi", vec![]).await.unwrap();
        assert_eq!(
            agent.continue_run().await.unwrap_err(),
            "Cannot continue from message role: assistant"
        );
    }

    #[tokio::test]
    async fn subscribe_and_unsubscribe_controls_listener_delivery() {
        let (stream_fn, _calls) = scripted_stream_fn(vec![text_script("a"), text_script("b")]);
        let agent = test_agent(stream_fn);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let id = {
            let seen = seen.clone();
            agent.subscribe(Arc::new(move |event, _signal| {
                let seen = seen.clone();
                Box::pin(async move {
                    seen.lock().unwrap().push(event);
                })
            }))
        };

        agent.prompt_text("first", vec![]).await.unwrap();
        let after_first = seen.lock().unwrap().len();
        assert!(after_first > 0);

        agent.unsubscribe(id);
        agent.prompt_text("second", vec![]).await.unwrap();
        assert_eq!(seen.lock().unwrap().len(), after_first);
    }

    #[tokio::test]
    async fn listeners_receive_current_run_abort_signal() {
        let (stream_fn, _calls) = scripted_stream_fn(vec![text_script("a")]);
        let agent = test_agent(stream_fn);
        let signals = Arc::new(Mutex::new(Vec::new()));
        {
            let signals = signals.clone();
            agent.subscribe(Arc::new(move |_event, signal| {
                let signals = signals.clone();
                Box::pin(async move {
                    signals.lock().unwrap().push(signal.clone());
                })
            }));
        }
        agent.prompt_text("hi", vec![]).await.unwrap();
        let signals = signals.lock().unwrap();
        assert!(!signals.is_empty());
        // 全部事件收到的是同一(当前 run)的 signal。
        assert!(signals.iter().all(|signal| !signal.is_cancelled()));
    }

    #[tokio::test]
    async fn state_snapshot_is_a_clone_and_setters_round_trip() {
        let (stream_fn, _calls) = scripted_stream_fn(vec![text_script("a")]);
        let agent = test_agent(stream_fn);
        agent.set_system_prompt("sys");
        agent.set_thinking_level(ModelThinkingLevel::High);
        assert_eq!(agent.system_prompt(), "sys");
        assert_eq!(agent.thinking_level(), ModelThinkingLevel::High);
        assert_eq!(agent.model().id, "test-model");
        agent.set_model(test_model());
        agent.wait_for_idle().await;
    }

    #[tokio::test]
    async fn error_stream_sets_error_message_state() {
        let (stream_fn, _calls) = scripted_stream_fn(vec![error_script(StopReason::Error, "boom")]);
        let agent = test_agent(stream_fn);
        agent.prompt_text("hi", vec![]).await.unwrap();

        let state = agent.state();
        assert_eq!(state.error_message.as_deref(), Some("boom"));
        // 失败 assistant 消息仍进入 transcript。
        match state.messages.last().unwrap() {
            AgentMessage::Message(TypedMessage::Assistant(assistant)) => {
                assert_eq!(assistant.stop_reason, StopReason::Error);
                assert_eq!(assistant.error_message.as_deref(), Some("boom"));
            }
            other => panic!("expected assistant message, got {other:?}"),
        }
    }
}
