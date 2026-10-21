//! Effectful adapters. No network listener, wire identity parser, or provider client.
use crate::{DeadManSwitch, Effect, Outcome};
use m8s_core::Timestamp;
use std::ffi::OsString;
use std::io;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

/// Integration trust boundary: only return true after authenticated sealed bootstrap,
/// certificate verification and enforcement of the received launch policy. This is
/// not a readiness assertion. Implementations must not accept claimed wire identities.
pub trait BootGate {
    fn authorize(&mut self) -> bool;
}

/// Production remains fail closed until the secure bootstrap integration is qualified.
pub struct UnavailableBoot;
impl BootGate for UnavailableBoot {
    fn authorize(&mut self) -> bool {
        false
    }
}

/// Explicit test/local-only opt-in. Never use as production admission evidence.
pub struct LocalOnlyBoot;
impl BootGate for LocalOnlyBoot {
    fn authorize(&mut self) -> bool {
        true
    }
}

/// Deliberately not Debug/Serialize. Arguments must contain no secret values;
/// integration must deliver join material by protected files, not argv.
pub struct ChildSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

/// Drivers must perform bounded, nonblocking work per call (including timeouts on
/// any future I/O). ConfirmAbsent must return ConfirmedAbsent only on explicit
/// provider-confirmed absence, never on stop, error, timeout or successful destroy.
/// Wipe must target only the instance's dedicated material, not arbitrary paths.
pub trait TeardownDriver {
    fn perform(&mut self, effect: Effect) -> Outcome;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeardownState {
    Running,
    Pending,
    ConfirmedAbsent,
}

/// One attempt per due tick, exponential backoff capped in injected seconds.
/// Retries never stop merely because the provider is uncertain.
pub struct RetryTeardown {
    next: Timestamp,
    delay: i64,
    cap: i64,
}
impl RetryTeardown {
    pub fn new(cap_seconds: i64) -> Result<Self, &'static str> {
        if cap_seconds < 1 {
            return Err("invalid retry cap");
        }
        Ok(Self {
            next: i64::MIN,
            delay: 1,
            cap: cap_seconds,
        })
    }
    fn due(&self, now: Timestamp) -> bool {
        now >= self.next
    }
    fn attempted(&mut self, now: Timestamp) {
        self.next = now.saturating_add(self.delay);
        self.delay = self.delay.saturating_mul(2).min(self.cap);
    }
}

