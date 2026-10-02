//! 运行期生命周期(自 `agent_harness.rs` 拆出):create 的崩溃恢复归约、
//! resume 与 abort(中止当前 operation,durable abort 标记优先于 signal)。

use crate::agent::agent_loop::now_ms;
use crate::agent::harness::agent_harness::{
    AbortOutcome, AbortResult, AgentHarness, OperationKind, ResumeRejected, ResumeResult,
    SuspendedOperation, SuspensionReason,
};
use crate::agent::harness::errors::{Closed, NothingToResume, OperationError};
use crate::agent::harness::session::session::Session;
use crate::agent::harness::session::types::{
    LaneRecord, OperationIntent, OperationOutcome, SessionError,
};
use crate::agent::harness::uuid::uuid_v7;

impl AgentHarness {
    /// 恢复辅助:把未完结 operation 归约为 aborted 并返回挂起概要。
    pub(crate) async fn restore_open_operations(
        session: &Session,
    ) -> Result<Vec<SuspendedOperation>, SessionError> {
        let open = session.find_open_operations("main", None).await?;
        let mut suspended = Vec::new();
        for record in open {
            let kind = match &record.intent {
                OperationIntent::Run { .. } => OperationKind::Run,
                OperationIntent::Compaction { .. } => OperationKind::Compaction,
                OperationIntent::Navigation { .. } => OperationKind::Navigation,
            };
            let prompt = match &record.intent {
                OperationIntent::Run {
                    original_prompt, ..
                } => Some(original_prompt.clone()),
                _ => None,
            };
            session
                .append_record(LaneRecord::OperationFinished(
                    crate::agent::harness::session::types::OperationFinishedRecord {
                        id: uuid_v7(),
                        seq: 0,
                        lane: "main".to_string(),
                        timestamp: now_ms(),
                        run_id: record.id.clone(),
                        outcome: OperationOutcome::Aborted,
                        error: Some(OperationError {
                            code: "crash_restored".to_string(),
                            message: "Operation was interrupted before completion and was restored as aborted."
                                .to_string(),
                        }),
                    },
                ))
                .await?;
            suspended.push(SuspendedOperation {
                lane: "main".to_string(),
                kind,
                id: record.id.clone(),
                started_at: record.timestamp,
                reason: SuspensionReason::Crash,
                prompt,
                aborting: None,
                missing: (Vec::new(), Vec::new()),
            });
        }
        Ok(suspended)
    }

    /// 恢复:create 已把崩溃 operation 归约 aborted,正常运行后无挂起可续。
    pub async fn resume(&self) -> ResumeResult {
        if self.is_closed() {
            return Err(ResumeRejected::Closed(Closed::new(
                "AgentHarness was closed",
            )));
        }
        Err(ResumeRejected::NothingToResume(NothingToResume::new(
            "No suspended operation to resume; interrupted operations are restored as aborted by create().",
            "main".to_string(),
        )))
    }

    /// 中止当前 operation:先落 durable abort 标记,再拉 abort signal
    /// (对齐蓝本 durable abort 顺序);返回被清空队列的载荷。
    pub async fn abort(&self) -> AbortResult {
        let shared = self.shared();
        // 守卫(单临界区):closed → Closed;无在途引擎 → NoActiveOperation。
        let (run_id, signal, gate_control) = match self.guard_abort() {
            Ok(parts) => parts,
            Err(rejected) => return Err(rejected),
        };
        // 队列载荷收集 + 清空(abort 语义:返回给调用方自行处置)。
        let (steer, follow_up) = {
            let mut queues = shared
                .queues
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (
                std::mem::take(&mut queues.steer),
                std::mem::take(&mut queues.follow_up),
            )
        };
        let _ = self
            .session()
            .append_record(LaneRecord::AbortRequested(
                crate::agent::harness::session::types::AbortRequestedRecord {
                    id: uuid_v7(),
                    seq: 0,
                    lane: "main".to_string(),
                    timestamp: now_ms(),
                    run_id: run_id.clone(),
                },
            ))
            .await;
        gate_control.begin_abort(signal.clone());
        gate_control.signal_abort();
        signal.cancel();
        Ok(AbortOutcome {
            run_id,
            steer: steer.into_iter().map(|item| item.message).collect(),
            follow_up: follow_up.into_iter().map(|item| item.message).collect(),
        })
    }
}
