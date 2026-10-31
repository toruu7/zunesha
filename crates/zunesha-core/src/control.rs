//! Cooperative cancellation for in-flight agent runs.
//!
//! A [`RunControl`] is a one-shot signal. The worker selects on it while the
//! agent process is running; cancelling wakes that select immediately, the
//! worker kills the whole process tree, and the task is released back to the
//! queue with a note explaining why.
//!
//! The registry is deliberately in-memory. Cancellation only makes sense for a
//! worker that is alive in this process; a task claimed by a dead daemon is
//! handled by the existing crash-recovery path (`zou recover` / boot resume).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::watch;

/// One task's cancellation signal.
///
/// Cloning is cheap and all clones observe the same state. The signal is
/// one-way: once cancelled, it stays cancelled.
#[derive(Clone, Debug)]
pub struct RunControl {
    tx: watch::Sender<bool>,
}

impl RunControl {
    /// Create a fresh, un-cancelled control.
    pub fn new() -> Self {
        let (tx, _rx) = watch::channel(false);
        Self { tx }
    }

    /// Request cancellation. Safe to call repeatedly and from any thread.
    ///
    /// `send_replace` updates the value even when no waiter has subscribed
    /// yet; plain `send` fails when the receiver count is zero, which would
    /// lose a cancel that arrives just before the worker starts waiting.
    pub fn cancel(&self) {
        self.tx.send_replace(true);
    }

    /// Whether cancellation has already been requested.
    pub fn is_cancelled(&self) -> bool {
        *self.tx.borrow()
    }

    /// Resolve as soon as cancellation is requested.
    ///
    /// Also resolves if the sender is dropped (for example because the worker
    /// finished), so callers never wait forever on a stale control.
    pub async fn cancelled(&self) {
        let mut rx = self.tx.subscribe();
        if *rx.borrow() {
            return;
        }
        while rx.changed().await.is_ok() {
            if *rx.borrow() {
                return;
            }
        }
    }
}

impl Default for RunControl {
    fn default() -> Self {
        Self::new()
    }
}

/// The set of tasks currently owned by live workers in this process.
///
/// The mutex is only held for map operations, never across an `await`.
#[derive(Clone, Debug, Default)]
pub struct RunRegistry {
    inner: Arc<Mutex<HashMap<String, RunControl>>>,
}

impl RunRegistry {
    /// Register a task and return its control.
    ///
    /// Re-registering an id replaces the previous control; callers only start a
    /// task after checking that it is `TODO`, so a duplicate should be
    /// impossible in normal operation.
    pub fn register(&self, task_id: &str) -> RunControl {
        let control = RunControl::new();
        self.lock().insert(task_id.to_string(), control.clone());
        control
    }

    /// Cancel `task_id` if it is currently running.
    ///
    /// Returns `true` when a live worker owned the task. `false` means it is
    /// not running (already finished, never started, or owned by another
    /// process).
    pub fn cancel(&self, task_id: &str) -> bool {
        match self.lock().get(task_id) {
            Some(control) => {
                control.cancel();
                true
            }
            None => false,
        }
    }

    /// Drop a finished task's control.
    pub fn remove(&self, task_id: &str) {
        self.lock().remove(task_id);
    }

    /// Ids currently owned by this process.
    pub fn active(&self) -> Vec<String> {
        self.lock().keys().cloned().collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, RunControl>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancel_wakes_a_waiter() {
        let control = RunControl::new();
        assert!(!control.is_cancelled());
        let waiter = control.clone();
        let handle = tokio::spawn(async move { waiter.cancelled().await });
        control.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .expect("cancel should wake the waiter")
            .unwrap();
        assert!(control.is_cancelled());
    }

    #[tokio::test]
    async fn cancelling_before_waiting_returns_immediately() {
        let control = RunControl::new();
        control.cancel();
        assert!(
            control.is_cancelled(),
            "cancel must update the shared value"
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), control.cancelled())
            .await
            .expect("already-cancelled control should resolve");
    }

    #[tokio::test]
    async fn a_clone_sees_a_cancel_sent_through_the_registry() {
        let registry = RunRegistry::default();
        let control = registry.register("t1");
        let waiter = control.clone();
        let handle = tokio::spawn(async move { waiter.cancelled().await });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(registry.cancel("t1"));
        tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .expect("clone should wake")
            .unwrap();
    }

    #[test]
    fn registry_reports_only_live_tasks() {
        let registry = RunRegistry::default();
        let control = registry.register("t1");
        assert_eq!(registry.active(), vec!["t1".to_string()]);
        assert!(registry.cancel("t1"));
        assert!(control.is_cancelled());
        assert!(!registry.cancel("missing"));
        registry.remove("t1");
        assert!(registry.active().is_empty());
    }
}
