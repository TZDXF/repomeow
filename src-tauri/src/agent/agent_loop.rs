//! 低层 agent 循环:对齐 `packages/agent/src/agent-loop.ts`(pi-agent-core 0.84.4)。
//!
//! 蓝本语义要点(逐条对齐):
//! - 外循环(follow-up)+ 内循环(tool calls / steering);内循环条件在回合末评估
//!   (`hasMoreToolCalls || pendingMessages.length > 0`)。
//! - steering 在循环开始 poll 一次;`prepareNextTurn` 之后仅当之前 poll 为空再 poll 一次。
//! - `prepareNextTurn` 只在 `lastCompletedTurn` 存在时调用,可替换 context/model/
//!   thinkingLevel(thinkingLevel 映射:off → reasoning None)。
//! - `streamAssistantResponse`:transformContext → convertToLlm → 构造 LLM Context
//!   (tools 经 `as_llm_tool`)→ resolvedApiKey 覆盖 options.api_key → streamFn;
//!   start 事件把 partial 推入 context.messages,后续增量事件原地替换最后一条;
//!   done/error 用 `response.result()` 的最终消息替换/补推,并发 message_end。
//! - stopReason error/aborted → turn_end(空 toolResults)+ agent_end 直接返回;
//!   stopReason length → 全部 toolCall 按截断错误失败(terminate: false)。
//! - 工具批次:任一工具 execution_mode=Sequential 或 config.tool_execution=Sequential
//!   → sequential,否则 parallel。parallel 先逐个 start+prepare(prepared 的并发执行),
//!   tool_execution_end 按完成顺序、toolResult 消息事件按 assistant 源顺序。
//! - prepareToolCall:工具未找到 → immediate error;prepareArguments →
//!   validateToolArguments(Err → immediate error);beforeToolCall 先查 aborted 再查
//!   block;最终 aborted → "Operation aborted"。
//! - terminate 语义:整批全部 terminate 才提前终止。

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use futures::future::{join_all, BoxFuture};
use futures::StreamExt;
use serde_json::Value;

use crate::agent::llm::event_stream::{event_stream, EventStream, EventStreamWriter};
use crate::agent::llm::validate::validate_tool_arguments;
use crate::agent::llm::{
    AssistantContent, AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream,
    Context as LlmContext, ModelThinkingLevel, StopReason, ThinkingLevel, TextOrImageContent,
    ToolCall, ToolResultMessage,
};
use crate::agent::types::{
    AbortSignal, AfterToolCallContext, AgentContext, AgentEvent, AgentEventSink, AgentLoopConfig,
    AgentLoopTurnUpdate, AgentMessage, AgentTool, AgentToolResult, AgentToolUpdateCallback,
    BeforeToolCallContext, ShouldStopAfterTurnContext, StreamFn, ToolExecutionMode, TypedMessage,
};

// ---------------------------------------------------------------------------
// 顶层入口:spawn 版 EventStream + 直接 async 版
// ---------------------------------------------------------------------------

/// 供 spawn 任务共享的流写入端(push 并发安全,end 恰好一次)。
struct SharedWriter<E, R> {
    inner: Mutex<Option<EventStreamWriter<E, R>>>,
}

impl<E, R> SharedWriter<E, R> {
    fn push(&self, event: E) {
        if let Some(writer) = self.inner.lock().unwrap().as_ref() {
            writer.push(event);
        }
    }

    fn end(&self, result: R) {
        if let Some(writer) = self.inner.lock().unwrap().take() {
            writer.end(result);
        }
    }
}

fn shared_emit<E: Send + 'static, R: Send + 'static>(
    writer: &Arc<SharedWriter<E, R>>,
) -> Arc<dyn Fn(E) -> BoxFuture<'static, ()> + Send + Sync> {
    let writer = writer.clone();
    Arc::new(move |event| {
        let writer = writer.clone();
        Box::pin(async move { writer.push(event) })
    })
}

/// 启动 agent 循环(对齐 TS `agentLoop`):prompt 加入 context,事件推入返回的流。
pub fn agent_loop(
    prompts: Vec<AgentMessage>,
    context: AgentContext,
    config: AgentLoopConfig,
    signal: Option<AbortSignal>,
    stream_fn: StreamFn,
) -> EventStream<AgentEvent, Vec<AgentMessage>> {
    let (stream, writer) = event_stream::<AgentEvent, Vec<AgentMessage>>();
    let writer = Arc::new(SharedWriter {
        inner: Mutex::new(Some(writer)),
    });
    let emit = shared_emit(&writer);
    tokio::spawn(async move {
        let messages = run_agent_loop(prompts, context, config, emit, signal, stream_fn).await;
        writer.end(messages);
    });
    stream
}

/// 从当前 context 续跑 agent 循环(对齐 TS `agentLoopContinue`)。
/// 前置校验(空 context / 末尾 assistant)对齐 TS throw,在 spawn 前同步返回 Err。
pub fn agent_loop_continue(
    context: AgentContext,
    config: AgentLoopConfig,
    signal: Option<AbortSignal>,
    stream_fn: StreamFn,
) -> Result<EventStream<AgentEvent, Vec<AgentMessage>>, String> {
    check_continue_preconditions(&context)?;
    let (stream, writer) = event_stream::<AgentEvent, Vec<AgentMessage>>();
    let writer = Arc::new(SharedWriter {
        inner: Mutex::new(Some(writer)),
    });
    let emit = shared_emit(&writer);
    tokio::spawn(async move {
        // 前置校验已通过,Err 分支按流契约以空消息列表收尾兜底。
        let messages = run_agent_loop_continue(context, config, emit, signal, stream_fn)
            .await
            .unwrap_or_default();
        writer.end(messages);
    });
    Ok(stream)
}

// ---------------------------------------------------------------------------
// 直接 async 版(TS runAgentLoop / runAgentLoopContinue)
// ---------------------------------------------------------------------------

/// 启动带新 prompt 消息的 agent 循环;prompt 加入 context 并发出消息事件。
pub async fn run_agent_loop(
    prompts: Vec<AgentMessage>,
    context: AgentContext,
    config: AgentLoopConfig,
    emit: AgentEventSink,
    signal: Option<AbortSignal>,
    stream_fn: StreamFn,
) -> Vec<AgentMessage> {
    let new_messages = prompts.clone();
    let mut current_context = context;
    current_context.messages.extend(prompts);

    emit_event(&emit, AgentEvent::AgentStart).await;
    emit_event(&emit, AgentEvent::TurnStart).await;
    for prompt in &new_messages {
        emit_event(
            &emit,
            AgentEvent::MessageStart {
                message: prompt.clone(),
            },
        )
        .await;
        emit_event(
            &emit,
            AgentEvent::MessageEnd {
                message: prompt.clone(),
            },
        )
        .await;
    }

    run_loop(
        current_context,
        new_messages,
        config,
        signal,
        &emit,
        stream_fn,
    )
    .await
}

/// 从当前 context 续跑(重试场景:context 末尾已是 user/toolResult 消息)。
/// 前置校验失败返回 Err(对齐 TS runAgentLoopContinue 的 throw)。
pub async fn run_agent_loop_continue(
    context: AgentContext,
    config: AgentLoopConfig,
    emit: AgentEventSink,
    signal: Option<AbortSignal>,
    stream_fn: StreamFn,
) -> Result<Vec<AgentMessage>, String> {
    check_continue_preconditions(&context)?;

    let new_messages: Vec<AgentMessage> = Vec::new();
    let current_context = context;

    emit_event(&emit, AgentEvent::AgentStart).await;
    emit_event(&emit, AgentEvent::TurnStart).await;

    Ok(run_loop(
        current_context,
        new_messages,
        config,
        signal,
        &emit,
        stream_fn,
    )
    .await)
}

