//! Bounded cleanup dispatch for a supervisor that must keep polling control,
//! signals and child exit while a provider request is in flight.
use super::TeardownDriver;
use crate::{Effect, Outcome};
use std::sync::mpsc::{self, Receiver, SyncSender};

/// At most one outstanding operation per effect and four queued results.
/// Dropping this handle disconnects dispatch without joining/blocking. Already
/// queued operations still run. This is NOT an independently surviving process
/// or a provider watchdog: the owner must retain external cleanup custody.
pub struct WorkerTeardown {
    requests: SyncSender<Effect>,
    results: Receiver<(Effect, Outcome)>,
    pending: [bool; 4],
    completed: [Option<Outcome>; 4],
}

fn index(effect: Effect) -> usize {
    match effect {
        Effect::Drain => 0,
        Effect::Wipe => 1,
        Effect::Destroy => 2,
        Effect::ConfirmAbsent => 3,
    }
}

impl WorkerTeardown {
    /// The operation must enforce its own I/O deadlines and exact ownership.
    /// It runs on a dedicated ordinary thread, suitable for blocking HTTPS
    /// clients. There are no hidden retries or automatic success conversions.
    pub fn spawn(
        mut operation: impl FnMut(Effect) -> Outcome + Send + 'static,
    ) -> std::io::Result<Self> {
        let (requests, incoming) = mpsc::sync_channel(4);
        let (outgoing, results) = mpsc::sync_channel(4);
        std::thread::Builder::new()
            .name("m8s-scoped-cleanup".into())
            .spawn(move || {
                while let Ok(effect) = incoming.recv() {
                    let outcome = operation(effect);
                    // If custody was dropped, finish queued operations anyway.
                    // Capacity is bounded by the four outstanding effect slots.
                    let _ = outgoing.try_send((effect, outcome));
                }
            })?;
        Ok(Self {
            requests,
            results,
            pending: [false; 4],
            completed: [None; 4],
        })
    }
}

impl TeardownDriver for WorkerTeardown {
    fn perform(&mut self, effect: Effect) -> Outcome {
        // A bounded drain avoids monopolizing the control executor.
        for _ in 0..4 {
            let Ok((finished, outcome)) = self.results.try_recv() else {
                break;
            };
            self.completed[index(finished)] = Some(outcome);
        }
        let slot = index(effect);
        if let Some(outcome) = self.completed[slot].take() {
            self.pending[slot] = false;
            return if outcome == Outcome::ConfirmedAbsent && effect != Effect::ConfirmAbsent {
                Outcome::Failure
            } else {
                outcome
            };
        }
        if !self.pending[slot] && self.requests.try_send(effect).is_ok() {
            self.pending[slot] = true;
        }
        // Pending, disconnected, full, or panicked worker is uncertainty.
        Outcome::Failure
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn await_result(driver: &mut WorkerTeardown, effect: Effect) -> Outcome {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let outcome = driver.perform(effect);
            if outcome != Outcome::Failure {
                return outcome;
            }
            assert!(Instant::now() < deadline, "synthetic worker timed out");
            std::thread::yield_now();
        }
    }

    #[test]
    fn pending_requests_never_block_or_duplicate_dispatch() {
        let (entered, started) = mpsc::sync_channel(1);
        let (release, blocked) = mpsc::sync_channel(1);
        let mut driver = WorkerTeardown::spawn(move |effect| {
            entered.send(effect).unwrap();
            blocked.recv().unwrap();
            Outcome::Success
        })
        .unwrap();
        assert_eq!(driver.perform(Effect::Destroy), Outcome::Failure);
        assert_eq!(
            started.recv_timeout(Duration::from_secs(2)).unwrap(),
            Effect::Destroy
        );
        for _ in 0..100 {
            assert_eq!(driver.perform(Effect::Destroy), Outcome::Failure);
        }
        release.send(()).unwrap();
        assert_eq!(await_result(&mut driver, Effect::Destroy), Outcome::Success);
        assert!(started.try_recv().is_err());
    }

    #[test]
    fn dropping_dispatch_retains_queued_cleanup_and_never_joins() {
        let (release, blocked) = mpsc::sync_channel(1);
        let (finished, completion) = mpsc::sync_channel(4);
        let mut driver = WorkerTeardown::spawn(move |effect| {
            if effect == Effect::Drain {
                blocked.recv().unwrap();
            }
            finished.send(effect).unwrap();
            Outcome::Failure
        })
        .unwrap();
        for effect in [
            Effect::Drain,
            Effect::Wipe,
            Effect::Destroy,
            Effect::ConfirmAbsent,
        ] {
            assert_eq!(driver.perform(effect), Outcome::Failure);
        }
        drop(driver);
        release.send(()).unwrap();
        for effect in [
            Effect::Drain,
            Effect::Wipe,
            Effect::Destroy,
            Effect::ConfirmAbsent,
        ] {
            assert_eq!(
                completion.recv_timeout(Duration::from_secs(2)).unwrap(),
                effect
            );
        }
    }

    #[test]
    fn only_explicit_confirmation_returns_absence() {
        let mut driver = WorkerTeardown::spawn(|_| Outcome::ConfirmedAbsent).unwrap();
        assert_eq!(
            await_result(&mut driver, Effect::ConfirmAbsent),
            Outcome::ConfirmedAbsent
        );
        // Even a faulty operation returning absence for DELETE is refused.
        driver.completed[index(Effect::Destroy)] = Some(Outcome::ConfirmedAbsent);
        assert_eq!(driver.perform(Effect::Destroy), Outcome::Failure);
    }
}
