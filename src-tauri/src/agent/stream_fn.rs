//! 全局默认 StreamFn:对齐 `packages/agent/src/stream-fn.ts`。
//!
//! 宿主可在不引入 provider 目录依赖的前提下安装缺省流函数;`Agent` 与低层
//! 循环在调用方未显式给出 streamFn 时回退到这里。取值时未配置则 panic,
//! 对齐 TS `getDefaultStreamFn()` 的 throw。

use std::sync::RwLock;

use crate::agent::types::StreamFn;

static DEFAULT_STREAM_FN: RwLock<Option<StreamFn>> = RwLock::new(None);

/// 安装全局默认流函数;`None` 清除(对齐 TS 传 `undefined`)。
pub fn set_default_stream_fn(stream_fn: Option<StreamFn>) {
    *DEFAULT_STREAM_FN
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = stream_fn;
}

/// 取全局默认流函数;未配置时 panic(消息对齐 TS 错误文案)。
pub fn default_stream_fn() -> StreamFn {
    DEFAULT_STREAM_FN
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
        .unwrap_or_else(|| {
            panic!(
                "No default stream function configured. Pass streamFn explicitly or call setDefaultStreamFn()."
            )
        })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    use super::*;
    use crate::agent::agent_loop::testing::{test_assistant, test_model};
    use crate::agent::llm::event_stream::event_stream;
    use crate::agent::llm::types::Context as LlmContext;
    use crate::agent::llm::{AssistantContent, AssistantMessageEvent, StopReason};

    /// 一次性文本响应流(测试 fixture)。
    fn simple_text_stream() -> crate::agent::llm::AssistantMessageEventStream {
        let (stream, writer) = event_stream();
        let final_message = test_assistant(vec![AssistantContent::text("x")], StopReason::Stop);
        writer.push(AssistantMessageEvent::Start {
            partial: test_assistant(vec![], StopReason::Pending),
        });
        writer.push(AssistantMessageEvent::Done {
            reason: StopReason::Stop,
            message: final_message.clone(),
        });
        writer.end(final_message);
        stream
    }

    #[test]
    fn default_stream_fn_panics_when_not_configured() {
        set_default_stream_fn(None);
        let result = std::panic::catch_unwind(default_stream_fn);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn set_and_get_round_trip() {
        set_default_stream_fn(None);
        let counter = Arc::new(AtomicU32::new(0));
        let stream_fn: StreamFn = {
            let counter = counter.clone();
            Arc::new(move |_model, _context, _options| {
                let counter = counter.clone();
                Box::pin(async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    simple_text_stream()
                })
            })
        };
        set_default_stream_fn(Some(stream_fn));

        let resolved = default_stream_fn();
        let stream = resolved(test_model(), LlmContext::default(), None).await;
        drop(stream);
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        // None 清除后再次 panic(对齐 TS 重复 throw)。
        set_default_stream_fn(None);
        assert!(std::panic::catch_unwind(default_stream_fn).is_err());
    }
}
