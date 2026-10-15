//! Admission for application-owned local compute. Recording intent outranks new work.
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

#[derive(Clone, Default, Debug)]
pub struct ResourceScheduler(Arc<Inner>);
#[derive(Default, Debug)]
struct Inner {
    state: Mutex<State>,
    changed: Notify,
}
#[derive(Default, Debug)]
struct State {
    next: u64,
    active: Option<u64>,
    recording_intent: Option<u64>,
    uncertain: Option<String>,
}
#[derive(Clone, Debug)]
pub struct ResourceLease(Arc<LeaseInner>);
#[derive(Debug)]
struct LeaseInner {
    scheduler: ResourceScheduler,
    id: u64,
}
pub struct RecordingIntent {
    scheduler: ResourceScheduler,
    id: u64,
}

impl ResourceScheduler {
    pub fn new() -> Self {
        Self::default()
    }
    fn lease(&self, state: &mut State) -> ResourceLease {
        state.next += 1;
        let id = state.next;
        state.active = Some(id);
        ResourceLease(Arc::new(LeaseInner {
            scheduler: self.clone(),
            id,
        }))
    }
    fn ready(state: &State) -> Result<(), String> {
        match &state.uncertain {
            Some(reason)=>Err(format!("BUSY: Local resource cleanup could not be confirmed ({reason}). Stop the local inference service and restart Meetily before retrying.")),
            None=>Ok(()),
        }
    }
    pub fn try_local(&self) -> Result<ResourceLease, String> {
        let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
        Self::ready(&state)?;
        if state.active.is_some() || state.recording_intent.is_some() {
            return Err("BUSY: Recording is starting, active or finishing, or another local task is running. Retry after it finishes.".into());
        }
        Ok(self.lease(&mut state))
    }
    pub async fn local(&self) -> Result<ResourceLease, String> {
        loop {
            // Register before inspecting state; notify_waiters must not be lost.
            let changed = self.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
                Self::ready(&state)?;
                if state.active.is_none() && state.recording_intent.is_none() {
                    return Ok(self.lease(&mut state));
                }
            }
            changed.await;
        }
    }
    pub fn recording_intent(&self) -> Result<RecordingIntent, String> {
        let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
        Self::ready(&state)?;
        if state.recording_intent.is_some() {
            return Err("BUSY: Recording start is already waiting.".into());
        }
        state.next += 1;
        let id = state.next;
        state.recording_intent = Some(id);
        Ok(RecordingIntent {
            scheduler: self.clone(),
            id,
        })
    }
}
impl RecordingIntent {
    pub async fn acquire(self) -> Result<ResourceLease, String> {
        loop {
            let changed = self.scheduler.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut state = self
                    .scheduler
                    .0
                    .state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                ResourceScheduler::ready(&state)?;
                if state.active.is_none() {
                    state.recording_intent = None;
                    return Ok(self.scheduler.lease(&mut state));
                }
            }
            changed.await;
        }
    }
}
impl Drop for RecordingIntent {
    fn drop(&mut self) {
        let mut state = self
            .scheduler
            .0
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if state.recording_intent == Some(self.id) {
            state.recording_intent = None;
        }
        drop(state);
        self.scheduler.0.changed.notify_waiters();
    }
}
impl ResourceLease {
    pub fn uncertain(&self, reason: &str) {
        self.0
            .scheduler
            .0
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .uncertain = Some(reason.into());
        self.0.scheduler.0.changed.notify_waiters();
    }
    pub fn operation(&self, reason: &'static str) -> ResourceOperation {
        ResourceOperation {
            lease: self.clone(),
            reason,
            finished: false,
        }
    }
}
impl Drop for LeaseInner {
    fn drop(&mut self) {
        let mut state = self
            .scheduler
            .0
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if state.active == Some(self.id) {
            state.active = None;
        }
        drop(state);
        self.scheduler.0.changed.notify_waiters();
    }
}
/// An async computation dropped or unwound before confirmed cleanup must not
/// silently admit recording while its native/HTTP work may still be running.
pub struct ResourceOperation {
    lease: ResourceLease,
    reason: &'static str,
    finished: bool,
}
impl ResourceOperation {
    pub fn uncertain(&self, reason: &str) {
        self.lease.uncertain(reason);
    }
    pub fn finish(&mut self) {
        self.finished = true;
    }
}
impl Drop for ResourceOperation {
    fn drop(&mut self) {
        if !self.finished {
            self.lease.uncertain(self.reason);
        }
    }
}

