//! harness 统一执行上下文:abort signal + telemetry parent carrier。

use crate::agent::harness::telemetry::TelemetryContext;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
pub struct Context {
    pub abort_signal: Option<CancellationToken>,
    pub telemetry: Option<Arc<dyn TelemetryContext>>,
}

impl std::fmt::Debug for Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context")
            .field("cancelled", &self.is_cancelled())
            .field("telemetry", &self.telemetry.as_ref().map(|_| "<telemetry>"))
            .finish()
    }
}

impl Context {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_signal(mut self, signal: CancellationToken) -> Self {
        {
            self.abort_signal = Some(signal);
            self
        }
    }

    pub fn with_signal_opt(self, signal: Option<CancellationToken>) -> Self {
        match signal {
            Some(signal) => self.with_signal(signal),
            None => self,
        }
    }

    pub fn with_telemetry_opt(self, telemetry: Option<Arc<dyn TelemetryContext>>) -> Self {
        match telemetry {
            Some(telemetry) => self.with_telemetry(telemetry),
            None => self,
        }
    }

    pub fn with_telemetry(mut self, telemetry: Arc<dyn TelemetryContext>) -> Self {
        self.telemetry = Some(telemetry);
        self
    }

    pub fn telemetry(&self) -> Option<&Arc<dyn TelemetryContext>> {
        self.telemetry.as_ref()
    }
    pub fn is_cancelled(&self) -> bool {
        self.abort_signal
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
    }

    pub fn child(&self) -> Self {
        Self {
            abort_signal: self.abort_signal.as_ref().map(|token| token.child_token()),
            telemetry: self.telemetry.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct Recorder(Mutex<Vec<String>>);
    impl TelemetryContext for Recorder {
        fn start_span(
            &self,
            options: crate::agent::harness::telemetry::SpanOptions,
        ) -> Arc<dyn crate::agent::harness::telemetry::TelemetrySpan> {
            self.0.lock().unwrap().push(options.name);
            Arc::new(crate::agent::harness::telemetry::NoopTelemetrySpan)
        }
    }

    #[test]
    fn context_tracks_child_cancellation() {
        let parent = Context::new().with_signal(CancellationToken::new());
        let child = parent.child();
        assert!(!parent.is_cancelled() && !child.is_cancelled());
        if let Some(token) = parent.abort_signal.as_ref() {
            token.cancel();
        }
        assert!(parent.is_cancelled() && child.is_cancelled());
    }

    #[test]
    fn telemetry_carries_to_child() {
        let telemetry: Arc<dyn TelemetryContext> = Arc::new(Recorder(Mutex::new(Vec::new())));
        let context = Context::new().with_telemetry(telemetry.clone());
        assert!(Arc::ptr_eq(context.telemetry().unwrap(), &telemetry));
        assert!(context.child().telemetry().is_some());
    }

    #[test]
    fn empty_context_has_no_signal_or_telemetry() {
        let context = Context::new();
        assert!(
            context.abort_signal.is_none()
                && context.telemetry.is_none()
                && !context.is_cancelled()
        );
    }
}
