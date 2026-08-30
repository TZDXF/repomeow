//! 泛型事件流:对齐 pi-ai `utils/event-stream.ts` 的 `EventStream<E, R>`。
//!
//! TS 版契约:生产者同步 `push(event)`(内部无界队列)并以 `end(result)` 收尾;
//! 消费者 `for await` 迭代事件、`result()` 取终值。Rust 版用 unbounded mpsc +
//! oneshot 实现,`Stream` 只消费事件队列,`result()` 独立等待终值。
//! 生产端未 `end` 即被丢弃时,oneshot 发送端随 Drop 关闭,消费端 `result()`
//! 会得到流异常终止(panic),用于在开发期暴露违背流契约的实现。

use std::pin::Pin;
use std::task::{Context, Poll};

use futures::Stream;
use tokio::sync::{mpsc, oneshot};

/// 流的生产端。`push` 对齐 TS 的同步无界入队;`end` 发送终值并关闭事件队列。
pub struct EventStreamWriter<E, R> {
    tx: mpsc::UnboundedSender<E>,
    done: Option<oneshot::Sender<R>>,
}

impl<E, R> EventStreamWriter<E, R> {
    /// 入队一个事件。返回 false 表示消费端已丢弃(对齐 TS push 后消费者被 GC 的容忍语义)。
    pub fn push(&self, event: E) -> bool {
        self.tx.send(event).is_ok()
    }

    /// 发送终值并结束事件流。重复调用是编程错误,静默忽略后续调用。
    pub fn end(mut self, result: R) {
        if let Some(done) = self.done.take() {
            let _ = done.send(result);
        }
    }
}

/// 流的消费端:`Stream<Item = E>` 迭代事件,`result()` 等待终值。
pub struct EventStream<E, R> {
    rx: mpsc::UnboundedReceiver<E>,
    done: oneshot::Receiver<R>,
}

impl<E, R> EventStream<E, R> {
    pub async fn result(&mut self) -> R {
        match (&mut self.done).await {
            Ok(result) => result,
            Err(_) => panic!("event stream producer dropped without end()"),
        }
    }
}

impl<E: Send + 'static, R> Stream for EventStream<E, R> {
    type Item = E;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<E>> {
        Pin::new(&mut self.rx).poll_recv(cx)
    }
}

/// 创建一对 (消费端, 生产端)。
pub fn event_stream<E, R>() -> (EventStream<E, R>, EventStreamWriter<E, R>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let (done_tx, done_rx) = oneshot::channel();
    (
        EventStream {
            rx,
            done: done_rx,
        },
        EventStreamWriter {
            tx,
            done: Some(done_tx),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn pushes_events_then_result() {
        let (mut stream, writer) = event_stream::<u32, String>();
        assert!(writer.push(1));
        assert!(writer.push(2));
        writer.end("done".into());

        let collected: Vec<u32> = (&mut stream).map(|v| v).collect().await;
        assert_eq!(collected, vec![1, 2]);
        assert_eq!(stream.result().await, "done");
    }

    #[tokio::test]
    async fn stream_ends_after_writer_consumed() {
        let (mut stream, writer) = event_stream::<u32, u32>();
        writer.push(1);
        writer.end(42);

        let first = stream.next().await;
        assert_eq!(first, Some(1));
        assert_eq!(stream.result().await, 42);
    }

    #[tokio::test]
    async fn interleaved_consumption_waits_for_result() {
        let (mut stream, writer) = event_stream::<u32, u32>();
        let producer = tokio::spawn(async move {
            writer.push(1);
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            writer.push(2);
            writer.end(7);
        });

        let mut seen = Vec::new();
        while let Some(v) = stream.next().await {
            seen.push(v);
        }
        producer.await.unwrap();
        assert_eq!(seen, vec![1, 2]);
        assert_eq!(stream.result().await, 7);
    }
}
