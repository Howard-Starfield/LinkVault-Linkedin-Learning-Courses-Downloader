use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Condvar, Mutex,
    },
    time::Duration,
};

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitReason {
    Close,
    Exit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Resolution {
    Durable,
    Blocked,
}

#[derive(Clone, Copy, Debug)]
struct ActiveAttempt {
    token: u64,
    reason: ExitReason,
    resolution: Option<Resolution>,
}

#[derive(Default)]
struct Inner {
    next_token: AtomicU64,
    active: Mutex<Option<ActiveAttempt>>,
    changed: Condvar,
    exit_authorized: AtomicBool,
}

#[derive(Clone, Default)]
pub struct CooperativeExit {
    inner: Arc<Inner>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExitRequest {
    pub token: u64,
    pub started: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitOutcome {
    Durable(ExitReason),
    Blocked(ExitReason),
    TimedOut(ExitReason),
    Stale,
}

impl CooperativeExit {
    pub fn request(&self, reason: ExitReason) -> ExitRequest {
        let mut active = self
            .inner
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(attempt) = active.as_mut() {
            if reason == ExitReason::Exit {
                attempt.reason = ExitReason::Exit;
            }
            return ExitRequest {
                token: attempt.token,
                started: false,
            };
        }
        let token = self
            .inner
            .next_token
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        *active = Some(ActiveAttempt {
            token,
            reason,
            resolution: None,
        });
        ExitRequest {
            token,
            started: true,
        }
    }

    pub fn resolve(&self, token: u64, durable: bool) -> bool {
        let mut active = self
            .inner
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(attempt) = active.as_mut() else {
            return false;
        };
        if attempt.token != token || attempt.resolution.is_some() {
            return false;
        }
        attempt.resolution = Some(if durable {
            Resolution::Durable
        } else {
            Resolution::Blocked
        });
        self.inner.changed.notify_all();
        true
    }

    pub fn wait(&self, token: u64, timeout: Duration) -> WaitOutcome {
        let active = self
            .inner
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (mut active, timed_out) = self
            .inner
            .changed
            .wait_timeout_while(active, timeout, |active| {
                active
                    .as_ref()
                    .is_some_and(|attempt| attempt.token == token && attempt.resolution.is_none())
            })
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(attempt) = active.as_ref().copied() else {
            return WaitOutcome::Stale;
        };
        if attempt.token != token {
            return WaitOutcome::Stale;
        }
        *active = None;
        match attempt.resolution {
            Some(Resolution::Durable) => WaitOutcome::Durable(attempt.reason),
            Some(Resolution::Blocked) => WaitOutcome::Blocked(attempt.reason),
            None if timed_out.timed_out() => WaitOutcome::TimedOut(attempt.reason),
            None => WaitOutcome::Stale,
        }
    }

    pub fn authorize_exit(&self) {
        self.inner.exit_authorized.store(true, Ordering::Release);
    }

    pub fn consume_exit_authorization(&self) -> bool {
        self.inner.exit_authorized.swap(false, Ordering::AcqRel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cooperative_exit_deduplicates_and_upgrades_close_to_exit() {
        let state = CooperativeExit::default();
        let close = state.request(ExitReason::Close);
        let quit = state.request(ExitReason::Exit);
        assert!(close.started);
        assert!(!quit.started);
        assert_eq!(close.token, quit.token);
        assert!(state.resolve(close.token, true));
        assert_eq!(
            state.wait(close.token, Duration::from_secs(1)),
            WaitOutcome::Durable(ExitReason::Exit)
        );
    }

    #[test]
    fn cooperative_exit_rejects_stale_and_duplicate_resolutions() {
        let state = CooperativeExit::default();
        let first = state.request(ExitReason::Close);
        assert!(state.resolve(first.token, false));
        assert!(!state.resolve(first.token, true));
        assert_eq!(
            state.wait(first.token, Duration::ZERO),
            WaitOutcome::Blocked(ExitReason::Close)
        );
        let second = state.request(ExitReason::Close);
        assert!(!state.resolve(first.token, true));
        assert!(state.resolve(second.token, true));
        assert_eq!(
            state.wait(second.token, Duration::ZERO),
            WaitOutcome::Durable(ExitReason::Close)
        );
    }

    #[test]
    fn cooperative_exit_timeout_fails_closed_and_clears_attempt() {
        let state = CooperativeExit::default();
        let request = state.request(ExitReason::Exit);
        assert_eq!(
            state.wait(request.token, Duration::ZERO),
            WaitOutcome::TimedOut(ExitReason::Exit)
        );
        assert!(!state.resolve(request.token, true));
        assert!(state.request(ExitReason::Exit).started);
    }

    #[test]
    fn confirmed_exit_authorization_is_single_use() {
        let state = CooperativeExit::default();
        assert!(!state.consume_exit_authorization());
        state.authorize_exit();
        assert!(state.consume_exit_authorization());
        assert!(!state.consume_exit_authorization());
    }
}