/// Owns a dedicated Unix process group. Teardown kills the group and reaps the
/// direct child; the OS init must reap orphan descendants. Descendants that escape
/// the group are outside this boundary. Drop is a best-effort fallback. OS signal
/// forwarding still requires a qualified container init.
pub struct Supervisor<D: TeardownDriver> {
    child: Child,
    group_terminated: bool,
    dms: DeadManSwitch,
    driver: D,
    retry: RetryTeardown,
    state: TeardownState,
}
impl<D: TeardownDriver> Supervisor<D> {
    pub fn start(
        spec: ChildSpec,
        gate: &mut impl BootGate,
        dms: DeadManSwitch,
        driver: D,
        retry: RetryTeardown,
    ) -> io::Result<Self> {
        if !gate.authorize() {
            return Err(io::Error::other("secure bootstrap unavailable or rejected"));
        }
        // Allowlist is intentionally empty: no inherited credentials, bootstrap
        // nonce, provider key, stdin control channel, or child output leakage.
        let mut command = Command::new(spec.program);
        command
            .args(spec.args)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(not(unix))]
        return Err(io::Error::other("process-group supervision unavailable"));
        let child = command.spawn()?;
        Ok(Self {
            child,
            group_terminated: false,
            dms,
            driver,
            retry,
            state: TeardownState::Running,
        })
    }
    /// Caller must supply the authenticated transport endpoint, not a wire field.
    /// No transport implementation is supplied by this crate.
    pub fn heartbeat(&mut self, authenticated_peer: &str, now: Timestamp) -> bool {
        self.dms.heartbeat(authenticated_peer, now)
    }
    pub fn shutdown(&mut self) {
        self.dms.trigger();
    }
    pub fn child_id(&self) -> u32 {
        self.child.id()
    }
    /// This supervisor exclusively owns reaping its child. External waitpid /
    /// SIGCHLD auto-reaping is unsupported and detected as an error, not absence.
    pub fn child_exited(&mut self) -> io::Result<bool> {
        if self.group_terminated {
            return Ok(self.child.try_wait()?.is_some());
        }
        let exited = self.observe_exit()?;
        if exited {
            // Keep the unreaped leader PID reserved until its group is killed.
            self.stop_child()?;
        }
        Ok(exited)
    }
    #[cfg(unix)]
    fn observe_exit(&self) -> io::Result<bool> {
        // SAFETY: zero is a valid initial siginfo_t; waitid fills it on exit.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // WNOWAIT is essential: try_wait would reap the leader before group
        // signalling and allow its numeric PID/PGID to be reused by another job.
        // SAFETY: info is valid writable storage; only our direct child is selected.
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                self.child.id() as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: waitid initialized the exited-child identity, or left zero
        // when WNOHANG found no exit event.
        Ok(unsafe { info.si_pid() } != 0)
    }
    #[cfg(not(unix))]
    fn observe_exit(&self) -> io::Result<bool> {
        Err(io::Error::other("process-group supervision unavailable"))
    }
    fn stop_child(&mut self) -> io::Result<()> {
        if !self.group_terminated {
            #[cfg(unix)]
            {
                // Refuse to signal if exclusive ownership has been lost (ECHILD).
                // A live child or an unreaped leader reserves the numeric PID.
                self.observe_exit()?;
                let pid = i32::try_from(self.child.id())
                    .ok()
                    .filter(|pid| *pid > 1)
                    .ok_or_else(|| io::Error::other("invalid process group"))?;
                // SAFETY: getpgrp has no pointer arguments or side effects.
                if pid == unsafe { libc::getpgrp() } {
                    return Err(io::Error::other("refusing own process group"));
                }
                // SAFETY: process_group(0) created this child as group leader;
                // its PID remains unreaped and reserved. Checked pid > 1 makes
                // negation safe and excludes kill(0) and kill(-1) semantics.
                let result = unsafe { libc::kill(-pid, libc::SIGKILL) };
                if result != 0 {
                    let error = io::Error::last_os_error();
                    // ESRCH: the group has already exited, not provider absence.
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        return Err(error);
                    }
                }
            }
            // Never signal this numeric group again after reaping its leader.
            self.group_terminated = true;
        }
        self.child.wait()?;
        Ok(())
    }
    pub fn tick(&mut self, now: Timestamp) -> io::Result<TeardownState> {
        if self.state == TeardownState::ConfirmedAbsent {
            return Ok(self.state);
        }
        match self.child_exited() {
            Ok(false) => {}
            // Cleanup uncertainty also latches spend containment. Do not let
            // local OS denial suppress the provider teardown driver attempts.
            Ok(true) | Err(_) => self.dms.trigger(),
        }
        let effects = self.dms.tick(now);
        if effects.is_empty() {
            return Ok(self.state);
        }
        self.state = TeardownState::Pending;
        if !self.retry.due(now) {
            return Ok(self.state);
        }
        // Drain failure cannot prevent child termination, wipe or spend containment.
        let mut stopped = false;
        for effect in effects {
            let mut outcome = self.driver.perform(effect);
            if effect == Effect::Drain {
                stopped = self.stop_child().is_ok();
                if !stopped {
                    outcome = Outcome::Failure;
                }
            }
            self.dms.complete(effect, outcome);
            if effect == Effect::ConfirmAbsent && outcome == Outcome::ConfirmedAbsent {
                self.state = TeardownState::ConfirmedAbsent;
            }
        }
        // Schedule the next attempt even when local OS cleanup fails, otherwise
        // an error would bypass backoff and hammer a future provider adapter.
        self.retry.attempted(now);
        // Even after an earlier successful drain, enforce stop on every attempt.
        if !stopped {
            self.stop_child()?;
        }
        Ok(self.state)
    }
}
impl<D: TeardownDriver> Drop for Supervisor<D> {
    fn drop(&mut self) {
        let _ = self.stop_child();
    }
}