fn check_continue_preconditions(context: &AgentContext) -> Result<(), String> {
    if context.messages.is_empty() {
        return Err("Cannot continue: no messages in context".to_string());
    }
    if matches!(
        context.messages.last(),
        Some(AgentMessage::Message(TypedMessage::Assistant(_)))
    ) {
        return Err("Cannot continue from message role: assistant".to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 主循环
// ---------------------------------------------------------------------------

/// 主循环逻辑(agentLoop 与 agentLoopContinue 共用)。
async fn run_loop(
    mut current_context: AgentContext,
    mut new_messages: Vec<AgentMessage>,
    mut config: AgentLoopConfig,
    signal: Option<AbortSignal>,
    emit: &AgentEventSink,
    stream_fn: StreamFn,
) -> Vec<AgentMessage> {
    let mut last_completed_turn: Option<ShouldStopAfterTurnContext> = None;
    // 循环开始时检查 steering(用户可能在等待期间输入)。
    let mut pending_messages = poll_steering(&config).await;

    // 外循环:agent 即将停止时如有 follow-up 消息则继续。
    'outer: loop {
        // TS 蓝本初值为 true,但首读前必然被回合一内的赋值覆盖;Rust 侧用
        // 未初始化声明让编译器验证赋值先于读取。
        let mut has_more_tool_calls;

        // 内循环:处理工具调用与 steering 消息。
        loop {
            if let Some(turn) = last_completed_turn.as_ref() {
                if let Some(prepare) = config.prepare_next_turn.clone() {
                    if let Some(update) = prepare(clone_turn_context(turn)).await {
                        apply_turn_update(&mut current_context, &mut config, update);
                    }
                }
                // 准备可能耗时(如 compaction),补捞期间入队的 steering;
                // 仅当之前 poll 为空再 poll,避免 one-at-a-time 模式一回合注入两条。
                if pending_messages.is_empty() {
                    pending_messages = poll_steering(&config).await;
                }
                emit_event(emit, AgentEvent::TurnStart).await;
            }

            // 在下一次 assistant 响应前注入 pending 消息。
            if !pending_messages.is_empty() {
                for message in std::mem::take(&mut pending_messages) {
                    emit_event(
                        emit,
                        AgentEvent::MessageStart {
                            message: message.clone(),
                        },
                    )
                    .await;
                    emit_event(
                        emit,
                        AgentEvent::MessageEnd {
                            message: message.clone(),
                        },
                    )
                    .await;
                    current_context.messages.push(message.clone());
                    new_messages.push(message);
                }
            }

            // 流式 assistant 响应。
            let message = stream_assistant_response(
                &mut current_context,
                &config,
                signal.as_ref(),
                emit,
                &stream_fn,
            )
            .await;
            new_messages.push(assistant_message_of(message.clone()));

            if matches!(
                message.stop_reason,
                StopReason::Error | StopReason::Aborted
            ) {
                emit_event(
                    emit,
                    AgentEvent::TurnEnd {
                        message: assistant_message_of(message),
                        tool_results: Vec::new(),
                    },
                )
                .await;
                emit_event(
                    emit,
                    AgentEvent::AgentEnd {
                        messages: new_messages.clone(),
                    },
                )
                .await;
                return new_messages;
            }

            // 工具调用处理。
            let tool_calls = message_tool_calls(&message);
            let mut tool_results: Vec<ToolResultMessage> = Vec::new();
            has_more_tool_calls = false;
            if !tool_calls.is_empty() {
                // stopReason "length" 表示输出被 token 上限截断,所有 toolCall 的
                // 参数都可能不完整:全部按错误失败,而不是执行可能损坏的调用。
                let batch = if message.stop_reason == StopReason::Length {
                    fail_tool_calls_from_truncated_message(&tool_calls, emit).await
                } else {
                    execute_tool_calls(&current_context, &message, &config, signal.as_ref(), emit)
                        .await
                };
                tool_results = batch.messages;
                has_more_tool_calls = !batch.terminate;

                for result in &tool_results {
                    current_context
                        .messages
                        .push(tool_result_message_of(result.clone()));
                    new_messages.push(tool_result_message_of(result.clone()));
                }
            }

            emit_event(
                emit,
                AgentEvent::TurnEnd {
                    message: assistant_message_of(message.clone()),
                    tool_results: tool_results.clone(),
                },
            )
            .await;

            let turn = ShouldStopAfterTurnContext {
                message: message.clone(),
                tool_results: tool_results.clone(),
                context: current_context.clone(),
                new_messages: new_messages.clone(),
            };
            if let Some(should_stop) = &config.should_stop_after_turn {
                if should_stop(clone_turn_context(&turn)).await {
                    emit_event(
                        emit,
                        AgentEvent::AgentEnd {
                            messages: new_messages.clone(),
                        },
                    )
                    .await;
                    return new_messages;
                }
            }
            last_completed_turn = Some(turn);

            pending_messages = poll_steering(&config).await;
            if !(has_more_tool_calls || !pending_messages.is_empty()) {
                break;
            }
        }

        // agent 本应停止:检查 follow-up 消息。
        let follow_up_messages = poll_follow_up(&config).await;
        if !follow_up_messages.is_empty() {
            pending_messages = follow_up_messages;
            continue 'outer;
        }

        break;
    }

    emit_event(
        emit,
        AgentEvent::AgentEnd {
            messages: new_messages.clone(),
        },
    )
    .await;
    new_messages
}

fn apply_turn_update(
    current_context: &mut AgentContext,
    config: &mut AgentLoopConfig,
    update: AgentLoopTurnUpdate,
) {
    if let Some(context) = update.context {
        *current_context = context;
    }
    if let Some(model) = update.model {
        config.model = model;
    }
    if let Some(level) = update.thinking_level {
        config.stream.reasoning = reasoning_from_thinking_level(level);
    }
}

/// agent 层 thinking 级别 → LLM 层 reasoning 选项(off → None)。
pub(crate) fn reasoning_from_thinking_level(level: ModelThinkingLevel) -> Option<ThinkingLevel> {
    match level {
        ModelThinkingLevel::Off => None,
        ModelThinkingLevel::Minimal => Some(ThinkingLevel::Minimal),
        ModelThinkingLevel::Low => Some(ThinkingLevel::Low),
        ModelThinkingLevel::Medium => Some(ThinkingLevel::Medium),
        ModelThinkingLevel::High => Some(ThinkingLevel::High),
        ModelThinkingLevel::Xhigh => Some(ThinkingLevel::Xhigh),
        ModelThinkingLevel::Max => Some(ThinkingLevel::Max),
    }
}

async fn poll_steering(config: &AgentLoopConfig) -> Vec<AgentMessage> {
    match &config.get_steering_messages {
        Some(get) => get().await,
        None => Vec::new(),
    }
}

async fn poll_follow_up(config: &AgentLoopConfig) -> Vec<AgentMessage> {
    match &config.get_follow_up_messages {
        Some(get) => get().await,
        None => Vec::new(),
    }
}

fn clone_turn_context(turn: &ShouldStopAfterTurnContext) -> ShouldStopAfterTurnContext {
    ShouldStopAfterTurnContext {
        message: turn.message.clone(),
        tool_results: turn.tool_results.clone(),
        context: turn.context.clone(),
        new_messages: turn.new_messages.clone(),
    }
}

// ---------------------------------------------------------------------------
// assistant 响应流
// ---------------------------------------------------------------------------

/// 从 LLM 流式获取 assistant 响应(AgentMessage → Message 转换只发生在 LLM 调用边界)。
async fn stream_assistant_response(
    context: &mut AgentContext,
    config: &AgentLoopConfig,
    signal: Option<&AbortSignal>,
    emit: &AgentEventSink,
    stream_fn: &StreamFn,
) -> AssistantMessage {
    // 先应用 AgentMessage 级上下文变换(窗口修剪/外部注入)。
    let mut messages = context.messages.clone();
    if let Some(transform) = &config.transform_context {
        messages = transform(messages, signal.cloned()).await;
    }

    // 转换为 LLM 兼容消息。
    let llm_messages = (config.convert_to_llm)(messages).await;

    let llm_context = LlmContext {
        system_prompt: Some(context.system_prompt.clone()),
        messages: llm_messages,
        tools: context.tools.iter().map(|tool| tool.as_llm_tool()).collect(),
    };

    // 动态解析 API key(短时 OAuth token 场景);空串视为未解析(对齐 TS `||` 语义)。
    let resolved_api_key = match &config.get_api_key {
        Some(get) => get(config.model.provider.clone())
            .await
            .filter(|key| !key.is_empty())
            .or_else(|| config.stream.api_key.clone()),
        None => config.stream.api_key.clone(),
    };
    let mut options = config.stream.clone();
    options.api_key = resolved_api_key;

    let mut response = (stream_fn)(config.model.clone(), llm_context, Some(options)).await;

    let mut added_partial = false;
    while let Some(event) = response.next().await {
        match &event {
            AssistantMessageEvent::Start { partial } => {
                let partial = partial.clone();
                context
                    .messages
                    .push(assistant_message_of(partial.clone()));
                added_partial = true;
                emit_event(
                    emit,
                    AgentEvent::MessageStart {
                        message: assistant_message_of(partial),
                    },
                )
                .await;
            }
            AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. } => {
                return finalize_stream_message(&mut response, context, added_partial, emit).await;
            }
            AssistantMessageEvent::TextStart { partial, .. }
            | AssistantMessageEvent::TextDelta { partial, .. }
            | AssistantMessageEvent::TextEnd { partial, .. }
            | AssistantMessageEvent::ThinkingStart { partial, .. }
            | AssistantMessageEvent::ThinkingDelta { partial, .. }
            | AssistantMessageEvent::ThinkingEnd { partial, .. }
            | AssistantMessageEvent::ToolcallStart { partial, .. }
            | AssistantMessageEvent::ToolcallDelta { partial, .. }
            | AssistantMessageEvent::ToolcallEnd { partial, .. } => {
                if added_partial {
                    let partial = partial.clone();
                    *context.messages.last_mut().expect(
                        "partial update event after start must have a partial message in context",
                    ) = assistant_message_of(partial.clone());
                    emit_event(
                        emit,
                        AgentEvent::MessageUpdate {
                            message: assistant_message_of(partial),
                            assistant_message_event: event.clone(),
                        },
                    )
                    .await;
                }
            }
        }
    }

    // 流在 done/error 之前结束(不符合契约的流):仍取终值收尾,对齐蓝本。
    finalize_stream_message(&mut response, context, added_partial, emit).await
}

/// done/error(或流提前结束)后的收尾:最终消息替换/补推进 context 并发 message_end。
async fn finalize_stream_message(
    response: &mut AssistantMessageEventStream,
    context: &mut AgentContext,
    added_partial: bool,
    emit: &AgentEventSink,
) -> AssistantMessage {
    let final_message = response.result().await;
    if added_partial {
        *context.messages.last_mut().expect(
            "partial was added by start event; final message must replace it",
        ) = assistant_message_of(final_message.clone());
    } else {
        context
            .messages
            .push(assistant_message_of(final_message.clone()));
        emit_event(
            emit,
            AgentEvent::MessageStart {
                message: assistant_message_of(final_message.clone()),
            },
        )
        .await;
    }
    emit_event(
        emit,
        AgentEvent::MessageEnd {
            message: assistant_message_of(final_message.clone()),
        },
    )
    .await;
    final_message
}

// ---------------------------------------------------------------------------
// 工具执行
// ---------------------------------------------------------------------------

struct ExecutedToolCallBatch {
    messages: Vec<ToolResultMessage>,
    terminate: bool,
}

struct FinalizedToolCallOutcome {
    tool_call: ToolCall,
    result: AgentToolResult,
    is_error: bool,
}

struct ExecutedToolCallOutcome {
    result: AgentToolResult,
    is_error: bool,
}

struct PreparedToolCall {
    tool_call: ToolCall,
    tool: AgentTool,
    args: Value,
}

enum Preparation {
    Prepared { tool: AgentTool, args: Value },
    Immediate { result: AgentToolResult, is_error: bool },
}

/// stopReason "length":全部 toolCall 按截断错误失败。
/// 流式 tool-call 参数会用尽力 JSON 补救解析定稿,截断消息可能产出"能解析但
/// 不完整"的参数,均不可安全执行;逐个报错让模型重新发起调用。
async fn fail_tool_calls_from_truncated_message(
    tool_calls: &[ToolCall],
    emit: &AgentEventSink,
) -> ExecutedToolCallBatch {
    let mut messages = Vec::new();
    for tool_call in tool_calls {
        emit_event(
            emit,
            AgentEvent::ToolExecutionStart {
                tool_call_id: tool_call.id.clone(),
                tool_name: tool_call.name.clone(),
                args: Value::Object(tool_call.arguments.clone()),
            },
        )
        .await;
        let finalized = FinalizedToolCallOutcome {
            tool_call: tool_call.clone(),
            result: create_error_tool_result(&format!(
                "Tool call \"{}\" was not executed: the response hit the output token limit, so its arguments may be truncated. Re-issue the tool call with complete arguments.",
                tool_call.name
            )),
            is_error: true,
        };
        emit_tool_execution_end(&finalized, emit).await;
        let tool_result_message = create_tool_result_message(&finalized);
        emit_tool_result_message(&tool_result_message, emit).await;
        messages.push(tool_result_message);
    }
    ExecutedToolCallBatch {
        messages,
        terminate: false,
    }
}

/// 执行 assistant 消息中的工具调用:任一工具为 Sequential 或 loop 默认 Sequential
/// 时逐个执行,否则并发执行。
async fn execute_tool_calls(
    current_context: &AgentContext,
    assistant_message: &AssistantMessage,
    config: &AgentLoopConfig,
    signal: Option<&AbortSignal>,
    emit: &AgentEventSink,
) -> ExecutedToolCallBatch {
    let tool_calls = message_tool_calls(assistant_message);
    let has_sequential_tool_call = tool_calls.iter().any(|tool_call| {
        current_context
            .tools
            .iter()
            .find(|tool| tool.name == tool_call.name)
            .and_then(|tool| tool.execution_mode)
            == Some(ToolExecutionMode::Sequential)
    });
    if config.tool_execution == ToolExecutionMode::Sequential || has_sequential_tool_call {
        execute_tool_calls_sequential(current_context, assistant_message, &tool_calls, config, signal, emit)
            .await
    } else {
        execute_tool_calls_parallel(current_context, assistant_message, &tool_calls, config, signal, emit)
            .await
    }
}

async fn execute_tool_calls_sequential(
    current_context: &AgentContext,
    assistant_message: &AssistantMessage,
    tool_calls: &[ToolCall],
    config: &AgentLoopConfig,
    signal: Option<&AbortSignal>,
    emit: &AgentEventSink,
) -> ExecutedToolCallBatch {
    let mut finalized_calls: Vec<FinalizedToolCallOutcome> = Vec::new();
    let mut messages: Vec<ToolResultMessage> = Vec::new();

    for tool_call in tool_calls {
        emit_event(
            emit,
            AgentEvent::ToolExecutionStart {
                tool_call_id: tool_call.id.clone(),
                tool_name: tool_call.name.clone(),
                args: Value::Object(tool_call.arguments.clone()),
            },
        )
        .await;

        let preparation =
            prepare_tool_call(current_context, assistant_message, tool_call, config, signal).await;
        let finalized = match preparation {
            Preparation::Immediate { result, is_error } => FinalizedToolCallOutcome {
                tool_call: tool_call.clone(),
                result,
                is_error,
            },
            Preparation::Prepared { tool, args } => {
                let prepared = PreparedToolCall {
                    tool_call: tool_call.clone(),
                    tool,
                    args,
                };
                let executed = execute_prepared_tool_call(&prepared, signal, emit).await;
                finalize_executed_tool_call(
                    current_context,
                    assistant_message,
                    &prepared,
                    executed,
                    config,
                    signal,
                )
                .await
            }
        };

        emit_tool_execution_end(&finalized, emit).await;
        let tool_result_message = create_tool_result_message(&finalized);
        emit_tool_result_message(&tool_result_message, emit).await;
        finalized_calls.push(finalized);
        messages.push(tool_result_message);

        if is_aborted(signal) {
            break;
        }
    }

    ExecutedToolCallBatch {
        messages,
        terminate: should_terminate_tool_batch(&finalized_calls),
    }
}

async fn execute_tool_calls_parallel(
    current_context: &AgentContext,
    assistant_message: &AssistantMessage,
    tool_calls: &[ToolCall],
    config: &AgentLoopConfig,
    signal: Option<&AbortSignal>,
    emit: &AgentEventSink,
) -> ExecutedToolCallBatch {
    enum FinalizedEntry {
        Done(FinalizedToolCallOutcome),
        Pending(BoxFuture<'static, FinalizedToolCallOutcome>),
    }

    // 先逐个 start + prepare;prepared 的调用并发执行,tool_execution_end 按
    // 完成顺序在各 thunk 内发出。
    let mut entries: Vec<FinalizedEntry> = Vec::new();
    for tool_call in tool_calls {
        emit_event(
            emit,
            AgentEvent::ToolExecutionStart {
                tool_call_id: tool_call.id.clone(),
                tool_name: tool_call.name.clone(),
                args: Value::Object(tool_call.arguments.clone()),
            },
        )
        .await;

        let preparation =
            prepare_tool_call(current_context, assistant_message, tool_call, config, signal).await;
        match preparation {
            Preparation::Immediate { result, is_error } => {
                let finalized = FinalizedToolCallOutcome {
                    tool_call: tool_call.clone(),
                    result,
                    is_error,
                };
                emit_tool_execution_end(&finalized, emit).await;
                entries.push(FinalizedEntry::Done(finalized));
                if is_aborted(signal) {
                    break;
                }
            }
            Preparation::Prepared { tool, args } => {
                let prepared = PreparedToolCall {
                    tool_call: tool_call.clone(),
                    tool,
                    args,
                };
                let context = current_context.clone();
                let assistant = assistant_message.clone();
                let config = config.clone();
                let thunk_signal = signal.cloned();
                let emit = emit.clone();
                entries.push(FinalizedEntry::Pending(Box::pin(async move {
                    let executed =
                        execute_prepared_tool_call(&prepared, thunk_signal.as_ref(), &emit).await;
                    let finalized = finalize_executed_tool_call(
                        &context,
                        &assistant,
                        &prepared,
                        executed,
                        &config,
                        thunk_signal.as_ref(),
                    )
                    .await;
                    emit_tool_execution_end(&finalized, &emit).await;
                    finalized
                })));
                if is_aborted(signal) {
                    break;
                }
            }
        }
    }

    // 全部完成后按 assistant 源顺序发出 toolResult 消息事件。
    let ordered_finalized_calls: Vec<FinalizedToolCallOutcome> = join_all(entries.into_iter().map(
        |entry| async move {
            match entry {
                FinalizedEntry::Done(outcome) => outcome,
                FinalizedEntry::Pending(future) => future.await,
            }
        },
    ))
    .await;

    let mut messages: Vec<ToolResultMessage> = Vec::new();
    for finalized in &ordered_finalized_calls {
        let tool_result_message = create_tool_result_message(finalized);
        emit_tool_result_message(&tool_result_message, emit).await;
        messages.push(tool_result_message);
    }

    ExecutedToolCallBatch {
        messages,
        terminate: should_terminate_tool_batch(&ordered_finalized_calls),
    }
}

fn is_aborted(signal: Option<&AbortSignal>) -> bool {
    signal.is_some_and(|signal| signal.is_cancelled())
}

/// 整批全部 terminate 才提前终止(对齐蓝本 shouldTerminateToolBatch)。
fn should_terminate_tool_batch(finalized_calls: &[FinalizedToolCallOutcome]) -> bool {
    !finalized_calls.is_empty() && finalized_calls.iter().all(|f| f.result.terminate)
}

/// 工具执行前准备:解析工具 → prepareArguments → 参数校验 → beforeToolCall 钩子 →
/// aborted 检查。任何一步失败都产出 immediate error 结果(对齐 TS try/catch)。
async fn prepare_tool_call(
    current_context: &AgentContext,
    assistant_message: &AssistantMessage,
    tool_call: &ToolCall,
    config: &AgentLoopConfig,
    signal: Option<&AbortSignal>,
) -> Preparation {
    let Some(tool) = current_context
        .tools
        .iter()
        .find(|tool| tool.name == tool_call.name)
        .cloned()
    else {
        return Preparation::Immediate {
            result: create_error_tool_result(&format!("Tool {} not found", tool_call.name)),
            is_error: true,
        };
    };

    let context = current_context.clone();
    let assistant = assistant_message.clone();
    let tool_call = tool_call.clone();
    let signal = signal.cloned();
    let inner = async move {
        // prepareArguments:校验前对原始参数的兼容转换。
        let raw_args = Value::Object(tool_call.arguments.clone());
        let prepared_args = match &tool.prepare_arguments {
            Some(prepare) => prepare(raw_args),
            None => raw_args,
        };
        let validated_args = validate_tool_arguments(&tool.parameters, prepared_args)?;

        if let Some(before) = &config.before_tool_call {
            let before_result = before(
                BeforeToolCallContext {
                    assistant_message: assistant.clone(),
                    tool_call: tool_call.clone(),
                    args: validated_args.clone(),
                    context: context.clone(),
                },
                signal.clone(),
            )
            .await;
            // aborted 优先于 block(对齐蓝本判断顺序)。
            if is_aborted(signal.as_ref()) {
                return Ok::<Preparation, String>(Preparation::Immediate {
                    result: create_error_tool_result("Operation aborted"),
                    is_error: true,
                });
            }
            if let Some(before_result) = before_result {
                if before_result.block {
                    let mut result = create_error_tool_result(
                        before_result
                            .reason
                            .as_deref()
                            .unwrap_or("Tool execution was blocked"),
                    );
                    if before_result.terminate {
                        result.terminate = true;
                    }
                    return Ok(Preparation::Immediate {
                        result,
                        is_error: true,
                    });
                }
            }
        }
        if is_aborted(signal.as_ref()) {
            return Ok(Preparation::Immediate {
                result: create_error_tool_result("Operation aborted"),
                is_error: true,
            });
        }
        Ok(Preparation::Prepared {
            tool,
            args: validated_args,
        })
    };
    match inner.await {
        Ok(preparation) => preparation,
        Err(message) => Preparation::Immediate {
            result: create_error_tool_result(&message),
            is_error: true,
        },
    }
}

/// 执行已准备好的工具调用;Ok → isError false,Err → error 工具结果。
/// 执行期间的流式部分结果缓存于缓冲区,execute 完成后(无论成败)依序发出,
/// 保证 update 事件严格夹在 start 与 end 之间。
async fn execute_prepared_tool_call(
    prepared: &PreparedToolCall,
    signal: Option<&AbortSignal>,
    emit: &AgentEventSink,
) -> ExecutedToolCallOutcome {
    let updates: Arc<Mutex<Vec<AgentEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let accepting = Arc::new(std::sync::atomic::AtomicBool::new(true));

    let on_update: AgentToolUpdateCallback = {
        let updates = updates.clone();
        let accepting = accepting.clone();
        let tool_call = prepared.tool_call.clone();
        Arc::new(move |partial_result| {
            // 工具 promise 已落定后的调用被忽略(对齐蓝本 acceptingUpdates)。
            if !accepting.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            updates.lock().unwrap().push(AgentEvent::ToolExecutionUpdate {
                tool_call_id: tool_call.id.clone(),
                tool_name: tool_call.name.clone(),
                args: Value::Object(tool_call.arguments.clone()),
                partial_result,
            });
        })
    };

    let outcome = match (prepared.tool.execute)(
        prepared.tool_call.id.clone(),
        prepared.args.clone(),
        signal.cloned(),
        Some(on_update),
    )
    .await
    {
        Ok(result) => ExecutedToolCallOutcome {
            result,
            is_error: false,
        },
        Err(error) => ExecutedToolCallOutcome {
            result: create_error_tool_result(&error.to_string()),
            is_error: true,
        },
    };

    accepting.store(false, std::sync::atomic::Ordering::SeqCst);
    let pending_updates: Vec<AgentEvent> = updates.lock().unwrap().drain(..).collect();
    for event in pending_updates {
        (emit)(event).await;
    }
    outcome
}

/// afterToolCall 钩子逐字段覆盖(缺省保留原值,无深合并;isError 可被覆盖)。
async fn finalize_executed_tool_call(
    current_context: &AgentContext,
    assistant_message: &AssistantMessage,
    prepared: &PreparedToolCall,
    executed: ExecutedToolCallOutcome,
    config: &AgentLoopConfig,
    signal: Option<&AbortSignal>,
) -> FinalizedToolCallOutcome {
    let mut result = executed.result;
    let mut is_error = executed.is_error;

    if let Some(after) = &config.after_tool_call {
        // TS 蓝本用 try/catch 包住钩子异常;Rust 契约要求钩子不抛(返回值即结果),
        // 因此无 catch 路径。
        if let Some(after_result) = after(
            AfterToolCallContext {
                assistant_message: assistant_message.clone(),
                tool_call: prepared.tool_call.clone(),
                args: prepared.args.clone(),
                result: result.clone(),
                is_error,
                context: current_context.clone(),
            },
            signal.cloned(),
        )
        .await
        {
            if let Some(content) = after_result.content {
                result.content = content;
            }
            // TS `??` 语义:显式 null 视为未提供,保留原值。
            if let Some(details) = after_result.details {
                if !details.is_null() {
                    result.details = details;
                }
            }
            if let Some(usage) = after_result.usage {
                result.usage = Some(usage);
            }
            if let Some(terminate) = after_result.terminate {
                result.terminate = terminate;
            }
            if let Some(after_is_error) = after_result.is_error {
                is_error = after_is_error;
            }
        }
    }

    FinalizedToolCallOutcome {
        tool_call: prepared.tool_call.clone(),
        result,
        is_error,
    }
}

fn create_error_tool_result(message: &str) -> AgentToolResult {
    AgentToolResult {
        content: vec![TextOrImageContent::text(message)],
        details: Value::Object(serde_json::Map::new()),
        ..Default::default()
    }
}

fn create_tool_result_message(finalized: &FinalizedToolCallOutcome) -> ToolResultMessage {
    ToolResultMessage {
        role: "toolResult".to_string(),
        tool_call_id: finalized.tool_call.id.clone(),
        tool_name: finalized.tool_call.name.clone(),
        // 无类型工具可能返回空 content,归一化保证历史与 provider 载荷无 null。
        content: finalized.result.content.clone(),
        details: if finalized.result.details.is_null() {
            None
        } else {
            Some(finalized.result.details.clone())
        },
        usage: finalized.result.usage.clone(),
        added_tool_names: finalized
            .result
            .added_tool_names
            .clone()
            .filter(|names| !names.is_empty()),
        is_error: finalized.is_error,
        timestamp: now_ms(),
    }
}

async fn emit_tool_execution_end(finalized: &FinalizedToolCallOutcome, emit: &AgentEventSink) {
    (emit)(AgentEvent::ToolExecutionEnd {
        tool_call_id: finalized.tool_call.id.clone(),
        tool_name: finalized.tool_call.name.clone(),
        result: finalized.result.clone(),
        is_error: finalized.is_error,
    })
    .await;
}

async fn emit_tool_result_message(message: &ToolResultMessage, emit: &AgentEventSink) {
    (emit)(AgentEvent::MessageStart {
        message: tool_result_message_of(message.clone()),
    })
    .await;
    (emit)(AgentEvent::MessageEnd {
        message: tool_result_message_of(message.clone()),
    })
    .await;
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

async fn emit_event(emit: &AgentEventSink, event: AgentEvent) {
    (emit)(event).await;
}

fn message_tool_calls(message: &AssistantMessage) -> Vec<ToolCall> {
    message
        .content
        .iter()
        .filter_map(|content| match content {
            AssistantContent::ToolCall(tool_call) => Some(tool_call.clone()),
            _ => None,
        })
        .collect()
}

/// Unix 时间戳(毫秒)。
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// AssistantMessage → AgentMessage。
pub(crate) fn assistant_message_of(message: AssistantMessage) -> AgentMessage {
    AgentMessage::Message(TypedMessage::Assistant(message))
}

/// ToolResultMessage → AgentMessage。
pub(crate) fn tool_result_message_of(message: ToolResultMessage) -> AgentMessage {
    AgentMessage::Message(TypedMessage::ToolResult(message))
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod testing {
    //! 供 agent_loop / agent 单测共用的离线 mock(StreamFn / 工具 / 事件收集)。

    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use serde_json::{json, Value};

    use super::*;
    use crate::agent::agent::default_convert_to_llm_fn;
    use crate::agent::llm::event_stream::event_stream;
    use crate::agent::llm::SimpleStreamOptions;

    /// 助手消息构造器(测试用最小字段)。
    pub fn test_assistant(content: Vec<AssistantContent>, stop_reason: StopReason) -> AssistantMessage {
        AssistantMessage {
            role: "assistant".to_string(),
            content,
            api: "openai-completions".to_string(),
            provider: "custom".to_string(),
            model: "test-model".to_string(),
            response_model: None,
            response_id: None,
            usage: crate::agent::llm::Usage::zero(),
            stop_reason,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: 0,
        }
    }

    pub fn test_tool_call(id: &str, name: &str, arguments: Value) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: serde_json::from_value(arguments).unwrap(),
            thought_signature: None,
            namespace: None,
        }
    }

    pub fn user_message(text: &str, timestamp: i64) -> AgentMessage {
        AgentMessage::user_text(text, timestamp)
    }

    pub fn test_model() -> crate::agent::llm::Model {
        crate::agent::llm::Model::from_settings("test-model", "http://localhost")
    }

    pub fn test_loop_config(model: crate::agent::llm::Model) -> AgentLoopConfig {
        AgentLoopConfig {
            model,
            stream: SimpleStreamOptions::default(),
            convert_to_llm: default_convert_to_llm_fn(),
            transform_context: None,
            get_api_key: None,
            should_stop_after_turn: None,
            prepare_next_turn: None,
            get_steering_messages: None,
            get_follow_up_messages: None,
            tool_execution: ToolExecutionMode::Parallel,
            before_tool_call: None,
            after_tool_call: None,
        }
    }

    /// 一次 LLM 调用的捕获项。
    #[derive(Clone, Debug)]
    pub struct CapturedCall {
        pub context: LlmContext,
        pub api_key: Option<String>,
        pub reasoning: Option<ThinkingLevel>,
    }    /// 一段脚本化 LLM 响应:事件序列 + result() 终值。
    pub struct Script {
        pub events: Vec<AssistantMessageEvent>,
        pub result: AssistantMessage,
    }

    /// 脚本化 mock StreamFn:每次调用弹一段脚本,并捕获 (context, options) 供断言。
    pub fn scripted_stream_fn(
        scripts: Vec<Script>,
    ) -> (StreamFn, Arc<Mutex<Vec<CapturedCall>>>) {
        let queue = Arc::new(Mutex::new(VecDeque::from(scripts)));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let sink = calls.clone();
        let stream_fn: StreamFn = Arc::new(move |_model, context, options| {
            let queue = queue.clone();
            let sink = sink.clone();
            Box::pin(async move {
                let (api_key, reasoning) = match &options {
                    Some(options) => (options.api_key.clone(), options.reasoning),
                    None => (None, None),
                };
                sink.lock().unwrap().push(CapturedCall {
                    context: context.clone(),
                    api_key,
                    reasoning,
                });
                let script = queue
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("scripted stream fn exhausted");
                let (stream, writer) = event_stream();
                for event in script.events {
                    writer.push(event);
                }
                writer.end(script.result);
                stream
            })
        });
        (stream_fn, calls)
    }

    /// 纯文本回复脚本:Start → TextDelta → Done(stop)。
    pub fn text_script(text: &str) -> Script {
        let final_message = test_assistant(vec![AssistantContent::text(text)], StopReason::Stop);
        let events = vec![
            AssistantMessageEvent::Start {
                partial: test_assistant(vec![], StopReason::Pending),
            },
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: text.to_string(),
                partial: test_assistant(vec![AssistantContent::text(text)], StopReason::Pending),
            },
            AssistantMessageEvent::Done {
                reason: StopReason::Stop,
                message: final_message.clone(),
            },
        ];
        Script { events, result: final_message }
    }

    /// 工具调用回复脚本:Start → ToolcallEnd* → Done(toolUse)。
    pub fn tool_call_script(tool_calls: Vec<ToolCall>, text: &str) -> Script {
        tool_call_script_with_stop(tool_calls, text, StopReason::ToolUse)
    }

    /// 指定 stopReason 的工具调用回复脚本(length 截断场景用)。
    pub fn tool_call_script_with_stop(
        tool_calls: Vec<ToolCall>,
        text: &str,
        stop_reason: StopReason,
    ) -> Script {
        let mut content: Vec<AssistantContent> = Vec::new();
        if !text.is_empty() {
            content.push(AssistantContent::text(text));
        }
        for tool_call in &tool_calls {
            content.push(AssistantContent::ToolCall(tool_call.clone()));
        }
        let mut events = vec![AssistantMessageEvent::Start {
            partial: test_assistant(vec![], StopReason::Pending),
        }];
        for (index, tool_call) in tool_calls.iter().enumerate() {
            let mut partial: Vec<AssistantContent> = Vec::new();
            if !text.is_empty() {
                partial.push(AssistantContent::text(text));
            }
            for seen in tool_calls.iter().take(index + 1) {
                partial.push(AssistantContent::ToolCall(seen.clone()));
            }
            events.push(AssistantMessageEvent::ToolcallEnd {
                content_index: index as u32,
                tool_call: tool_call.clone(),
                partial: test_assistant(partial, StopReason::Pending),
            });
        }
        let final_message = test_assistant(content, stop_reason);
        events.push(AssistantMessageEvent::Done {
            reason: stop_reason,
            message: final_message.clone(),
        });
        Script { events, result: final_message }
    }

    /// 错误/中止回复脚本:Start → Error(终值为带 errorMessage 的 assistant 消息)。
    pub fn error_script(stop_reason: StopReason, message: &str) -> Script {
        let mut error = test_assistant(vec![], stop_reason);
        error.error_message = Some(message.to_string());
        let events = vec![
            AssistantMessageEvent::Start {
                partial: test_assistant(vec![], StopReason::Pending),
            },
            AssistantMessageEvent::Error {
                reason: stop_reason,
                error: error.clone(),
            },
        ];
        Script { events, result: error }
    }

    /// 工具行为脚本。
    #[derive(Clone)]
    pub enum ToolBehavior {
        /// 正常返回文本结果。
        Ok(String),
        /// 执行失败(转 error 工具结果)。
        Err(String),
        /// 延迟后返回(并行完成序测试用)。
        DelayedOk(u64, String),
        /// 先发一次流式部分结果再返回。
        Update(String, String),
    }

    #[derive(Clone, Debug)]
    pub struct ToolCallRecord {
        pub id: String,
        pub args: Value,
    }

    /// 构造记录型 mock 工具,返回 (工具定义, 调用记录)。
    pub fn make_tool(name: &str, behavior: ToolBehavior) -> (AgentTool, Arc<Mutex<Vec<ToolCallRecord>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let tool = AgentTool {
            name: name.to_string(),
            label: name.to_string(),
            description: "test tool".to_string(),
            parameters: json!({"type": "object"}),
            execution_mode: None,
            prepare_arguments: None,
            execute: {
                let calls = calls.clone();
                Arc::new(move |id: String, args: Value, _signal, on_update| {
                    let calls = calls.clone();
                    let behavior = behavior.clone();
                    Box::pin(async move {
                        calls.lock().unwrap().push(ToolCallRecord { id, args });
                        match behavior {
                            ToolBehavior::Ok(text) => Ok(AgentToolResult::text(text)),
                            ToolBehavior::Err(message) => Err(message.into()),
                            ToolBehavior::DelayedOk(ms, text) => {
                                tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                                Ok(AgentToolResult::text(text))
                            }
                            ToolBehavior::Update(partial, final_text) => {
                                if let Some(on_update) = on_update {
                                    on_update(AgentToolResult::text(partial));
                                }
                                Ok(AgentToolResult::text(final_text))
                            }
                        }
                    })
                })
            },
        };
        (tool, calls)
    }

    /// 收集全部事件的 emit sink。
    pub fn collecting_sink() -> (AgentEventSink, Arc<Mutex<Vec<AgentEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink: AgentEventSink = {
            let events = events.clone();
            Arc::new(move |event| {
                let events = events.clone();
                Box::pin(async move {
                    events.lock().unwrap().push(event);
                })
            })
        };
        (sink, events)
    }

    pub fn event_kinds(events: &[AgentEvent]) -> Vec<&'static str> {
        events
            .iter()
            .map(|event| match event {
                AgentEvent::AgentStart => "agent_start",
                AgentEvent::AgentEnd { .. } => "agent_end",
                AgentEvent::TurnStart => "turn_start",
                AgentEvent::TurnEnd { .. } => "turn_end",
                AgentEvent::MessageStart { .. } => "message_start",
                AgentEvent::MessageUpdate { .. } => "message_update",
                AgentEvent::MessageEnd { .. } => "message_end",
                AgentEvent::ToolExecutionStart { .. } => "tool_execution_start",
                AgentEvent::ToolExecutionUpdate { .. } => "tool_execution_update",
                AgentEvent::ToolExecutionEnd { .. } => "tool_execution_end",
            })
            .collect()
    }

    /// 从事件里提取 tool_execution_start 的 (toolCallId, toolName, args)。
    pub fn tool_exec_starts(events: &[AgentEvent]) -> Vec<(String, String, Value)> {
        events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ToolExecutionStart {
                    tool_call_id,
                    tool_name,
                    args,
                } => Some((tool_call_id.clone(), tool_name.clone(), args.clone())),
                _ => None,
            })
            .collect()
    }

    /// 从事件里提取 tool_execution_end 的 (toolCallId, isError)。
    pub fn tool_exec_ends(events: &[AgentEvent]) -> Vec<(String, bool)> {
        events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ToolExecutionEnd {
                    tool_call_id, is_error, ..
                } => Some((tool_call_id.clone(), *is_error)),
                _ => None,
            })
            .collect()
    }

    /// 从事件里提取 message_start 携带的 toolResult 消息(按发出顺序)。
    pub fn message_start_tool_results(events: &[AgentEvent]) -> Vec<ToolResultMessage> {
        events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::MessageStart { message } => match message {
                    AgentMessage::Message(TypedMessage::ToolResult(tool_result)) => {
                        Some(tool_result.clone())
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use serde_json::json;
    use std::collections::VecDeque;

    #[tokio::test]
    async fn full_turn_without_tools_emits_complete_sequence() {
        let (stream_fn, _calls) = scripted_stream_fn(vec![text_script("hello")]);
        let prompt = user_message("hi", 1_000);
        let context = AgentContext {
            system_prompt: "sys".to_string(),
            messages: Vec::new(),
            tools: Vec::new(),
        };
        let stream = agent_loop(
            vec![prompt],
            context,
            test_loop_config(test_model()),
            None,
            stream_fn,
        );

        let mut stream = stream;
        let events: Vec<AgentEvent> = (&mut stream).collect().await;
        let messages = stream.result().await;

        assert_eq!(
            event_kinds(&events),
            vec![
                "agent_start",
                "turn_start",
                "message_start",
                "message_end",
                "message_start",
                "message_update",
                "message_end",
                "turn_end",
                "agent_end",
            ]
        );
        // message_update 携带原始 assistantMessageEvent 与流式 partial。
        match &events[5] {
            AgentEvent::MessageUpdate {
                message,
                assistant_message_event,
            } => {
                assert!(matches!(
                    assistant_message_event,
                    AssistantMessageEvent::TextDelta { delta, .. } if delta == "hello"
                ));
                assert!(matches!(
                    message,
                    AgentMessage::Message(TypedMessage::Assistant(_))
                ));
            }
            other => panic!("expected message_update, got {other:?}"),
        }
        // turn_end 无 toolResults。
        match &events[7] {
            AgentEvent::TurnEnd { tool_results, .. } => assert!(tool_results.is_empty()),
            other => panic!("expected turn_end, got {other:?}"),
        }
        // agent_end 携带完整新消息列表。
        match &events[8] {
            AgentEvent::AgentEnd { messages } => {
                assert_eq!(messages.len(), 2);
                assert_eq!(messages[0].role_name(), "user");
                assert_eq!(messages[1].role_name(), "assistant");
            }
            other => panic!("expected agent_end, got {other:?}"),
        }
        assert_eq!(messages.len(), 2);
    }

    #[tokio::test]
    async fn tool_call_round_executes_tool_and_feeds_result_back() {
        let tool_call = test_tool_call("call_1", "echo", json!({"value": "x"}));
        let (stream_fn, calls) = scripted_stream_fn(vec![
            tool_call_script(vec![tool_call.clone()], ""),
            text_script("done"),
        ]);
        let (tool, tool_log) = make_tool("echo", ToolBehavior::Ok("echo:x".to_string()));
        let (emit, events) = collecting_sink();
        let context = AgentContext {
            system_prompt: "sys".to_string(),
            messages: Vec::new(),
            tools: vec![tool],
        };

        let messages = run_agent_loop(
            vec![user_message("run", 1_000)],
            context,
            test_loop_config(test_model()),
            emit,
            None,
            stream_fn,
        )
        .await;

        // 工具以(校验后的)参数执行了一次。
        let log = tool_log.lock().unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].id, "call_1");
        assert_eq!(log[0].args, json!({"value": "x"}));
        drop(log);

        // 新消息:user → assistant(toolcall) → toolResult → assistant(最终)。
        assert_eq!(messages.len(), 4);
        let tool_result = match &messages[2] {
            AgentMessage::Message(TypedMessage::ToolResult(tool_result)) => tool_result.clone(),
            other => panic!("expected toolResult message, got {other:?}"),
        };
        assert_eq!(tool_result.tool_call_id, "call_1");
        assert_eq!(tool_result.tool_name, "echo");
        assert_eq!(tool_result.content, vec![TextOrImageContent::text("echo:x")]);
        assert!(!tool_result.is_error);

        assert_eq!(
            event_kinds(&events.lock().unwrap()),
            vec![
                "agent_start",
                "turn_start",
                "message_start",
                "message_end",
                "message_start",
                "message_update",
                "message_end",
                "tool_execution_start",
                "tool_execution_end",
                "message_start",
                "message_end",
                "turn_end",
                "turn_start",
                "message_start",
                "message_update",
                "message_end",
                "turn_end",
                "agent_end",
            ]
        );

        // 第二次 LLM 调用的上下文已包含 assistant + toolResult。
        let captured = calls.lock().unwrap();
        assert_eq!(captured.len(), 2);
        assert_eq!(captured[0].context.messages.len(), 1);
        assert_eq!(captured[1].context.messages.len(), 3);
        assert!(matches!(
            captured[1].context.messages[1],
            crate::agent::llm::Message::Assistant(_)
        ));
        assert!(matches!(
            captured[1].context.messages[2],
            crate::agent::llm::Message::ToolResult(_)
        ));
    }

    #[tokio::test]
    async fn tool_execution_update_events_flow_between_start_and_end() {
        let tool_call = test_tool_call("call_1", "progress", json!({}));
        let (stream_fn, _calls) = scripted_stream_fn(vec![
            tool_call_script(vec![tool_call], ""),
            text_script("done"),
        ]);
        let (tool, _log) = make_tool(
            "progress",
            ToolBehavior::Update("half".to_string(), "full".to_string()),
        );
        let (emit, events) = collecting_sink();
        let context = AgentContext {
            system_prompt: String::new(),
            messages: Vec::new(),
            tools: vec![tool],
        };

        run_agent_loop(
            vec![user_message("run", 1_000)],
            context,
            test_loop_config(test_model()),
            emit,
            None,
            stream_fn,
        )
        .await;

        let events = events.lock().unwrap();
        let kinds = event_kinds(&events);
        let update_pos = kinds
            .iter()
            .position(|kind| *kind == "tool_execution_update")
            .expect("expected a tool_execution_update event");
        let start_pos = kinds
            .iter()
            .position(|kind| *kind == "tool_execution_start")
            .unwrap();
        let end_pos = kinds
            .iter()
            .position(|kind| *kind == "tool_execution_end")
            .unwrap();
        assert!(start_pos < update_pos && update_pos < end_pos);
        match &events[update_pos] {
            AgentEvent::ToolExecutionUpdate {
                tool_call_id,
                partial_result,
                ..
            } => {
                assert_eq!(tool_call_id, "call_1");
                assert_eq!(
                    partial_result.content,
                    vec![TextOrImageContent::text("half")]
                );
            }
            other => panic!("expected tool_execution_update, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn before_tool_call_block_produces_error_result_without_executing() {
        let tool_call = test_tool_call("call_1", "echo", json!({}));
        let (stream_fn, _calls) = scripted_stream_fn(vec![
            tool_call_script(vec![tool_call], ""),
            text_script("ok"),
        ]);
        let (tool, tool_log) = make_tool("echo", ToolBehavior::Ok("ran".to_string()));
        let mut config = test_loop_config(test_model());
        config.before_tool_call = Some(Arc::new(|_context, _signal| {
            Box::pin(async move {
                Some(crate::agent::types::BeforeToolCallResult {
                    block: true,
                    reason: Some("denied".to_string()),
                    terminate: false,
                })
            })
        }));
        let (emit, events) = collecting_sink();
        let context = AgentContext {
            system_prompt: String::new(),
            messages: Vec::new(),
            tools: vec![tool],
        };

        let messages = run_agent_loop(
            vec![user_message("run", 1_000)],
            context,
            config,
            emit,
            None,
            stream_fn,
        )
        .await;

        // 工具未执行。
        assert!(tool_log.lock().unwrap().is_empty());
        // 工具结果为 error,内容为 block reason;批次未 terminate → 继续下一回合。
        assert_eq!(messages.len(), 4);
        let tool_result = match &messages[2] {
            AgentMessage::Message(TypedMessage::ToolResult(tool_result)) => tool_result.clone(),
            other => panic!("expected toolResult message, got {other:?}"),
        };
        assert!(tool_result.is_error);
        assert_eq!(tool_result.content, vec![TextOrImageContent::text("denied")]);
        let ends = tool_exec_ends(&events.lock().unwrap());
        assert_eq!(ends, vec![("call_1".to_string(), true)]);
    }

    #[tokio::test]
    async fn before_tool_call_block_without_reason_uses_default_message() {
        let tool_call = test_tool_call("call_1", "echo", json!({}));
        let (stream_fn, _calls) = scripted_stream_fn(vec![
            tool_call_script(vec![tool_call], ""),
            text_script("ok"),
        ]);
        let (tool, tool_log) = make_tool("echo", ToolBehavior::Ok("ran".to_string()));
        let mut config = test_loop_config(test_model());
        config.before_tool_call = Some(Arc::new(|_context, _signal| {
            Box::pin(async move {
                Some(crate::agent::types::BeforeToolCallResult {
                    block: true,
                    reason: None,
                    terminate: false,
                })
            })
        }));
        let (emit, _events) = collecting_sink();
        let context = AgentContext {
            system_prompt: String::new(),
            messages: Vec::new(),
            tools: vec![tool],
        };

        let messages = run_agent_loop(
            vec![user_message("run", 1_000)],
            context,
            config,
            emit,
            None,
            stream_fn,
        )
        .await;

        assert!(tool_log.lock().unwrap().is_empty());
        let tool_result = match &messages[2] {
            AgentMessage::Message(TypedMessage::ToolResult(tool_result)) => tool_result.clone(),
            other => panic!("expected toolResult message, got {other:?}"),
        };
        assert_eq!(
            tool_result.content,
            vec![TextOrImageContent::text("Tool execution was blocked")]
        );
    }

    #[tokio::test]
    async fn prepare_arguments_runs_before_validation_and_hook() {
        let tool_call = test_tool_call("call_1", "echo", json!({"value": 1}));
        let (stream_fn, _calls) = scripted_stream_fn(vec![
            tool_call_script(vec![tool_call], ""),
            text_script("ok"),
        ]);
        let (tool, tool_log) = make_tool("echo", ToolBehavior::Ok("ran".to_string()));
        // 为 mock 工具补上 prepare_arguments(合并进原始参数)。
        let mut tool = tool;
        tool.prepare_arguments = Some(Arc::new(|mut args: Value| {
            if let Value::Object(map) = &mut args {
                map.insert("extra".to_string(), json!(true));
            }
            args
        }));
        let captured_args = Arc::new(Mutex::new(Vec::new()));
        let mut config = test_loop_config(test_model());
        let hook_args = captured_args.clone();
        config.before_tool_call = Some(Arc::new(move |context, _signal| {
            let hook_args = hook_args.clone();
            Box::pin(async move {
                hook_args.lock().unwrap().push(context.args.clone());
                None
            })
        }));
        let (emit, _events) = collecting_sink();
        let context = AgentContext {
            system_prompt: String::new(),
            messages: Vec::new(),
            tools: vec![tool],
        };

        run_agent_loop(
            vec![user_message("run", 1_000)],
            context,
            config,
            emit,
            None,
            stream_fn,
        )
        .await;

        // beforeToolCall 收到的是 prepareArguments 之后的参数(validate stub 原样通过)。
        let args = captured_args.lock().unwrap();
        assert_eq!(args.len(), 1);
        assert_eq!(args[0], json!({"value": 1, "extra": true}));
        drop(args);
        // 工具执行同样拿到 prepared 参数。
        let log = tool_log.lock().unwrap();
        assert_eq!(log[0].args, json!({"value": 1, "extra": true}));
    }

    #[tokio::test]
    async fn execute_error_becomes_error_tool_result_and_loop_continues() {
        let tool_call = test_tool_call("call_1", "boom", json!({}));
        let (stream_fn, _calls) = scripted_stream_fn(vec![
            tool_call_script(vec![tool_call], ""),
            text_script("ok"),
        ]);
        let (tool, tool_log) = make_tool("boom", ToolBehavior::Err("exploded".to_string()));
        let (emit, events) = collecting_sink();
        let context = AgentContext {
            system_prompt: String::new(),
            messages: Vec::new(),
            tools: vec![tool],
        };

        let messages = run_agent_loop(
            vec![user_message("run", 1_000)],
            context,
            test_loop_config(test_model()),
            emit,
            None,
            stream_fn,
        )
        .await;

        assert_eq!(tool_log.lock().unwrap().len(), 1);
        assert_eq!(messages.len(), 4);
        let tool_result = match &messages[2] {
            AgentMessage::Message(TypedMessage::ToolResult(tool_result)) => tool_result.clone(),
            other => panic!("expected toolResult message, got {other:?}"),
        };
        assert!(tool_result.is_error);
        assert_eq!(
            tool_result.content,
            vec![TextOrImageContent::text("exploded")]
        );
        let ends = tool_exec_ends(&events.lock().unwrap());
        assert_eq!(ends, vec![("call_1".to_string(), true)]);
    }

    #[tokio::test]
    async fn after_tool_call_overrides_result_fields() {
        let tool_call = test_tool_call("call_1", "echo", json!({}));
        let (stream_fn, _calls) = scripted_stream_fn(vec![
            tool_call_script(vec![tool_call], ""),
            text_script("ok"),
        ]);
        let (tool, _log) = make_tool("echo", ToolBehavior::Ok("original".to_string()));
        let mut config = test_loop_config(test_model());
        config.after_tool_call = Some(Arc::new(|_context, _signal| {
            Box::pin(async move {
                Some(crate::agent::types::AfterToolCallResult {
                    content: Some(vec![TextOrImageContent::text("overridden")]),
                    details: Some(json!({"x": 1})),
                    is_error: Some(true),
                    ..Default::default()
                })
            })
        }));
        let (emit, events) = collecting_sink();
        let context = AgentContext {
            system_prompt: String::new(),
            messages: Vec::new(),
            tools: vec![tool],
        };

        let messages = run_agent_loop(
            vec![user_message("run", 1_000)],
            context,
            config,
            emit,
            None,
            stream_fn,
        )
        .await;

        // tool_execution_end 携带覆盖后的结果与 is_error。
        let events = events.lock().unwrap();
        let end = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::ToolExecutionEnd {
                    tool_call_id,
                    result,
                    is_error,
                    ..
                } => Some((tool_call_id.clone(), result.clone(), *is_error)),
                _ => None,
            })
            .unwrap();
        assert_eq!(end.0, "call_1");
        assert_eq!(end.1.content, vec![TextOrImageContent::text("overridden")]);
        assert_eq!(end.1.details, json!({"x": 1}));
        assert!(end.2);
        drop(events);
        // toolResult 消息同样反映覆盖。
        let tool_result = match &messages[2] {
            AgentMessage::Message(TypedMessage::ToolResult(tool_result)) => tool_result.clone(),
            other => panic!("expected toolResult message, got {other:?}"),
        };
        assert!(tool_result.is_error);
        assert_eq!(
            tool_result.content,
            vec![TextOrImageContent::text("overridden")]
        );
    }

    #[tokio::test]
    async fn length_stop_fails_all_tool_calls_with_truncation_error() {
        let first = test_tool_call("call_1", "tool_one", json!({"a": 1}));
        let second = test_tool_call("call_2", "tool_two", json!({"b": 2}));
        let (stream_fn, _calls) = scripted_stream_fn(vec![
            tool_call_script_with_stop(vec![first, second], "", StopReason::Length),
            text_script("ok"),
        ]);
        let (tool_one, log_one) = make_tool("tool_one", ToolBehavior::Ok("one".to_string()));
        let (tool_two, log_two) = make_tool("tool_two", ToolBehavior::Ok("two".to_string()));
        let (emit, events) = collecting_sink();
        let context = AgentContext {
            system_prompt: String::new(),
            messages: Vec::new(),
            tools: vec![tool_one, tool_two],
        };

        let messages = run_agent_loop(
            vec![user_message("run", 1_000)],
            context,
            test_loop_config(test_model()),
            emit,
            None,
            stream_fn,
        )
        .await;

        // 全部按截断错误失败,工具未执行;terminate: false → 仍进入下一回合。
        assert!(log_one.lock().unwrap().is_empty());
        assert!(log_two.lock().unwrap().is_empty());
        assert_eq!(messages.len(), 5);
        let first_result = match &messages[2] {
            AgentMessage::Message(TypedMessage::ToolResult(tool_result)) => tool_result.clone(),
            other => panic!("expected toolResult message, got {other:?}"),
        };
        let second_result = match &messages[3] {
            AgentMessage::Message(TypedMessage::ToolResult(tool_result)) => tool_result.clone(),
            other => panic!("expected toolResult message, got {other:?}"),
        };
        assert!(first_result.is_error && second_result.is_error);
        assert_eq!(
            first_result.content,
            vec![TextOrImageContent::text(
                "Tool call \"tool_one\" was not executed: the response hit the output token limit, so its arguments may be truncated. Re-issue the tool call with complete arguments."
            )]
        );
        assert_eq!(
            second_result.content,
            vec![TextOrImageContent::text(
                "Tool call \"tool_two\" was not executed: the response hit the output token limit, so its arguments may be truncated. Re-issue the tool call with complete arguments."
            )]
        );
        let ends = tool_exec_ends(&events.lock().unwrap());
        assert_eq!(
            ends,
            vec![("call_1".to_string(), true), ("call_2".to_string(), true)]
        );
    }

    #[tokio::test]
    async fn unknown_tool_produces_error_result() {
        let tool_call = test_tool_call("call_1", "missing_tool", json!({}));
        let (stream_fn, _calls) = scripted_stream_fn(vec![
            tool_call_script(vec![tool_call], ""),
            text_script("ok"),
        ]);
        let (emit, _events) = collecting_sink();
        let context = AgentContext {
            system_prompt: String::new(),
            messages: Vec::new(),
            tools: Vec::new(),
        };

        let messages = run_agent_loop(
            vec![user_message("run", 1_000)],
            context,
            test_loop_config(test_model()),
            emit,
            None,
            stream_fn,
        )
        .await;

        let tool_result = match &messages[2] {
            AgentMessage::Message(TypedMessage::ToolResult(tool_result)) => tool_result.clone(),
            other => panic!("expected toolResult message, got {other:?}"),
        };
        assert!(tool_result.is_error);
        assert_eq!(
            tool_result.content,
            vec![TextOrImageContent::text("Tool missing_tool not found")]
        );
    }

    #[tokio::test]
    async fn steering_messages_injected_before_next_llm_call() {
        let tool_call = test_tool_call("call_1", "echo", json!({}));
        let (stream_fn, calls) = scripted_stream_fn(vec![
            tool_call_script(vec![tool_call], ""),
            text_script("done"),
        ]);
        let (tool, _log) = make_tool("echo", ToolBehavior::Ok("r".to_string()));
        // steering 队列:第一次 poll 空,第二次 poll 注入一条。
        let polls = Arc::new(Mutex::new(VecDeque::from(vec![
            Vec::new(),
            vec![user_message("steer", 2_000)],
            Vec::new(),
        ])));
        let mut config = test_loop_config(test_model());
        let config = {
            let polls = polls.clone();
            config.get_steering_messages = Some(Arc::new(move || {
                let polls = polls.clone();
                Box::pin(async move {
                    polls.lock().unwrap().pop_front().unwrap_or_default()
                })
            }));
            config
        };
        let (emit, events) = collecting_sink();
        let context = AgentContext {
            system_prompt: String::new(),
            messages: Vec::new(),
            tools: vec![tool],
        };

        let messages = run_agent_loop(
            vec![user_message("run", 1_000)],
            context,
            config,
            emit,
            None,
            stream_fn,
        )
        .await;

        // steering 注入在 turn_end 之后、下一回合 LLM 调用之前。
        assert_eq!(
            event_kinds(&events.lock().unwrap()),
            vec![
                "agent_start",
                "turn_start",
                "message_start",
                "message_end",
                "message_start",
                "message_update",
                "message_end",
                "tool_execution_start",
                "tool_execution_end",
                "message_start",
                "message_end",
                "turn_end",
                "turn_start",
                "message_start",
                "message_end",
                "message_start",
                "message_update",
                "message_end",
                "turn_end",
                "agent_end",
            ]
        );
        // 新消息列表包含 steering 消息。
        assert_eq!(messages.len(), 5);
        assert_eq!(messages[3].role_name(), "user");
        // 第二次 LLM 调用的上下文末尾是 steering 消息。
        let captured = calls.lock().unwrap();
        assert_eq!(captured[1].context.messages.len(), 4);
        let last = &captured[1].context.messages[3];
        match last {
            crate::agent::llm::Message::User(user) => {
                assert_eq!(user.content.to_plain_text(), "steer");
            }
            other => panic!("expected steering user message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn follow_up_messages_trigger_outer_loop() {
        let (stream_fn, calls) = scripted_stream_fn(vec![text_script("first"), text_script("second")]);
        // follow-up 仅在 agent 本应停止时 poll:第一次 poll(第一回合后)即返回消息。
        let follow_ups = Arc::new(Mutex::new(VecDeque::from(vec![
            vec![user_message("follow-up", 2_000)],
            Vec::new(),
        ])));
        let mut config = test_loop_config(test_model());
        let config = {
            let follow_ups = follow_ups.clone();
            config.get_follow_up_messages = Some(Arc::new(move || {
                let follow_ups = follow_ups.clone();
                Box::pin(async move {
                    follow_ups.lock().unwrap().pop_front().unwrap_or_default()
                })
            }));
            config
        };
        let (emit, events) = collecting_sink();
        let context = AgentContext {
            system_prompt: String::new(),
            messages: Vec::new(),
            tools: Vec::new(),
        };

        let messages = run_agent_loop(
            vec![user_message("run", 1_000)],
            context,
            config,
            emit,
            None,
            stream_fn,
        )
        .await;

        // 外循环重启:第二个 turn_start 之前注入 follow-up 消息。
        assert_eq!(
            event_kinds(&events.lock().unwrap()),
            vec![
                "agent_start",
                "turn_start",
                "message_start",
                "message_end",
                "message_start",
                "message_update",
                "message_end",
                "turn_end",
                "turn_start",
                "message_start",
                "message_end",
                "message_start",
                "message_update",
                "message_end",
                "turn_end",
                "agent_end",
            ]
        );
        assert_eq!(
            messages,
            vec![
                user_message("run", 1_000),
                assistant_message_of(test_assistant(
                    vec![AssistantContent::text("first")],
                    StopReason::Stop
                )),
                user_message("follow-up", 2_000),
                assistant_message_of(test_assistant(
                    vec![AssistantContent::text("second")],
                    StopReason::Stop
                )),
            ]
        );
        let captured = calls.lock().unwrap();
        assert_eq!(captured[1].context.messages.len(), 3);
    }

    #[tokio::test]
    async fn aborted_stream_ends_run_immediately() {
        let (stream_fn, calls) = scripted_stream_fn(vec![error_script(StopReason::Aborted, "canceled")]);
        let (emit, events) = collecting_sink();
        let context = AgentContext {
            system_prompt: String::new(),
            messages: Vec::new(),
            tools: Vec::new(),
        };

        let messages = run_agent_loop(
            vec![user_message("run", 1_000)],
            context,
            test_loop_config(test_model()),
            emit,
            None,
            stream_fn,
        )
        .await;

        // turn_end(空 toolResults)+ agent_end,不再发起新 LLM 调用。
        assert_eq!(
            event_kinds(&events.lock().unwrap()),
            vec![
                "agent_start",
                "turn_start",
                "message_start",
                "message_end",
                "message_start",
                "message_end",
                "turn_end",
                "agent_end",
            ]
        );
        {
            let events = events.lock().unwrap();
            match &events[6] {
                AgentEvent::TurnEnd { tool_results, .. } => assert!(tool_results.is_empty()),
                other => panic!("expected turn_end, got {other:?}"),
            }
        }
        assert_eq!(messages.len(), 2);
        assert_eq!(calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn aborted_signal_skips_tool_execution() {
        let tool_call = test_tool_call("call_1", "echo", json!({}));
        let (stream_fn, _calls) = scripted_stream_fn(vec![
            tool_call_script(vec![tool_call], ""),
            text_script("ok"),
        ]);
        let (tool, tool_log) = make_tool("echo", ToolBehavior::Ok("ran".to_string()));
        let signal = tokio_util::sync::CancellationToken::new();
        signal.cancel();
        let (emit, _events) = collecting_sink();
        let context = AgentContext {
            system_prompt: String::new(),
            messages: Vec::new(),
            tools: vec![tool],
        };

        let messages = run_agent_loop(
            vec![user_message("run", 1_000)],
            context,
            test_loop_config(test_model()),
            emit,
            Some(signal),
            stream_fn,
        )
        .await;

        assert!(tool_log.lock().unwrap().is_empty());
        let tool_result = match &messages[2] {
            AgentMessage::Message(TypedMessage::ToolResult(tool_result)) => tool_result.clone(),
            other => panic!("expected toolResult message, got {other:?}"),
        };
        assert!(tool_result.is_error);
        assert_eq!(
            tool_result.content,
            vec![TextOrImageContent::text("Operation aborted")]
        );
    }

    #[tokio::test]
    async fn parallel_mode_emits_end_by_completion_and_messages_by_source_order() {
        let slow_call = test_tool_call("call_1", "slow", json!({}));
        let fast_call = test_tool_call("call_2", "fast", json!({}));
        let (stream_fn, _calls) = scripted_stream_fn(vec![
            tool_call_script(vec![slow_call, fast_call], ""),
            text_script("ok"),
        ]);
        let (slow, _slow_log) = make_tool("slow", ToolBehavior::DelayedOk(50, "slow-result".to_string()));
        let (fast, _fast_log) = make_tool("fast", ToolBehavior::Ok("fast-result".to_string()));
        let (emit, events) = collecting_sink();
        let context = AgentContext {
            system_prompt: String::new(),
            messages: Vec::new(),
            tools: vec![slow, fast],
        };

        let messages = run_agent_loop(
            vec![user_message("run", 1_000)],
            context,
            test_loop_config(test_model()),
            emit,
            None,
            stream_fn,
        )
        .await;

        let events = events.lock().unwrap();
        // start 按准备顺序(源顺序)。
        let starts: Vec<String> = tool_exec_starts(&events)
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        assert_eq!(starts, vec!["call_1", "call_2"]);
        // end 按完成顺序:fast 先完成。
        let ends: Vec<String> = tool_exec_ends(&events)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ends, vec!["call_2", "call_1"]);
        // toolResult 消息事件按 assistant 源顺序。
        let message_order: Vec<String> = message_start_tool_results(&events)
            .iter()
            .map(|message| message.tool_call_id.clone())
            .collect();
        assert_eq!(message_order, vec!["call_1", "call_2"]);
        drop(events);
        // 消息内容正确且按源顺序入上下文。
        let first_result = match &messages[2] {
            AgentMessage::Message(TypedMessage::ToolResult(tool_result)) => tool_result.clone(),
            other => panic!("expected toolResult message, got {other:?}"),
        };
        let second_result = match &messages[3] {
            AgentMessage::Message(TypedMessage::ToolResult(tool_result)) => tool_result.clone(),
            other => panic!("expected toolResult message, got {other:?}"),
        };
        assert_eq!(first_result.tool_call_id, "call_1");
        assert_eq!(second_result.tool_call_id, "call_2");
    }

    #[tokio::test]
    async fn sequential_mode_executes_and_emits_one_by_one() {
        let first = test_tool_call("call_1", "slow", json!({}));
        let second = test_tool_call("call_2", "fast", json!({}));
        let (stream_fn, _calls) = scripted_stream_fn(vec![
            tool_call_script(vec![first, second], ""),
            text_script("ok"),
        ]);
        let (slow, _slow_log) = make_tool("slow", ToolBehavior::DelayedOk(20, "slow-result".to_string()));
        let (fast, _fast_log) = make_tool("fast", ToolBehavior::Ok("fast-result".to_string()));
        let mut config = test_loop_config(test_model());
        config.tool_execution = ToolExecutionMode::Sequential;
        let (emit, events) = collecting_sink();
        let context = AgentContext {
            system_prompt: String::new(),
            messages: Vec::new(),
            tools: vec![slow, fast],
        };

        run_agent_loop(
            vec![user_message("run", 1_000)],
            context,
            config,
            emit,
            None,
            stream_fn,
        )
        .await;

        // sequential:start1 → end1 → start2 → end2,与源顺序一致。
        assert_eq!(
            event_kinds(&events.lock().unwrap())
                .into_iter()
                .filter(|kind| kind.starts_with("tool_execution"))
                .collect::<Vec<_>>(),
            vec![
                "tool_execution_start",
                "tool_execution_end",
                "tool_execution_start",
                "tool_execution_end",
            ]
        );
        let starts: Vec<String> = tool_exec_starts(&events.lock().unwrap())
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        assert_eq!(starts, vec!["call_1", "call_2"]);
    }

    #[tokio::test]
    async fn per_tool_sequential_mode_overrides_parallel_default() {
        let first = test_tool_call("call_1", "plain", json!({}));
        let second = test_tool_call("call_2", "strict", json!({}));
        let (stream_fn, _calls) = scripted_stream_fn(vec![
            tool_call_script(vec![first, second], ""),
            text_script("ok"),
        ]);
        let (plain, _plain_log) = make_tool("plain", ToolBehavior::Ok("plain".to_string()));
        let (mut strict, _strict_log) = make_tool("strict", ToolBehavior::Ok("strict".to_string()));
        // 第二个工具声明 Sequential → 整批退化为 sequential。
        strict.execution_mode = Some(ToolExecutionMode::Sequential);
        let (emit, events) = collecting_sink();
        let context = AgentContext {
            system_prompt: String::new(),
            messages: Vec::new(),
            tools: vec![plain, strict],
        };

        run_agent_loop(
            vec![user_message("run", 1_000)],
            context,
            test_loop_config(test_model()),
            emit,
            None,
            stream_fn,
        )
        .await;

        let kinds: Vec<&str> = event_kinds(&events.lock().unwrap())
            .into_iter()
            .filter(|kind| kind.starts_with("tool_execution"))
            .collect();
        assert_eq!(
            kinds,
            vec![
                "tool_execution_start",
                "tool_execution_end",
                "tool_execution_start",
                "tool_execution_end",
            ]
        );
    }

    #[tokio::test]
    async fn get_api_key_resolves_dynamic_key_with_fallbacks() {
        // 三次调用依次返回动态 key、None、空串:分别得到 dynamic、base、base(TS `||` 语义)。
        let (stream_fn, calls) = scripted_stream_fn(vec![
            text_script("one"),
            text_script("two"),
            text_script("three"),
        ]);
        let mut config = test_loop_config(test_model());
        config.stream.api_key = Some("base".to_string());
        let round = Arc::new(Mutex::new(0u32));
        let mut config = {
            let round = round.clone();
            config.get_api_key = Some(Arc::new(move |_provider| {
                let round = round.clone();
                Box::pin(async move {
                    let mut count = round.lock().unwrap();
                    *count += 1;
                    match *count {
                        1 => Some("dynamic".to_string()),
                        2 => None,
                        _ => Some(String::new()),
                    }
                })
            }));
            config
        };
        let rounds = Arc::new(Mutex::new(0u32));
        let config = {
            let rounds = rounds.clone();
            config.get_follow_up_messages = Some(Arc::new(move || {
                let rounds = rounds.clone();
                Box::pin(async move {
                    let mut count = rounds.lock().unwrap();
                    *count += 1;
                    if *count <= 2 {
                        vec![user_message("again", 1_000)]
                    } else {
                        Vec::new()
                    }
                })
            }));
            config
        };
        let (emit, _events) = collecting_sink();
        let context = AgentContext {
            system_prompt: String::new(),
            messages: Vec::new(),
            tools: Vec::new(),
        };

        run_agent_loop(
            vec![user_message("run", 1_000)],
            context,
            config,
            emit,
            None,
            stream_fn,
        )
        .await;

        let captured = calls.lock().unwrap();
        assert_eq!(captured.len(), 3);
        assert_eq!(captured[0].api_key.as_deref(), Some("dynamic"));
        assert_eq!(captured[1].api_key.as_deref(), Some("base"));
        assert_eq!(captured[2].api_key.as_deref(), Some("base"));
    }

    #[tokio::test]
    async fn get_api_key_none_falls_back_to_config_api_key() {
        let (stream_fn, calls) = scripted_stream_fn(vec![text_script("one")]);
        let mut config = test_loop_config(test_model());
        config.stream.api_key = Some("base".to_string());
        config.get_api_key = Some(Arc::new(|_provider| {
            Box::pin(async { None::<String> })
        }));
        let (emit, _events) = collecting_sink();
        let context = AgentContext {
            system_prompt: String::new(),
            messages: Vec::new(),
            tools: Vec::new(),
        };

        run_agent_loop(
            vec![user_message("run", 1_000)],
            context,
            config,
            emit,
            None,
            stream_fn,
        )
        .await;

        assert_eq!(calls.lock().unwrap()[0].api_key.as_deref(), Some("base"));
    }

    #[tokio::test]
    async fn prepare_next_turn_replaces_context_and_reasoning() {
        // 三个回合:第 2 回合 thinking Low,第 3 回合 thinking off(→ reasoning None)。
        let (stream_fn, calls) = scripted_stream_fn(vec![
            text_script("one"),
            text_script("two"),
            text_script("three"),
        ]);
        let mut config = test_loop_config(test_model());
        let rounds = Arc::new(Mutex::new(0u32));
        let mut config = {
            let rounds = rounds.clone();
            config.get_follow_up_messages = Some(Arc::new(move || {
                let rounds = rounds.clone();
                Box::pin(async move {
                    let mut count = rounds.lock().unwrap();
                    *count += 1;
                    if *count <= 2 {
                        vec![user_message("again", 1_000)]
                    } else {
                        Vec::new()
                    }
                })
            }));
            config
        };
        let config = {
            let rounds = rounds.clone();
            config.prepare_next_turn = Some(Arc::new(move |context| {
                let rounds = rounds.clone();
                Box::pin(async move {
                    let count = *rounds.lock().unwrap();
                    if count == 1 {
                        let mut next = context.context.clone();
                        next.messages.push(user_message("prep", 5_000));
                        Some(AgentLoopTurnUpdate {
                            context: Some(next),
                            model: None,
                            thinking_level: Some(ModelThinkingLevel::Low),
                        })
                    } else {
                        Some(AgentLoopTurnUpdate {
                            context: None,
                            model: None,
                            thinking_level: Some(ModelThinkingLevel::Off),
                        })
                    }
                })
            }));
            config
        };
        let (emit, _events) = collecting_sink();
        let context = AgentContext {
            system_prompt: String::new(),
            messages: Vec::new(),
            tools: Vec::new(),
        };

        run_agent_loop(
            vec![user_message("run", 1_000)],
            context,
            config,
            emit,
            None,
            stream_fn,
        )
        .await;

        let captured = calls.lock().unwrap();
        assert_eq!(captured.len(), 3);
        // 第 1 回合:初始 Off → reasoning None。
        assert_eq!(captured[0].reasoning, None);
        // 第 2 回合:context 被 prep 更新追加(含 follow-up "again" 在 prep 之后注入),
        // reasoning = Low。
        assert_eq!(captured[1].reasoning, Some(ThinkingLevel::Low));
        assert!(matches!(
            captured[1].context.messages.last(),
            Some(crate::agent::llm::Message::User(user))
                if user.content.to_plain_text() == "again"
        ));
        // 第 3 回合:thinking off → reasoning None。
        assert_eq!(captured[2].reasoning, None);
    }

    #[tokio::test]
    async fn should_stop_after_turn_stops_before_next_llm_call() {
        let (stream_fn, calls) = scripted_stream_fn(vec![
            text_script("one"),
            text_script("never reached"),
        ]);
        let mut config = test_loop_config(test_model());
        config.should_stop_after_turn = Some(Arc::new(|_context| Box::pin(async { true })));
        let (emit, events) = collecting_sink();
        let context = AgentContext {
            system_prompt: String::new(),
            messages: Vec::new(),
            tools: Vec::new(),
        };

        let messages = run_agent_loop(
            vec![user_message("run", 1_000)],
            context,
            config,
            emit,
            None,
            stream_fn,
        )
        .await;

        assert_eq!(calls.lock().unwrap().len(), 1);
        assert_eq!(messages.len(), 2);
        let kinds = event_kinds(&events.lock().unwrap());
        assert_eq!(kinds.last(), Some(&"agent_end"));
        assert_eq!(
            kinds.iter().filter(|kind| **kind == "turn_start").count(),
            1
        );
    }

    #[test]
    fn continue_preconditions_reject_empty_and_assistant_tail() {
        let empty = AgentContext::default();
        assert_eq!(
            check_continue_preconditions(&empty).unwrap_err(),
            "Cannot continue: no messages in context"
        );
        let assistant_tail = AgentContext {
            system_prompt: String::new(),
            messages: vec![
                user_message("hi", 1),
                assistant_message_of(test_assistant(vec![], StopReason::Stop)),
            ],
            tools: Vec::new(),
        };
        assert_eq!(
            check_continue_preconditions(&assistant_tail).unwrap_err(),
            "Cannot continue from message role: assistant"
        );
        let user_tail = AgentContext {
            system_prompt: String::new(),
            messages: vec![user_message("hi", 1)],
            tools: Vec::new(),
        };
        assert!(check_continue_preconditions(&user_tail).is_ok());
    }

    #[test]
    fn terminate_only_when_whole_batch_terminates() {
        let make = |terminate: bool| FinalizedToolCallOutcome {
            tool_call: test_tool_call("call_1", "t", json!({})),
            result: AgentToolResult {
                terminate,
                ..AgentToolResult::text("x")
            },
            is_error: false,
        };
        assert!(!should_terminate_tool_batch(&[]));
        assert!(!should_terminate_tool_batch(&[make(true), make(false)]));
        assert!(should_terminate_tool_batch(&[make(true), make(true)]));
    }
}
