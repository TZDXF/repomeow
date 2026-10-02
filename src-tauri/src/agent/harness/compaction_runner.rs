//! 压缩运行器:harness 手动/自动压缩的执行路径(自 `agent_harness.rs` 拆出)。
//!
//! - [`AgentHarness::compact`]:手动 compaction,完整 operation 生命周期
//!   (intent 落库 → 准备 → 摘要 → compaction 条目 → StepAttempt/Usage 记录)。
//! - [`AgentHarness::auto_compact`] + [`run_auto_compaction`]:自动压缩
//!   (对齐 pi `_runAutoCompaction` 默认摘要路径),不注册引擎、不切换 busy。
//! - 阈值估算防重触发守卫([`guarded_context_tokens`])等纯辅助,供
//!   `run.rs` 的 pre-prompt/mid-run/run 末检查共用。

use crate::agent::agent_loop::now_ms;
use crate::agent::harness::agent_harness::{AgentHarness, CompactOptions, CompactionOutcome};
use crate::agent::harness::compaction::compaction::{self as compaction_mod, CompactionSettings};
use crate::agent::harness::context::Context as HarnessContext;
use crate::agent::harness::errors::{HarnessUnavailable, OperationError};
use crate::agent::harness::execution::{create_gate, Gate};
use crate::agent::harness::hooks::{
    BeforeCompactionEvent, BeforeDriveEvent, DriveOperation, HookCompactionReason, HookEvent,
    HookName, HookRegistry, HookResult,
};
use crate::agent::harness::runtime::{branch_entries, operation_error};
use crate::agent::harness::session::session::Session;
use crate::agent::harness::session::types::{
    CompactionReason, Entry, LaneRecord, OperationIntent, OperationOutcome, OperationStartedRecord,
    ProvisionedEntry, SessionTree, StepAttemptRecord, StepKind, UsageCauseKind, UsageRecord,
};
use crate::agent::harness::telemetry::TelemetryContext;
use crate::agent::harness::uuid::uuid_v7;
use crate::agent::llm::types::{Model, ThinkingLevel, Usage};
use crate::agent::types::{AgentMessage, StreamFn};
use std::sync::Arc;

/// compact 守卫产物:压缩 operation 所需配置快照(单临界区克隆)。
pub(crate) struct CompactSnapshot {
    pub settings: CompactionSettings,
    pub model: Model,
    pub thinking_level: Option<ThinkingLevel>,
    pub stream_fn: StreamFn,
    pub hooks: Arc<HookRegistry>,
    pub telemetry_context: Option<Arc<dyn TelemetryContext>>,
}

impl AgentHarness {
    pub(crate) async fn auto_compact(
        &self,
        reason: CompactionReason,
        gate: &Gate,
        context: &HarnessContext,
        run_id: &str,
    ) {
        // 配置快照(单临界区):closed 或压缩未启用时直接跳过。
        let Some((settings, model, thinking_level, stream_fn)) = self.auto_compact_config() else {
            return;
        };
        if let Err(error) = run_auto_compaction(
            self.session(),
            settings,
            &model,
            thinking_level,
            &stream_fn,
            reason,
            &self.hooks(),
            gate,
            context,
            run_id,
        )
        .await
        {
            eprintln!(
                "[harness] 自动压缩({})失败: {}",
                reason_str(reason),
                error.message
            );
        }
    }

    /// 手动 compaction:接 compaction 模块(prepare → 摘要 → compaction 条目)。

