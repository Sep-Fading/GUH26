//! Tracks sandbox cleanup so orderly shutdown can wait for durable revocation.

use std::{
    io,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct State {
    running: usize,
    failed: bool,
}

#[derive(Default)]
pub(super) struct CleanupQueue {
    state: Arc<(Mutex<State>, Condvar)>,
}

impl CleanupQueue {
    /// Registers work before spawning so a concurrent drain cannot miss it.
    pub(super) fn spawn(
        &self,
        work: impl FnOnce() -> io::Result<()> + Send + 'static,
    ) -> io::Result<()> {
        let mut state = self.state.0.lock().unwrap_or_else(|e| e.into_inner());
        state.running += 1;
        let shared = self.state.clone();
        let result = std::thread::Builder::new()
            .name("miniclaw-sandbox-cleanup".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
                let mut state = shared.0.lock().unwrap_or_else(|e| e.into_inner());
                state.failed |= !matches!(result, Ok(Ok(())));
                state.running -= 1;
                shared.1.notify_all();
            });
        if let Err(error) = result {
            state.running -= 1;
            state.failed = true;
            self.state.1.notify_all();
            return Err(error);
        }
        Ok(())
    }

    /// A timeout retains pending work and its journals; it never claims successful cleanup.
    pub(super) fn drain(&self, timeout: Duration) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        let mut state = self.state.0.lock().unwrap_or_else(|e| e.into_inner());
        while state.running != 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            state = self
                .state
                .1
                .wait_timeout(state, remaining)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        if state.failed {
            return Err(io::Error::other(
                "Sandbox cleanup failed; recovery journals were retained",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};

    #[test]
    fn drain_waits_for_cleanup_and_has_a_deadline() {
        let queue = CleanupQueue::default();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        queue
            .spawn(move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            })
            .unwrap();
        started_rx.recv().unwrap();
        assert_eq!(
            queue.drain(Duration::from_millis(10)).unwrap_err().kind(),
            std::io::ErrorKind::TimedOut
        );
        release_tx.send(()).unwrap();
        queue.drain(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn failed_or_panicked_cleanup_cannot_report_success() {
        for panic in [false, true] {
            let queue = CleanupQueue::default();
            queue
                .spawn(move || {
                    if panic {
                        panic!("cleanup fixture");
                    }
                    Err(std::io::Error::other("cleanup fixture"))
                })
                .unwrap();
            assert!(queue.drain(Duration::from_secs(2)).is_err());
            assert!(queue.drain(Duration::ZERO).is_err());
        }
    }
}