tokio::task_local! { pub static LOCAL_WORK: ResourceLease; }
pub fn local_inference_guard() -> Result<Option<ResourceOperation>, String> {
    LOCAL_WORK
        .try_with(|lease| {
            ResourceScheduler::ready(
                &lease
                    .0
                    .scheduler
                    .0
                    .state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()),
            )?;
            Ok(Some(
                lease.operation("Local inference did not confirm completion"),
            ))
        })
        .unwrap_or(Ok(None))
}
/// Only fixed cloud providers are exempt. Configurable endpoints (including
/// remote Ollama/custom hosts) conservatively share local admission.
pub fn summary_uses_local_resources(provider: &str) -> bool {
    !matches!(
        provider.to_ascii_lowercase().as_str(),
        "openai" | "claude" | "groq" | "openrouter"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[tokio::test]
    async fn recording_intent_blocks_new_work_and_waits_for_existing_work() {
        let scheduler = ResourceScheduler::new();
        let work = scheduler.try_local().unwrap();
        let intent = scheduler.recording_intent().unwrap();
        assert!(scheduler.try_local().is_err());
        let mut recording = Box::pin(intent.acquire());
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut recording)
                .await
                .is_err()
        );
        drop(work);
        let recording = recording.await.unwrap();
        assert!(scheduler.try_local().is_err());
        drop(recording);
        assert!(scheduler.try_local().is_ok());
    }
    #[tokio::test]
    async fn timeout_or_dropped_intent_restores_admission() {
        let scheduler = ResourceScheduler::new();
        let work = scheduler.try_local().unwrap();
        assert!(tokio::time::timeout(
            Duration::from_millis(10),
            scheduler.recording_intent().unwrap().acquire()
        )
        .await
        .is_err());
        drop(work);
        assert!(scheduler.try_local().is_ok());
        drop(scheduler.recording_intent().unwrap());
        assert!(scheduler.try_local().is_ok());
    }
    #[tokio::test]
    async fn queued_local_work_cannot_pass_recording_intent() {
        let scheduler = ResourceScheduler::new();
        let work = scheduler.try_local().unwrap();
        let mut queued = Box::pin(scheduler.local());
        assert!(tokio::time::timeout(Duration::from_millis(10), &mut queued)
            .await
            .is_err());
        let intent = scheduler.recording_intent().unwrap();
        drop(work);
        assert!(tokio::time::timeout(Duration::from_millis(10), &mut queued)
            .await
            .is_err());
        let recording = intent.acquire().await.unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(10), &mut queued)
            .await
            .is_err());
        drop(recording);
        assert!(queued.await.is_ok());
    }
    #[tokio::test]
    async fn interrupted_cleanup_blocks_recording_and_wakes_queued_work() {
        let scheduler = ResourceScheduler::new();
        let work = scheduler.try_local().unwrap();
        let guard = work.operation("test cleanup failure");
        let intent = scheduler.recording_intent().unwrap();
        drop(guard);
        drop(work);
        assert!(intent
            .acquire()
            .await
            .unwrap_err()
            .contains("cleanup could not be confirmed"));
        assert!(scheduler.local().await.is_err());
        assert!(scheduler.try_local().is_err());
    }
    #[test]
    fn confirmed_error_cleanup_and_startup_failure_restore_admission() {
        let scheduler = ResourceScheduler::new();
        let work = scheduler.try_local().unwrap();
        let mut guard = work.operation("test");
        guard.finish();
        drop(guard);
        drop(work);
        assert!(scheduler.try_local().is_ok());
    }
    #[test]
    fn aliases_and_configurable_endpoints_are_local() {
        for name in [
            "builtin-ai",
            "local-llama",
            "localllama",
            "OLLAMA",
            "custom-openai",
            "",
        ] {
            assert!(summary_uses_local_resources(name));
        }
        for name in ["OpenAI", "Claude", "groq", "OpenRouter"] {
            assert!(!summary_uses_local_resources(name));
        }
    }
    #[tokio::test]
    async fn concurrent_recording_intent_and_batch_admission_are_atomic() {
        for _ in 0..32 {
            let scheduler = ResourceScheduler::new();
            let barrier = Arc::new(tokio::sync::Barrier::new(2));
            let other = scheduler.clone();
            let gate = barrier.clone();
            let batch = tokio::spawn(async move {
                gate.wait().await;
                other.try_local()
            });
            barrier.wait().await;
            let intent = scheduler.recording_intent().unwrap();
            let work = batch.await.unwrap();
            assert!(scheduler.try_local().is_err());
            if let Ok(work) = work {
                drop(work);
            }
            let recording = intent.acquire().await.unwrap();
            assert!(scheduler.try_local().is_err());
            drop(recording);
        }
    }
    #[test]
    fn panic_during_native_work_never_releases_as_idle() {
        let scheduler = ResourceScheduler::new();
        let work = scheduler.try_local().unwrap();
        let _ = std::panic::catch_unwind(|| {
            let _guard = work.operation("panic");
            panic!("synthetic");
        });
        drop(work);
        assert!(scheduler.recording_intent().is_err());
    }
}