    pub async fn compact(
        &self,
        options: Option<CompactOptions>,
    ) -> Result<CompactionOutcome, HarnessUnavailable> {
        let shared = self.shared();
        // 守卫 + 配置快照(单临界区;closed/busy 检查顺序与拆分前一致)。
        let snapshot = match self.guard_compact() {
            Ok(snapshot) => snapshot,
            Err(error) => return Err(error),
        };
        let settings = snapshot.settings;
        let model = snapshot.model;
        let thinking_level = snapshot.thinking_level;
        let stream_fn = snapshot.stream_fn;
        let hooks = snapshot.hooks;
        let telemetry_context = snapshot.telemetry_context;
        let run_id = uuid_v7();
        let (hook_gate, _hook_gate_control) = create_gate();
        let hook_context = HarnessContext::new()
            .with_telemetry_opt(telemetry_context)
            .with_signal(hook_gate.signal());
        if hooks
            .run_with_gate(
                HookName::BeforeDrive,
                HookEvent::BeforeDrive(BeforeDriveEvent {
                    lane: "main".to_string(),
                    run_id: run_id.clone(),
                    operation: DriveOperation::Compaction,
                }),
                &hook_gate,
                hook_context.clone(),
            )
            .await
            .is_err()
        {
            return Ok(CompactionOutcome::Failed {
                leaf_id: self.leaf_id().await,
                error: OperationError {
                    code: "hook_gate".to_string(),
                    message: "Hook gate rejected compaction".to_string(),
                },
            });
        }
        let result_entry_id = uuid_v7();
        let custom_instructions = options.and_then(|options| options.custom_instructions);

        // intent 落库。
        if let Err(error) = self
            .session()
            .append_record(LaneRecord::OperationStarted(OperationStartedRecord {
                id: run_id.clone(),
                seq: 0,
                lane: "main".to_string(),
                timestamp: now_ms(),
                source_leaf_id: self.session().get_leaf_id().await.unwrap_or(None),
                intent: OperationIntent::Compaction {
                    custom_instructions: custom_instructions.clone(),
                    result_entry_id: result_entry_id.clone(),
                },
            }))
            .await
        {
            return Ok(CompactionOutcome::Failed {
                leaf_id: self.leaf_id().await,
                error: operation_error(error),
            });
        }
        let _ = shared.busy.send(true);

        // 准备 + 摘要。
        let entries = match branch_entries(self.session()).await {
            Ok(entries) => entries,
            Err(error) => {
                self.finish_run(
                    &shared,
                    &run_id,
                    OperationOutcome::Failed,
                    Some(operation_error(error)),
                )
                .await;
                return Ok(CompactionOutcome::Failed {
                    leaf_id: self.leaf_id().await,
                    error: OperationError {
                        code: "session_error".to_string(),
                        message: "failed to read session branch".to_string(),
                    },
                });
            }
        };
        let preparation = match compaction_mod::prepare_compaction(&entries, settings) {
            Ok(Some(preparation)) => preparation,
            Ok(None) => {
                self.finish_run(&shared, &run_id, OperationOutcome::Declined, None)
                    .await;
                return Ok(CompactionOutcome::DeclinedOrAborted {
                    leaf_id: self.leaf_id().await,
                });
            }
            Err(error) => {
                self.finish_run(
                    &shared,
                    &run_id,
                    OperationOutcome::Failed,
                    Some(OperationError {
                        code: format!("compaction_{}", error.code),
                        message: error.message.clone(),
                    }),
                )
                .await;
                return Ok(CompactionOutcome::Failed {
                    leaf_id: self.leaf_id().await,
                    error: OperationError {
                        code: "compaction_prepare".to_string(),
                        message: error.to_string(),
                    },
                });
            }
        };
        let hook = hooks
            .run_with_gate(
                HookName::BeforeCompaction,
                HookEvent::BeforeCompaction(BeforeCompactionEvent {
                    lane: "main".to_string(),
                    run_id: run_id.clone(),
                    reason: HookCompactionReason::Manual,
                    preparation: compaction_preparation_json(&preparation),
                    custom_instructions: custom_instructions.clone(),
                }),
                &hook_gate,
                hook_context.clone(),
            )
            .await;
        let mut hook_result = None;
        match hook {
            Ok(Some(HookResult::BeforeCompaction(Some(result)))) => {
                if result.decline == Some(true) {
                    self.finish_run(&shared, &run_id, OperationOutcome::Declined, None)
                        .await;
                    return Ok(CompactionOutcome::DeclinedOrAborted {
                        leaf_id: self.leaf_id().await,
                    });
                }
                if let Some(custom) = parse_hook_compaction(&result.compaction) {
                    hook_result = Some(custom);
                }
            }
            Ok(_) => {}
            Err(_) => {
                let error = OperationError {
                    code: "hook_gate".to_string(),
                    message: "Hook gate rejected compaction".to_string(),
                };
                self.finish_run(
                    &shared,
                    &run_id,
                    OperationOutcome::Failed,
                    Some(error.clone()),
                )
                .await;
                return Ok(CompactionOutcome::Failed {
                    leaf_id: self.leaf_id().await,
                    error,
                });
            }
        }

        let compacted = match hook_result {
            Some(result) => Ok(result),
            None => {
                compaction_mod::compact(
                    preparation,
                    &stream_fn,
                    &model,
                    custom_instructions.as_deref(),
                    thinking_level,
                )
                .await
            }
        };
        match compacted {
            Ok(result) => {
                let entry = self
                    .session()
                    .append_entry(
                        ProvisionedEntry::Compaction(
                            crate::agent::harness::session::types::ProvisionedCompactionEntry {
                                id: result_entry_id.clone(),
                                summary: result.summary,
                                retained_tail: result.retained_tail,
                                tokens_before: result.tokens_before,
                                details: Some(result.details),
                                usage: Some(result.usage.clone()),
                            },
                        ),
                        "main".to_string(),
                    )
                    .await;
                let entry = match entry {
                    Ok(Entry::Compaction(entry)) => entry,
                    Ok(_) => unreachable!("compaction entry round-trips"),
                    Err(error) => {
                        self.finish_run(
                            &shared,
                            &run_id,
                            OperationOutcome::Failed,
                            Some(operation_error(error)),
                        )
                        .await;
                        return Ok(CompactionOutcome::Failed {
                            leaf_id: self.leaf_id().await,
                            error: OperationError {
                                code: "session_error".to_string(),
                                message: "failed to append compaction entry".to_string(),
                            },
                        });
                    }
                };
                let _ = self
                    .session()
                    .append_record(LaneRecord::StepAttempt(StepAttemptRecord {
                        id: uuid_v7(),
                        seq: 0,
                        lane: "main".to_string(),
                        timestamp: now_ms(),
                        run_id: run_id.clone(),
                        step: StepKind::Compaction,
                        attempt: 1,
                        result_entry_id: entry.id.clone(),
                        compaction_reason: Some(CompactionReason::Manual),
                    }))
                    .await;
                let _ = self
                    .session()
                    .append_record(LaneRecord::Usage(UsageRecord {
                        id: uuid_v7(),
                        seq: 0,
                        lane: "main".to_string(),
                        timestamp: now_ms(),
                        usage: result.usage,
                        cause: UsageCauseKind::Compaction,
                        run_id: Some(run_id.clone()),
                        entry_id: Some(entry.id.clone()),
                        attempt: Some(1),
                        stop_reason: None,
                        tool_call_id: None,
                        details: None,
                    }))
                    .await;
                self.finish_run(&shared, &run_id, OperationOutcome::Completed, None)
                    .await;
                Ok(CompactionOutcome::Completed {
                    leaf_id: self.leaf_id().await,
                    entry: Box::new(entry),
                })
            }
            Err(error) => {
                let aborted =
                    error.code == crate::agent::harness::types::CompactionErrorCode::Aborted;
                self.finish_run(
                    &shared,
                    &run_id,
                    if aborted {
                        OperationOutcome::Aborted
                    } else {
                        OperationOutcome::Failed
                    },
                    Some(OperationError {
                        code: format!("compaction_{}", error.code),
                        message: error.message.clone(),
                    }),
                )
                .await;
                if aborted {
                    Ok(CompactionOutcome::DeclinedOrAborted {
                        leaf_id: self.leaf_id().await,
                    })
                } else {
                    Ok(CompactionOutcome::Failed {
                        leaf_id: self.leaf_id().await,
                        error: OperationError {
                            code: "compaction_failed".to_string(),
                            message: error.to_string(),
                        },
                    })
                }
            }
        }
    }
}

