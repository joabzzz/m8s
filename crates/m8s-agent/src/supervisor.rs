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
/// Adapter destructors must also remain bounded; arbitrary user code is not
/// preempted by the synchronous coordinator.
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

/// Direct-child cleanup evidence only, never provider or whole-tree absence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalTerminationState {
    Running,
    StopPending,
    DirectChildReaped,
    Uncertain,
}

// Private trusted seam: no production constructor accepts synthetic operations.
// Each operation must be bounded/nonblocking. In particular observe_exit must
// preserve the leader PID (WNOWAIT), and try_reap_exited must never wait for exit.
trait ProcessOps: Send + Sync {
    fn id(&self) -> u32;
    fn observe_exit(&mut self) -> io::Result<bool>;
    fn signal_group(&mut self) -> io::Result<()>;
    fn try_reap_exited(&mut self) -> io::Result<bool>;
}
struct LocalProcess {
    child: Child,
}
impl ProcessOps for LocalProcess {
    fn id(&self) -> u32 {
        self.child.id()
    }
    #[cfg(unix)]
    fn observe_exit(&mut self) -> io::Result<bool> {
        // SAFETY: zero is valid initial storage; waitid fills an exit identity,
        // or leaves zero when WNOHANG found no exit event.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // WNOWAIT reserves the leader's PID until its group has been signalled.
        // SAFETY: valid writable storage, selecting only our direct child;
        // WNOHANG never waits for a live child to exit.
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
        // SAFETY: waitid initialized the exited-child identity or left zero.
        Ok(unsafe { info.si_pid() } != 0)
    }
    #[cfg(not(unix))]
    fn observe_exit(&mut self) -> io::Result<bool> {
        Err(io::Error::other("process-group supervision unavailable"))
    }
    fn signal_group(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        {
            // Refuse signalling if exclusive ownership was lost (ECHILD).
            // Neither this check nor the coordinator reaps the reserved leader.
            self.observe_exit()?;
            let pid = i32::try_from(self.child.id())
                .ok()
                .filter(|pid| *pid > 1)
                .ok_or_else(|| io::Error::other("invalid process group"))?;
            // SAFETY: getpgrp has no pointers or side effects.
            if pid == unsafe { libc::getpgrp() } {
                return Err(io::Error::other("refusing own process group"));
            }
            // SAFETY: process_group(0) created this reserved leader; checked
            // pid > 1 excludes own-group/broadcast and negation overflow.
            let result = unsafe { libc::kill(-pid, libc::SIGKILL) };
            if result != 0 {
                let error = io::Error::last_os_error();
                // ESRCH means only the local group disappeared, not the host.
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(error);
                }
            }
            Ok(())
        }
        #[cfg(not(unix))]
        Err(io::Error::other("process-group supervision unavailable"))
    }
    fn try_reap_exited(&mut self) -> io::Result<bool> {
        // Called only after WNOWAIT reports exit AND group signalling succeeded.
        // try_wait itself is WNOHANG, never Child::wait even on that path.
        Ok(self.child.try_wait()?.is_some())
    }
}