fn reason_str(reason: CompactionReason) -> &'static str {
    match reason {
        CompactionReason::Threshold => "threshold",
        CompactionReason::Overflow => "overflow",
        CompactionReason::Manual => "manual",
    }
}

/// 最新 compaction 条目的时间戳(防重触发守卫;无则 None)。
pub(crate) async fn session_latest_compaction_timestamp(session: &Session) -> Option<i64> {
    let entries = branch_entries(session).await.ok()?;
    entries.iter().rev().find_map(|entry| match entry {
        Entry::Compaction(compaction) => Some(compaction.timestamp),
        _ => None,
    })
}

/// 阈值估算(对齐 pi 防重触发守卫:usage 来源消息早于最新压缩条目时,
/// 其 usage 反映压缩前旧上下文,返回 0 跳过,等新鲜 usage 到达)。
pub(crate) fn guarded_context_tokens(
    messages: &[AgentMessage],
    latest_compaction_ts: Option<i64>,
) -> i64 {
    let estimate = compaction_mod::estimate_context_tokens(messages);
    if let (Some(index), Some(ts)) = (estimate.last_usage_index, latest_compaction_ts) {
        if messages[index].timestamp() <= ts {
            return 0;
        }
    }
    estimate.tokens
}

/// 自动压缩执行体(对齐 pi `_runAutoCompaction` 默认摘要路径):
/// 不校验/注册 engine、不切换 busy、不落 Operation 记录;
/// 压缩条目 + StepAttempt(带 reason)+ Usage 记录照常写入会话。
pub(crate) async fn run_auto_compaction(
    session: &Session,
    settings: CompactionSettings,
    model: &Model,
    thinking_level: Option<ThinkingLevel>,
    stream_fn: &StreamFn,
    reason: CompactionReason,
    hooks: &HookRegistry,
    gate: &Gate,
    context: &HarnessContext,
    run_id: &str,
) -> Result<(), OperationError> {
    if !settings.enabled {
        return Ok(());
    }
    let result: Option<compaction_mod::CompactResult> = async {
        let entries = branch_entries(session).await.map_err(operation_error)?;
        let preparation =
            compaction_mod::prepare_compaction(&entries, settings).map_err(|error| {
                OperationError {
                    code: format!("compaction_{}", error.code),
                    message: error.message.clone(),
                }
            })?;
        let Some(preparation) = preparation else {
            return Ok(None);
        };
        let hook = hooks
            .run_with_gate(
                HookName::BeforeCompaction,
                HookEvent::BeforeCompaction(BeforeCompactionEvent {
                    lane: "main".to_string(),
                    run_id: run_id.to_string(),
                    reason: match reason {
                        CompactionReason::Manual => HookCompactionReason::Manual,
                        CompactionReason::Threshold => HookCompactionReason::Threshold,
                        CompactionReason::Overflow => HookCompactionReason::Overflow,
                    },
                    preparation: compaction_preparation_json(&preparation),
                    custom_instructions: None,
                }),
                gate,
                context.clone(),
            )
            .await;
        if let Ok(Some(HookResult::BeforeCompaction(Some(result)))) = hook {
            if result.decline == Some(true) {
                return Ok(None);
            }
            if let Some(custom) = parse_hook_compaction(&result.compaction) {
                return Ok(Some(custom));
            }
        } else if hook.is_err() {
            return Err(OperationError {
                code: "hook_gate".to_string(),
                message: "Hook gate rejected compaction".to_string(),
            });
        }
        let result = compaction_mod::compact(preparation, stream_fn, model, None, thinking_level)
            .await
            .map_err(|error| OperationError {
                code: format!("compaction_{}", error.code),
                message: error.message.clone(),
            })?;
        Ok(Some(result))
    }
    .await?;
    let Some(result) = result else {
        return Ok(());
    };
    let entry = session
        .append_entry(
            ProvisionedEntry::Compaction(
                crate::agent::harness::session::types::ProvisionedCompactionEntry {
                    id: uuid_v7(),
                    summary: result.summary,
                    retained_tail: result.retained_tail,
                    tokens_before: result.tokens_before,
                    details: Some(result.details),
                    usage: Some(result.usage.clone()),
                },
            ),
            "main".to_string(),
        )
        .await
        .map_err(operation_error)?;
    let Entry::Compaction(entry) = entry else {
        unreachable!("compaction entry round-trips")
    };
    let run_id = uuid_v7();
    let _ = session
        .append_record(LaneRecord::StepAttempt(StepAttemptRecord {
            id: uuid_v7(),
            seq: 0,
            lane: "main".to_string(),
            timestamp: now_ms(),
            run_id: run_id.clone(),
            step: StepKind::Compaction,
            attempt: 1,
            result_entry_id: entry.id.clone(),
            compaction_reason: Some(reason),
        }))
        .await;
    let _ = session
        .append_record(LaneRecord::Usage(UsageRecord {
            id: uuid_v7(),
            seq: 0,
            lane: "main".to_string(),
            timestamp: now_ms(),
            usage: result.usage,
            cause: UsageCauseKind::Compaction,
            run_id: Some(run_id),
            entry_id: Some(entry.id.clone()),
            attempt: Some(1),
            stop_reason: None,
            tool_call_id: None,
            details: None,
        }))
        .await;
    Ok(())
}

fn compaction_preparation_json(
    preparation: &compaction_mod::CompactionPreparation,
) -> serde_json::Value {
    serde_json::json!({
        "messagesToSummarize": preparation.messages_to_summarize,
        "turnPrefixMessages": preparation.turn_prefix_messages,
        "retainedTail": preparation.retained_tail,
        "isSplitTurn": preparation.is_split_turn,
        "tokensBefore": preparation.tokens_before,
        "previousSummary": preparation.previous_summary,
    })
}

fn parse_hook_compaction(
    value: &Option<serde_json::Value>,
) -> Option<compaction_mod::CompactResult> {
    let value = value.as_ref()?;
    let summary = value.get("summary")?.as_str()?.to_string();
    let retained_tail = value
        .get("retainedTail")
        .cloned()
        .map(serde_json::from_value::<Vec<AgentMessage>>)
        .transpose()
        .ok()?
        .unwrap_or_default();
    let usage = value
        .get("usage")
        .cloned()
        .map(serde_json::from_value::<Usage>)
        .transpose()
        .ok()?
        .unwrap_or_default();
    Some(compaction_mod::CompactResult {
        summary,
        tokens_before: value
            .get("tokensBefore")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0),
        usage,
        retained_tail,
        details: value
            .get("details")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    })
}
// ---------------------------------------------------------------------------
// 纯辅助函数回归测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::agent_loop::testing::{test_assistant, user_message};
    use crate::agent::llm::types::StopReason;
    use crate::agent::types::TypedMessage;

    /// 带 provider usage 与指定时间戳的 assistant 消息。
    fn assistant_with_usage(total_tokens: i64, timestamp: i64) -> AgentMessage {
        let mut assistant = test_assistant(Vec::new(), StopReason::Stop);
        assistant.usage.total_tokens = total_tokens;
        assistant.timestamp = timestamp;
        AgentMessage::Message(TypedMessage::Assistant(assistant))
    }

    #[test]
    fn guarded_tokens_passthrough_without_compaction() {
        let messages = vec![assistant_with_usage(500, 100)];
        assert_eq!(guarded_context_tokens(&messages, None), 500);
    }

    #[test]
    fn guarded_tokens_zero_when_usage_predates_compaction() {
        // usage 来源消息早于(或等于)最新压缩条目:usage 反映压缩前旧上下文,
        // 返回 0 跳过,等新鲜 usage 到达(防重触发守卫)。
        let messages = vec![assistant_with_usage(500, 100)];
        assert_eq!(guarded_context_tokens(&messages, Some(100)), 0);
        assert_eq!(guarded_context_tokens(&messages, Some(200)), 0);
    }

    #[test]
    fn guarded_tokens_fresh_usage_after_compaction() {
        let messages = vec![assistant_with_usage(500, 300)];
        assert_eq!(guarded_context_tokens(&messages, Some(100)), 500);
    }

    #[test]
    fn guarded_tokens_estimate_without_usage_messages() {
        // 无 usage 消息:启发式估算(40 chars → 10 tokens),不受守卫影响。
        let messages = vec![user_message(&"a".repeat(40), 50)];
        assert_eq!(guarded_context_tokens(&messages, Some(100)), 10);
    }

    #[test]
    fn reason_str_maps_all_variants() {
        assert_eq!(reason_str(CompactionReason::Threshold), "threshold");
        assert_eq!(reason_str(CompactionReason::Overflow), "overflow");
        assert_eq!(reason_str(CompactionReason::Manual), "manual");
    }

    #[test]
    fn parse_hook_compaction_round_trip() {
        let value = serde_json::json!({
            "summary": "s",
            "retainedTail": [],
            "usage": {"input": 1, "output": 2, "totalTokens": 3},
            "tokensBefore": 10,
            "details": {"k": 1},
        });
        let result = parse_hook_compaction(&Some(value)).expect("valid hook compaction json");
        assert_eq!(result.summary, "s");
        assert_eq!(result.tokens_before, 10);
        assert_eq!(result.usage.total_tokens, 3);
        assert!(result.retained_tail.is_empty());
        assert_eq!(result.details, serde_json::json!({"k": 1}));
    }

    #[test]
    fn parse_hook_compaction_rejects_missing_fields() {
        assert!(parse_hook_compaction(&None).is_none());
        // summary 缺失 → None。
        assert!(parse_hook_compaction(&Some(serde_json::json!({}))).is_none());
        // 仅 summary:usage/retainedTail 缺省走 serde default → Some。
        let minimal = parse_hook_compaction(&Some(serde_json::json!({"summary": "s"})));
        assert!(minimal.is_some());
    }
}