/// Owns a dedicated Unix process group with nonblocking stop/reap progress.
/// Retain and tick until DirectChildReaped, or explicitly transfer cleanup to a
/// qualified reaper. Drop performs one best-effort step; it never waits/spins.
/// OS init reaps orphan descendants; escaped groups are outside this boundary.
/// Nonblocking OS calls and all driver calls must themselves return promptly;
/// this coordinator cannot enforce deadlines against a stalled kernel/driver.
pub struct Supervisor<D: TeardownDriver> {
    #[cfg(feature = "authenticated-control")]
    control_binding: crate::control::ControlBinding,
    process: Box<dyn ProcessOps>,
    #[cfg(all(unix, feature = "sealed-node"))]
    _node_material: Option<crate::node::NodeMaterial>,
    group_signalled: bool,
    stopping: bool,
    reaped: bool,
    local_error: Option<io::ErrorKind>,
    dms: DeadManSwitch,
    driver: D,
    retry: RetryTeardown,
    state: TeardownState,
}
impl<D: TeardownDriver> Supervisor<D> {
    /// Starts an unbound supervisor. Raw heartbeats remain a trusted library seam;
    /// verified control evidence is refused without a pinned local target.
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
        let mut command = Command::new(spec.program);
        command
            .args(spec.args)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        Self::spawn_command(command, dms, driver, retry)
    }
    fn spawn_command(
        mut command: Command,
        dms: DeadManSwitch,
        driver: D,
        retry: RetryTeardown,
    ) -> io::Result<Self> {
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(not(unix))]
        return Err(io::Error::other("process-group supervision unavailable"));
        let child = command.spawn()?;
        Ok(Self {
            #[cfg(feature = "authenticated-control")]
            control_binding: crate::control::ControlBinding::default(),
            process: Box::new(LocalProcess { child }),
            #[cfg(all(unix, feature = "sealed-node"))]
            _node_material: None,
            group_signalled: false,
            stopping: false,
            reaped: false,
            local_error: None,
            dms,
            driver,
            retry,
            state: TeardownState::Running,
        })
    }
    /// Consume staged files while retaining the existing BootGate and pinned
    /// heartbeat semantics. No standalone binary route or default admission.
    #[cfg(all(unix, feature = "sealed-node"))]
    pub fn start_mesh(
        material: crate::node::NodeMaterial,
        gate: &mut impl BootGate,
        dms: DeadManSwitch,
        driver: D,
        retry: RetryTeardown,
        verifier: &m8s_bootstrap::mp001::Mp001CertificateVerifier,
        now: Timestamp,
    ) -> io::Result<Self> {
        material
            .recheck(verifier, now)
            .and_then(|()| material.check_dms(&dms, now))
            .map_err(|_| io::Error::other("node material or policy unavailable"))?;
        let mut command = material
            .command()
            .map_err(|_| io::Error::other("node launch policy unavailable"))?;
        if !gate.authorize() {
            return Err(io::Error::other("secure bootstrap unavailable or rejected"));
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let node = material.node();
        let mut supervisor = Self::spawn_command(command, dms, driver, retry)?;
        supervisor.control_binding = crate::control::ControlBinding::pinned(node);
        supervisor._node_material = Some(material);
        Ok(supervisor)
    }
    /// Pins this supervisor's local TLS node from trusted owner/boot configuration.
    /// This configuration is NOT evidence that bootstrap/admission succeeded.
    /// No listener, handler or transport is started; the BootGate still applies.
    #[cfg(feature = "authenticated-control")]
    pub fn start_with_control_node(
        spec: ChildSpec,
        gate: &mut impl BootGate,
        dms: DeadManSwitch,
        driver: D,
        retry: RetryTeardown,
        local_node: crate::control::EndpointId,
    ) -> io::Result<Self> {
        let mut supervisor = Self::start(spec, gate, dms, driver, retry)?;
        supervisor.control_binding = crate::control::ControlBinding::pinned(local_node);
        Ok(supervisor)
    }

    /// Applies one opaque received heartbeat, using the stored target, not a
    /// caller-selected per-event node. Bootstrap retains all live authorization
    /// checks and its read guard through the short, nonblocking DMS update.
    #[cfg(feature = "authenticated-control")]
    pub fn verified_heartbeat(
        &mut self,
        evidence: crate::control::VerifiedHeartbeat<'_>,
        now: Timestamp,
    ) -> Result<bool, crate::control::ControlError> {
        self.control_binding.consume(evidence, now, &mut self.dms)
    }

    /// Caller must supply an authenticated transport endpoint, never a wire field.
    /// Trusted backward-compatible library API only, not a network/stdin entrypoint.
    pub fn heartbeat(&mut self, authenticated_peer: &str, now: Timestamp) -> bool {
        self.dms.heartbeat(authenticated_peer, now)
    }
    pub fn shutdown(&mut self) {
        self.dms.trigger();
        self.stopping = true;
    }
    pub fn child_id(&self) -> u32 {
        self.process.id()
    }
    pub fn local_termination_state(&self) -> LocalTerminationState {
        if self.reaped {
            LocalTerminationState::DirectChildReaped
        } else if self.local_error.is_some() {
            LocalTerminationState::Uncertain
        } else if self.stopping {
            LocalTerminationState::StopPending
        } else {
            LocalTerminationState::Running
        }
    }
    fn inspect_natural_exit(&mut self) -> io::Result<()> {
        if self.stopping || self.reaped {
            return Ok(());
        }
        match self.process.observe_exit() {
            Ok(false) => Ok(()),
            Ok(true) => {
                self.shutdown();
                Ok(())
            }
            Err(error) => {
                self.shutdown();
                self.local_error = Some(error.kind());
                Err(error)
            }
        }
    }
    /// One nonblocking poll/cleanup step; true only after the direct child was
    /// reaped. False is pending, never proof of local/provider absence. This
    /// supervisor must remain the exclusive reaper until ownership is dropped.
    pub fn child_exited(&mut self) -> io::Result<bool> {
        self.inspect_natural_exit()?;
        if self.stopping {
            self.progress_stop()?;
        }
        Ok(self.reaped)
    }
    fn progress_stop(&mut self) -> io::Result<()> {
        if self.reaped {
            return Ok(());
        }
        self.stopping = true;
        let result = (|| {
            let exited = self.process.observe_exit()?;
            if !self.group_signalled {
                self.process.signal_group()?;
                // Never signal this numeric group again, especially after reap.
                // Accepted SIGKILL is NOT evidence the child has exited.
                self.group_signalled = true;
            }
            if exited || self.process.observe_exit()? {
                self.reaped = self.process.try_reap_exited()?;
            }
            Ok(())
        })();
        self.local_error = result.as_ref().err().map(io::Error::kind);
        result
    }
    /// Provider containment state is independent of local stop/reap progress.
    /// Local errors are recorded as Uncertain, not returned to abort a watchdog
    /// retry loop. Call local_termination_state for that separate evidence.
    pub fn tick(&mut self, now: Timestamp) -> io::Result<TeardownState> {
        let _ = self.inspect_natural_exit();
        let effects = self.dms.tick(now);
        if !effects.is_empty() {
            self.stopping = true;
        }
        let mut progressed = false;
        if !effects.is_empty() && self.retry.due(now) {
            self.state = TeardownState::Pending;
            // Commit deadline regardless of pending/error local operations.
            self.retry.attempted(now);
            for effect in effects {
                let mut outcome = self.driver.perform(effect);
                if effect == Effect::Drain {
                    progressed = true;
                    let _ = self.progress_stop();
                    if !self.reaped {
                        outcome = Outcome::Failure;
                    }
                }
                // No error/pending local stop can skip these remaining effects,
                // including the independent Destroy/ConfirmAbsent attempts.
                self.dms.complete(effect, outcome);
                if effect == Effect::ConfirmAbsent && outcome == Outcome::ConfirmedAbsent {
                    self.state = TeardownState::ConfirmedAbsent;
                }
            }
        }
        // Poll local cleanup between provider backoff deadlines and even after
        // explicit provider absence. No provider calls bypass their backoff.
        if self.stopping && !progressed {
            let _ = self.progress_stop();
        }
        Ok(self.state)
    }
}
impl<D: TeardownDriver> Drop for Supervisor<D> {
    fn drop(&mut self) {
        // Exactly one bounded step. No wait, sleep, spin or background reaper.
        let _ = self.progress_stop();
    }
}

#[cfg(test)]
mod nonblocking_tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    #[derive(Default)]
    struct Operations {
        exited: bool,
        reaped: bool,
        killed: bool,
        observe_error: bool,
        kill_error: bool,
        calls: Vec<&'static str>,
    }
    struct SyntheticProcess(Arc<Mutex<Operations>>);
    impl ProcessOps for SyntheticProcess {
        fn id(&self) -> u32 {
            42
        }
        fn observe_exit(&mut self) -> io::Result<bool> {
            let mut ops = self.0.lock().unwrap();
            ops.calls.push("observe");
            assert!(!ops.reaped, "no operation may touch a reaped/reusable PID");
            if ops.observe_error {
                Err(io::Error::from_raw_os_error(libc::ECHILD))
            } else {
                Ok(ops.exited)
            }
        }
        fn signal_group(&mut self) -> io::Result<()> {
            let mut ops = self.0.lock().unwrap();
            ops.calls.push("signal");
            assert!(!ops.reaped);
            assert!(!ops.killed, "accepted signal must not be repeated");
            if ops.kill_error {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            } else {
                ops.killed = true;
                Ok(())
            }
        }
        fn try_reap_exited(&mut self) -> io::Result<bool> {
            let mut ops = self.0.lock().unwrap();
            ops.calls.push("reap");
            assert!(ops.exited, "reap only after WNOWAIT exit evidence");
            assert!(
                ops.killed,
                "signal group before releasing reserved leader PID"
            );
            assert!(!ops.reaped);
            ops.reaped = true;
            Ok(true)
        }
    }
    struct Driver {
        calls: Arc<Mutex<Vec<Effect>>>,
        absent: bool,
    }
    impl TeardownDriver for Driver {
        fn perform(&mut self, effect: Effect) -> Outcome {
            self.calls.lock().unwrap().push(effect);
            if effect == Effect::ConfirmAbsent && self.absent {
                Outcome::ConfirmedAbsent
            } else if effect == Effect::Wipe {
                Outcome::Failure
            } else {
                Outcome::Success
            }
        }
    }
    fn synthetic(
        ops: Arc<Mutex<Operations>>,
        absent: bool,
    ) -> (Supervisor<Driver>, Arc<Mutex<Vec<Effect>>>) {
        let calls = Arc::new(Mutex::new(vec![]));
        let supervisor = Supervisor {
            #[cfg(feature = "authenticated-control")]
            control_binding: crate::control::ControlBinding::default(),
            process: Box::new(SyntheticProcess(ops)),
            #[cfg(all(unix, feature = "sealed-node"))]
            _node_material: None,
            group_signalled: false,
            stopping: false,
            reaped: false,
            local_error: None,
            dms: DeadManSwitch::new("owner".into(), vec![], 0, 10, 100).unwrap(),
            driver: Driver {
                calls: calls.clone(),
                absent,
            },
            retry: RetryTeardown::new(4).unwrap(),
            state: TeardownState::Running,
        };
        (supervisor, calls)
    }
    fn destroys(calls: &Arc<Mutex<Vec<Effect>>>) -> usize {
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|e| **e == Effect::Destroy)
            .count()
    }
    #[test]
    fn never_exits_after_kill_cannot_block_destroy_or_backoff() {
        let ops = Arc::new(Mutex::new(Operations::default()));
        let (mut supervisor, calls) = synthetic(ops.clone(), false);
        supervisor.shutdown();
        for (now, attempts) in [
            (0, 1),
            (0, 1),
            (1, 2),
            (2, 2),
            (3, 3),
            (6, 3),
            (7, 4),
            (10, 4),
            (11, 5),
            (15, 6),
        ] {
            let before = ops.lock().unwrap().calls.len();
            assert_eq!(supervisor.tick(now).unwrap(), TeardownState::Pending);
            assert_eq!(
                supervisor.local_termination_state(),
                LocalTerminationState::StopPending
            );
            assert_eq!(destroys(&calls), attempts);
            // Fixed maximum operations per tick: no spin/poll-until-exit loop.
            assert!(ops.lock().unwrap().calls.len() - before <= 3);
        }
        assert!(ops.lock().unwrap().killed);
        assert!(!ops.lock().unwrap().reaped);
        assert!(!supervisor.child_exited().unwrap());
        let before = ops.lock().unwrap().calls.len();
        drop(supervisor);
        assert!(ops.lock().unwrap().calls.len() - before <= 3);
        assert_eq!(
            ops.lock()
                .unwrap()
                .calls
                .iter()
                .filter(|c| **c == "signal")
                .count(),
            1
        );
        assert!(!ops.lock().unwrap().calls.contains(&"reap"));
    }
    #[test]
    fn kill_denial_and_lost_reaper_ownership_do_not_suppress_containment() {
        for observe_error in [false, true] {
            let ops = Arc::new(Mutex::new(Operations {
                kill_error: true,
                observe_error,
                ..Default::default()
            }));
            let (mut supervisor, calls) = synthetic(ops.clone(), false);
            supervisor.shutdown();
            for (now, attempts) in [(0, 1), (0, 1), (1, 2), (3, 3), (7, 4)] {
                assert_eq!(supervisor.tick(now).unwrap(), TeardownState::Pending);
                assert_eq!(
                    supervisor.local_termination_state(),
                    LocalTerminationState::Uncertain
                );
                assert_eq!(destroys(&calls), attempts);
            }
            assert!(!ops.lock().unwrap().reaped);
            if observe_error {
                assert!(!ops.lock().unwrap().calls.contains(&"signal"));
            }
            let before = ops.lock().unwrap().calls.len();
            drop(supervisor);
            assert!(ops.lock().unwrap().calls.len() - before <= 3);
        }
    }
    #[test]
    fn only_reap_after_exit_and_never_signal_reusable_pid_after_reap() {
        let ops = Arc::new(Mutex::new(Operations::default()));
        let (mut supervisor, _) = synthetic(ops.clone(), false);
        supervisor.shutdown();
        supervisor.tick(0).unwrap();
        assert!(!ops.lock().unwrap().reaped);
        ops.lock().unwrap().exited = true;
        assert!(supervisor.child_exited().unwrap());
        assert_eq!(
            supervisor.local_termination_state(),
            LocalTerminationState::DirectChildReaped
        );
        let calls = ops.lock().unwrap().calls.len();
        assert!(supervisor.child_exited().unwrap());
        supervisor.tick(0).unwrap();
        drop(supervisor);
        assert_eq!(ops.lock().unwrap().calls.len(), calls);
    }
    #[test]
    fn explicit_provider_absence_does_not_fabricate_local_exit() {
        let ops = Arc::new(Mutex::new(Operations::default()));
        let (mut supervisor, calls) = synthetic(ops.clone(), true);
        supervisor.shutdown();
        assert_eq!(supervisor.tick(0).unwrap(), TeardownState::ConfirmedAbsent);
        assert_eq!(
            supervisor.local_termination_state(),
            LocalTerminationState::StopPending
        );
        assert!(!supervisor.child_exited().unwrap());
        assert_eq!(
            supervisor.tick(100).unwrap(),
            TeardownState::ConfirmedAbsent
        );
        assert_eq!(destroys(&calls), 1);
        assert!(!ops.lock().unwrap().reaped);
        ops.lock().unwrap().exited = true;
        supervisor.tick(101).unwrap();
        assert_eq!(
            supervisor.local_termination_state(),
            LocalTerminationState::DirectChildReaped
        );
        assert_eq!(destroys(&calls), 1);
    }
}
