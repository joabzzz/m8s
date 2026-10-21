//! Parent-alive watchdog: no journal operations, caller locks or blocking sends.
use super::*;
use std::{
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc::{Receiver, sync_channel},
    },
    thread::{self, Thread},
    time::{Duration, Instant, SystemTime},
};

const POLL: Duration = Duration::from_millis(50);
const STARTING: u8 = 0;
const RUNNING: u8 = 1;
const PENDING: u8 = 2;
const REAPED: u8 = 3;
const UNCERTAIN: u8 = 4;
const FAILED: u8 = 5;

/// Immutable real-clock scheduling assertion, not an admission/claim capability.
/// No public clock injection or renewal. A forward wall jump can stop early;
/// a backward jump never extends the anchored monotonic deadlines.
pub struct LocalDeadlines {
    stop: Instant,
    cleanup: Instant,
    stop_wall: SystemTime,
    cleanup_wall: SystemTime,
}
impl LocalDeadlines {
    pub fn new(stop_at: SystemTime, cleanup_at: SystemTime) -> Result<Self> {
        // Order deliberately conservative: elapsed sampling time shortens runtime.
        let mono = Instant::now();
        let wall = SystemTime::now();
        Self::anchored(stop_at, cleanup_at, mono, wall)
    }
    fn anchored(
        stop_at: SystemTime,
        cleanup_at: SystemTime,
        mono: Instant,
        wall: SystemTime,
    ) -> Result<Self> {
        let stop_delta = stop_at.duration_since(wall).map_err(|_| Error::Invalid)?;
        let cleanup_delta = cleanup_at
            .duration_since(wall)
            .map_err(|_| Error::Invalid)?;
        if stop_delta.is_zero() || cleanup_delta < stop_delta {
            return Err(Error::Invalid);
        }
        Ok(Self {
            stop: mono.checked_add(stop_delta).ok_or(Error::Invalid)?,
            cleanup: mono.checked_add(cleanup_delta).ok_or(Error::Invalid)?,
            stop_wall: stop_at,
            cleanup_wall: cleanup_at,
        })
    }
    fn stop_due(&self, mono: Instant, wall: SystemTime) -> bool {
        mono >= self.stop || wall >= self.stop_wall
    }
    fn cleanup_due(&self, mono: Instant, wall: SystemTime) -> bool {
        mono >= self.cleanup || wall >= self.cleanup_wall
    }
}

struct Shared {
    stop: AtomicBool,
    state: AtomicU8,
    cleanup_missed: AtomicBool,
    worker: OnceLock<Thread>,
}
/// Cloneable stop-only latch. It cannot launch, renew, join, signal PIDs or reap.
#[derive(Clone)]
pub struct LocalStopHandle(Arc<Shared>);
impl LocalStopHandle {
    pub fn request_stop(&self) {
        if !self.0.stop.swap(true, Ordering::AcqRel)
            && let Some(worker) = self.0.worker.get()
        {
            worker.unpark();
        }
    }
}
/// Move-only launch ownership. Drop requests stop without waiting for cleanup.
/// Stopped/reaped is not durable destruction or provider absence.
pub struct LocalLaunch {
    instance: Instance,
    stop: LocalStopHandle,
}
impl LocalLaunch {
    pub fn instance(&self) -> &Instance {
        &self.instance
    }
    pub fn stop_handle(&self) -> LocalStopHandle {
        self.stop.clone()
    }
    pub fn termination_state(&self) -> LocalTerminationState {
        termination_state(self.stop.0.state.load(Ordering::Acquire))
    }
    /// Latched when cleanup boundary was reached without direct-child reap.
    pub fn cleanup_deadline_missed(&self) -> bool {
        self.stop.0.cleanup_missed.load(Ordering::Acquire)
    }
}
impl Drop for LocalLaunch {
    fn drop(&mut self) {
        self.stop.request_stop();
    }
}
fn termination_state(state: u8) -> LocalTerminationState {
    match state {
        REAPED => LocalTerminationState::DirectChildReaped,
        PENDING => LocalTerminationState::StopPending,
        FAILED | UNCERTAIN => LocalTerminationState::Uncertain,
        _ => LocalTerminationState::Running,
    }
}

#[cfg(test)]
#[derive(Default)]
struct Hooks {
    after_spawn: Option<(
        std::sync::mpsc::SyncSender<()>,
        std::sync::mpsc::Receiver<()>,
    )>,
}

pub(super) struct TimedProcess(LocalStopHandle);
impl TimedProcess {
    pub(super) fn start(
        spec: ChildSpec,
        deadlines: LocalDeadlines,
        store: Arc<Mutex<storage::Store>>,
    ) -> Result<(Self, Receiver<Result<()>>)> {
        Self::start_inner(
            spec,
            deadlines,
            store,
            #[cfg(test)]
            Hooks::default(),
        )
    }
    fn start_inner(
        spec: ChildSpec,
        deadlines: LocalDeadlines,
        store: Arc<Mutex<storage::Store>>,
        #[cfg(test)] hooks: Hooks,
    ) -> Result<(Self, Receiver<Result<()>>)> {
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            state: AtomicU8::new(STARTING),
            cleanup_missed: AtomicBool::new(false),
            worker: OnceLock::new(),
        });
        let handle = LocalStopHandle(shared.clone());
        let (reply, started) = sync_channel(1);
        thread::Builder::new()
            .name("m8s-local-watchdog".into())
            .spawn(move || {
                // Keep the EXACT descriptor-opened directory/lock/state anchor alive.
                // Never invoke its methods from this thread.
                let _store = store;
                let _ = shared.worker.set(thread::current());
                if shared.stop.load(Ordering::Acquire)
                    || deadlines.stop_due(Instant::now(), SystemTime::now())
                {
                    shared.state.store(FAILED, Ordering::Release);
                    let _ = reply.try_send(Err(Error::Unavailable));
                    return; // durable Intent remains uncertain, even with no spawn
                }
                let process = start_process(spec);
                let process = match process {
                    Ok(process) => process,
                    Err(_) => {
                        shared.state.store(FAILED, Ordering::Release);
                        let _ = reply.try_send(Err(Error::Unavailable));
                        return;
                    }
                };
                #[cfg(test)]
                if let Some((entered, release)) = hooks.after_spawn {
                    let _ = entered.try_send(());
                    let _ = release.recv_timeout(Duration::from_secs(5));
                }
                // The worker owns the Supervisor from the moment spawn returns.
                // Late spawn is stopped BEFORE replying, never abandoned/retried.
                run_worker(process, shared, deadlines, _store, reply, RealRuntime);
            })
            .map_err(|_| Error::Unavailable)?;
        Ok((Self(handle), started))
    }
    pub(super) fn launch(&self, instance: Instance) -> LocalLaunch {
        LocalLaunch {
            instance,
            stop: self.0.clone(),
        }
    }
    pub(super) fn request_stop(&self) {
        self.0.request_stop();
    }
    pub(super) fn reaped(&self) -> bool {
        self.0.0.state.load(Ordering::Acquire) == REAPED
    }
    pub(super) fn child_exited(&self) -> Result<bool> {
        match self.0.0.state.load(Ordering::Acquire) {
            FAILED | UNCERTAIN => Err(Error::Unavailable),
            REAPED => Ok(true),
            _ => Ok(false),
        }
    }
    pub(super) fn acknowledge_stop(&self) -> Result<()> {
        match self.0.0.state.load(Ordering::Acquire) {
            FAILED | UNCERTAIN | STARTING | RUNNING => Err(Error::Unavailable),
            _ => Ok(()),
        }
    }
}
impl Drop for TimedProcess {
    fn drop(&mut self) {
        self.request_stop();
    }
}
fn start_process(spec: ChildSpec) -> Result<Process> {
    Process::start(
        spec,
        &mut LocalOnlyBoot,
        DeadManSwitch::new("synthetic-timed-local".into(), vec![], 0, 1, 1)
            .map_err(|_| Error::Invalid)?,
        NoExternalEffects,
        RetryTeardown::new(1).map_err(|_| Error::Invalid)?,
    )
    .map_err(|_| Error::Unavailable)
}
// Private scheduling seam only: no PID, signal or spawn logic is duplicated.
// Production always supplies the actual retained Supervisor and real runtime.
trait WorkerProcess {
    fn shutdown(&mut self);
    fn child_exited(&mut self) -> std::io::Result<bool>;
    fn local_termination_state(&self) -> LocalTerminationState;
}
impl WorkerProcess for Process {
    fn shutdown(&mut self) {
        Process::shutdown(self);
    }
    fn child_exited(&mut self) -> std::io::Result<bool> {
        Process::child_exited(self)
    }
    fn local_termination_state(&self) -> LocalTerminationState {
        Process::local_termination_state(self)
    }
}
trait WorkerRuntime {
    fn now(&self) -> (Instant, SystemTime);
    fn park(&mut self, duration: Duration);
}
struct RealRuntime;
impl WorkerRuntime for RealRuntime {
    fn now(&self) -> (Instant, SystemTime) {
        (Instant::now(), SystemTime::now())
    }
    fn park(&mut self, duration: Duration) {
        thread::park_timeout(duration);
    }
}
fn run_worker(
    mut process: impl WorkerProcess,
    shared: Arc<Shared>,
    deadlines: LocalDeadlines,
    // This is strictly a lifetime token. NEVER lock or dereference it here.
    _store: Arc<Mutex<storage::Store>>,
    reply: std::sync::mpsc::SyncSender<Result<()>>,
    mut runtime: impl WorkerRuntime,
) {
    let (mono, wall) = runtime.now();
    progress(&mut process, &shared, &deadlines, mono, wall);
    let _ = reply.try_send(Ok(()));
    while shared.state.load(Ordering::Acquire) != REAPED {
        // Fixed polling even when stop remains latched or OS errors persist.
        // unpark is a coalescing wake token, never a notification queue.
        runtime.park(POLL);
        let (mono, wall) = runtime.now();
        progress(&mut process, &shared, &deadlines, mono, wall);
    }
    // Only direct-child reap allows dropping the Supervisor and lifetime token.
}
fn progress(
    process: &mut impl WorkerProcess,
    shared: &Shared,
    deadlines: &LocalDeadlines,
    mono: Instant,
    wall: SystemTime,
) {
    if shared.stop.load(Ordering::Acquire) || deadlines.stop_due(mono, wall) {
        process.shutdown();
    }
    let exited = process.child_exited();
    let state = match exited {
        Ok(true) => REAPED,
        Err(_) => UNCERTAIN,
        Ok(false) => match process.local_termination_state() {
            LocalTerminationState::Running => RUNNING,
            LocalTerminationState::StopPending => PENDING,
            LocalTerminationState::DirectChildReaped => REAPED,
            LocalTerminationState::Uncertain => UNCERTAIN,
        },
    };
    // Observation after cleanup boundary is conservatively a miss even if this
    // poll reaps: exact earlier exit/reap time was not observed.
    if deadlines.cleanup_due(mono, wall) {
        shared.cleanup_missed.store(true, Ordering::Release);
    }
    shared.state.store(state, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        sync::{atomic::AtomicU64, mpsc::sync_channel},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "m8s-timed-unit-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
        fn open(&self) -> Local {
            self.open_fixture(|| {}).0
        }
        fn open_fixture(&self, contended: impl FnMut()) -> (Local, usize) {
            let (result, retries) = crate::tests::open_fixture_result(
                &self.0,
                "owner",
                || DevelopmentCommand::new("/bin/sleep".into(), vec!["60".into()]).unwrap(),
                contended,
            );
            (result.unwrap(), retries)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn directory_fixture_waits_for_inherited_lock() {
        let dir = Directory::new();
        let local = dir.open();
        crate::tests::with_inherited_fixture_lock(&dir.0, local, |contended| {
            let (reopened, retries) = dir.open_fixture(contended);
            assert!(retries >= 1);
            drop(reopened);
        });
    }
    #[test]
    fn directory_fixture_does_not_retry_storage_errors() {
        let dir = Directory::new();
        // Synthetic unlocked journal avoids any earlier inheritable lock.
        for (name, bytes) in [
            ("lock", &b""[..]),
            ("state.json", &b"malformed synthetic state"[..]),
        ] {
            fs::write(dir.0.join(name), bytes).unwrap();
            fs::set_permissions(dir.0.join(name), fs::Permissions::from_mode(0o600)).unwrap();
        }
        let mut retries = 0;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dir.open_fixture(|| retries += 1);
        }));
        assert_eq!(retries, 0);
        let panic = result.expect_err("Storage must fail the timed fixture");
        assert!(panic.downcast_ref::<String>().unwrap().contains("Storage"));
    }
    fn intent(local: &mut Local) -> Instance {
        let id = format!("local-{}-1", local.state.namespace);
        let instance = Instance {
            id: id.clone(),
            owner: "owner".into(),
            node_uid: "uid".into(),
            tier: "dev".into(),
            state: InstanceState::Starting,
        };
        local.state.next = 1;
        local.state.records.insert(
            id,
            Record {
                instance: instance.clone(),
                status: Status::Intent,
            },
        );
        local.save().unwrap(); // EVERY test OS spawn also has a durable intent
        instance
    }
    fn spec() -> ChildSpec {
        ChildSpec {
            program: "/bin/sleep".into(),
            args: vec!["60".into()],
        }
    }
    fn deadlines(after: Duration) -> LocalDeadlines {
        let stop = SystemTime::now() + after;
        LocalDeadlines::new(stop, stop + Duration::from_millis(200)).unwrap()
    }
    fn wait_reaped(launch: &LocalLaunch) {
        let end = Instant::now() + Duration::from_secs(4);
        while launch.termination_state() != LocalTerminationState::DirectChildReaped {
            assert!(Instant::now() < end, "worker did not reap direct child");
            thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn blocked_journal_and_store_mutex_do_not_delay_deadline() {
        let dir = Directory::new();
        let mut local = dir.open();
        let instance = intent(&mut local);
        let (process, started) = TimedProcess::start(
            spec(),
            deadlines(Duration::from_millis(300)),
            local.store.clone(),
        )
        .unwrap();
        started
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap();
        let launch = process.launch(instance.clone());
        local.timed_processes.insert(instance.id.clone(), process);
        let store = local.store.clone();
        let (entered, at_save) = sync_channel(1);
        let (release, gate) = sync_channel(1);
        local.save_barrier = Some((0, entered, gate));
        let caller = thread::spawn(move || {
            local.save().unwrap();
            local
        });
        at_save.recv_timeout(Duration::from_secs(3)).unwrap();
        let held = store.lock().unwrap();
        wait_reaped(&launch); // no store mutex, journal hook or caller required
        drop(held);
        release.try_send(()).unwrap();
        let mut local = caller.join().unwrap();
        assert!(local.state.records[&instance.id].status == Status::Intent);
        assert!(matches!(
            local.describe(&instance.id).unwrap(),
            Observation::Present(Instance {
                state: InstanceState::Stopped,
                ..
            })
        ));
        local.destroy(&instance.id).unwrap();
        assert_eq!(local.describe(&instance.id).unwrap(), Observation::Absent);
    }
    #[test]
    fn destroy_signals_before_blocked_journal_save() {
        let dir = Directory::new();
        let mut local = dir.open();
        let launch = local
            .launch_development_until(
                &LaunchSpec {
                    owner: "owner".into(),
                    node_uid: "uid".into(),
                    tier: "dev".into(),
                },
                deadlines(Duration::from_secs(20)),
            )
            .unwrap();
        let id = launch.instance().id.clone();
        let (entered, at_save) = sync_channel(1);
        let (release, gate) = sync_channel(1);
        local.save_barrier = Some((0, entered, gate));
        let caller = thread::spawn(move || {
            local.destroy(&id).unwrap();
            local
        });
        at_save.recv_timeout(Duration::from_secs(3)).unwrap();
        wait_reaped(&launch); // stop cannot wait for Destroying fsync
        release.try_send(()).unwrap();
        let mut local = caller.join().unwrap();
        assert_eq!(
            local.describe(&launch.instance().id).unwrap(),
            Observation::Absent
        );
    }
    #[test]
    fn drop_local_retains_exact_lock_until_late_spawn_is_stopped_and_reaped() {
        let dir = Directory::new();
        let mut local = dir.open();
        let instance = intent(&mut local);
        let (entered, at_spawn) = sync_channel(1);
        let (release, gate) = sync_channel(1);
        let (process, started) = TimedProcess::start_inner(
            spec(),
            deadlines(Duration::from_secs(10)),
            local.store.clone(),
            Hooks {
                after_spawn: Some((entered, gate)),
            },
        )
        .unwrap();
        let launch = process.launch(instance.clone());
        local.timed_processes.insert(instance.id.clone(), process);
        at_spawn.recv_timeout(Duration::from_secs(3)).unwrap();
        drop(local); // requests stop while test deliberately delays spawn return handling
        assert!(matches!(
            Local::open_development(
                &dir.0,
                "owner".into(),
                DevelopmentCommand::new("/bin/sleep".into(), vec!["0".into()]).unwrap()
            ),
            Err(Error::Unavailable)
        ));
        release.try_send(()).unwrap();
        started
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap();
        assert_ne!(launch.termination_state(), LocalTerminationState::Running);
        wait_reaped(&launch);
        // Worker may be between publishing REAPED and releasing its Arc.
        let end = Instant::now() + Duration::from_secs(3);
        loop {
            match Local::open_development(
                &dir.0,
                "owner".into(),
                DevelopmentCommand::new("/bin/sleep".into(), vec!["0".into()]).unwrap(),
            ) {
                Ok(mut reopened) => {
                    assert!(reopened.describe(&instance.id).is_err());
                    break;
                }
                Err(Error::Unavailable) => {
                    assert!(Instant::now() < end);
                    thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("unexpected reopen error {e:?}"),
            }
        }
    }
    #[test]
    fn late_spawn_lost_reply_and_cleanup_miss_still_retain_ownership() {
        let dir = Directory::new();
        let mut local = dir.open();
        let instance = intent(&mut local);
        let (entered, at_spawn) = sync_channel(1);
        let (release, gate) = sync_channel(1);
        let (process, started) = TimedProcess::start_inner(
            spec(),
            deadlines(Duration::from_millis(100)),
            local.store.clone(),
            Hooks {
                after_spawn: Some((entered, gate)),
            },
        )
        .unwrap();
        let launch = process.launch(instance);
        at_spawn.recv_timeout(Duration::from_secs(3)).unwrap();
        drop(started); // reply disconnect must never block/drop the Supervisor
        thread::sleep(Duration::from_millis(350));
        release.try_send(()).unwrap();
        wait_reaped(&launch);
        assert!(launch.cleanup_deadline_missed());
        assert!(process.reaped());
    }
    #[test]
    fn expired_before_spawn_and_spawn_error_leave_uncertain_intents() {
        let dir = Directory::new();
        let mut local = dir.open();
        let instance = intent(&mut local);
        let d = deadlines(Duration::from_millis(1));
        thread::sleep(Duration::from_millis(5));
        let (process, started) = TimedProcess::start(spec(), d, local.store.clone()).unwrap();
        assert!(
            started
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .is_err()
        );
        assert!(process.child_exited().is_err());
        assert!(!process.reaped());
        assert!(local.state.records[&instance.id].status == Status::Intent);
        let (failed, started) = TimedProcess::start(
            ChildSpec {
                program: "/nonexistent/m8s-synthetic".into(),
                args: vec![],
            },
            deadlines(Duration::from_secs(1)),
            local.store.clone(),
        )
        .unwrap();
        assert!(
            started
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .is_err()
        );
        assert!(failed.child_exited().is_err());
        assert!(failed.acknowledge_stop().is_err());
    }
    #[test]
    fn pending_and_uncertain_evidence_never_become_reaped() {
        let process = TimedProcess(LocalStopHandle(Arc::new(Shared {
            stop: AtomicBool::new(false),
            state: AtomicU8::new(UNCERTAIN),
            cleanup_missed: AtomicBool::new(false),
            worker: OnceLock::new(),
        })));
        assert!(process.child_exited().is_err());
        assert!(!process.reaped());
        process.request_stop();
        assert!(process.child_exited().is_err());
        process.0.0.state.store(PENDING, Ordering::Release);
        assert!(!process.child_exited().unwrap());
        assert!(!process.reaped());
    }
    #[derive(Clone, Copy)]
    enum Unresolved {
        NeverReaped,
        ObserveError,
        KillDenied,
    }
    impl Unresolved {
        fn state(self) -> u8 {
            match self {
                Self::NeverReaped => PENDING,
                _ => UNCERTAIN,
            }
        }
    }
    #[derive(Default)]
    struct EngineTrace {
        stops: usize,
        polls: usize,
        parks: Vec<Duration>,
        allow_reap: bool,
        reaped: bool,
        dropped: bool,
    }
    type Trace = std::rc::Rc<std::cell::RefCell<EngineTrace>>;
    struct ScriptedProcess {
        unresolved: Unresolved,
        trace: Trace,
    }
    impl WorkerProcess for ScriptedProcess {
        fn shutdown(&mut self) {
            let mut trace = self.trace.borrow_mut();
            assert!(!trace.reaped, "no later stop action after identity release");
            trace.stops += 1;
        }
        fn child_exited(&mut self) -> std::io::Result<bool> {
            let mut trace = self.trace.borrow_mut();
            assert!(!trace.reaped, "no later poll/action after identity release");
            trace.polls += 1;
            if trace.allow_reap {
                trace.reaped = true;
                return Ok(true);
            }
            match self.unresolved {
                Unresolved::NeverReaped => Ok(false),
                // Script only the Supervisor-facing outcomes. No duplicate OS
                // signalling, PID identity or reaping algorithm lives here.
                Unresolved::ObserveError => Err(std::io::ErrorKind::Other.into()),
                Unresolved::KillDenied => Err(std::io::ErrorKind::PermissionDenied.into()),
            }
        }
        fn local_termination_state(&self) -> LocalTerminationState {
            assert!(!self.trace.borrow().reaped);
            LocalTerminationState::StopPending
        }
    }
    impl Drop for ScriptedProcess {
        fn drop(&mut self) {
            let mut trace = self.trace.borrow_mut();
            assert!(trace.reaped, "unresolved worker must not abandon ownership");
            assert!(!trace.dropped);
            trace.dropped = true;
        }
    }
    struct ScriptedRuntime {
        mono: Instant,
        wall: SystemTime,
        trace: Trace,
        shared: Arc<Shared>,
        store: std::sync::Weak<Mutex<storage::Store>>,
        directory: PathBuf,
        unresolved: Unresolved,
    }
    impl WorkerRuntime for ScriptedRuntime {
        fn now(&self) -> (Instant, SystemTime) {
            (self.mono, self.wall)
        }
        fn park(&mut self, duration: Duration) {
            assert_eq!(
                duration, POLL,
                "persistent errors/latched stop cannot busy-spin"
            );
            assert_eq!(
                self.shared.state.load(Ordering::Acquire),
                self.unresolved.state()
            );
            assert_eq!(
                self.store.strong_count(),
                1,
                "worker alone must retain exact Store"
            );
            assert!(
                matches!(
                    Local::open_development(
                        &self.directory,
                        "owner".into(),
                        DevelopmentCommand::new("/bin/sleep".into(), vec!["0".into()]).unwrap()
                    ),
                    Err(Error::Unavailable)
                ),
                "journal lock released while process unresolved"
            );
            let mut trace = self.trace.borrow_mut();
            assert!(!trace.reaped && !trace.dropped);
            assert_eq!(
                trace.polls,
                trace.parks.len() + 1,
                "exactly one bounded progress step per poll"
            );
            trace.parks.push(duration);
            // Eight persistently unresolved intervals, including cleanup miss.
            // Only explicit later direct-reaped evidence permits engine exit.
            if trace.parks.len() == 8 {
                trace.allow_reap = true;
            }
            assert!(trace.parks.len() <= 8, "worker failed to exit after reap");
            self.mono += duration;
            self.wall += duration;
        }
    }
    fn scripted_worker_retains_until_explicit_reap(
        unresolved: Unresolved,
        stop_requested: bool,
        full_reply: bool,
    ) {
        let dir = Directory::new();
        let mut local = dir.open();
        let instance = intent(&mut local);
        let store = local.store.clone();
        let weak = Arc::downgrade(&store);
        drop(local); // engine, not caller or fake process, must own lock lifetime
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(stop_requested),
            state: AtomicU8::new(STARTING),
            cleanup_missed: AtomicBool::new(false),
            worker: OnceLock::new(),
        });
        let trace = Trace::default();
        let mono = Instant::now();
        let wall = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let deadlines = LocalDeadlines::anchored(wall + POLL, wall + POLL * 3, mono, wall).unwrap();
        let runtime = ScriptedRuntime {
            mono,
            wall,
            trace: trace.clone(),
            shared: shared.clone(),
            store: weak.clone(),
            directory: dir.0.clone(),
            unresolved,
        };
        let (reply, receiver) = sync_channel(1);
        if full_reply {
            reply.try_send(Ok(())).unwrap();
        }
        run_worker(
            ScriptedProcess {
                unresolved,
                trace: trace.clone(),
            },
            shared.clone(),
            deadlines,
            store,
            reply,
            runtime,
        );
        assert!(
            receiver
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .is_ok()
        );
        assert_eq!(shared.state.load(Ordering::Acquire), REAPED);
        assert!(shared.cleanup_missed.load(Ordering::Acquire));
        let trace = trace.borrow();
        assert_eq!(trace.parks, vec![POLL; 8]);
        assert_eq!(trace.polls, 9);
        assert_eq!(trace.stops, if stop_requested { 9 } else { 8 });
        assert!(trace.reaped && trace.dropped);
        assert!(
            weak.upgrade().is_none(),
            "lock token not released after confirmed reap"
        );
        let mut reopened = dir.open();
        assert!(
            reopened.describe(&instance.id).is_err(),
            "scripted reap is NOT durable provider absence"
        );
    }
    #[test]
    fn worker_persistent_uncertainty_retains_lock_and_polls_until_reap() {
        scripted_worker_retains_until_explicit_reap(Unresolved::ObserveError, true, false);
    }
    #[test]
    fn worker_never_reaped_state_does_not_exit_or_busy_spin_after_stop() {
        scripted_worker_retains_until_explicit_reap(Unresolved::NeverReaped, true, false);
    }
    #[test]
    fn worker_kill_error_and_full_reply_cannot_abandon_or_delay_deadline() {
        scripted_worker_retains_until_explicit_reap(Unresolved::KillDenied, false, true);
    }
    #[test]
    fn anchored_boundaries_fractional_and_no_rebase() {
        let mono = Instant::now();
        let wall = SystemTime::UNIX_EPOCH + Duration::from_millis(1234);
        let stop = wall + Duration::from_nanos(777);
        let end = stop + Duration::from_secs(1);
        let d = LocalDeadlines::anchored(stop, end, mono, wall).unwrap();
        assert!(!d.stop_due(mono + Duration::from_nanos(776), wall));
        assert!(d.stop_due(mono + Duration::from_nanos(777), wall));
        assert!(d.stop_due(mono, stop)); // forward wall change
        assert!(d.stop_due(d.stop, SystemTime::UNIX_EPOCH)); // rollback cannot renew
        assert!(d.cleanup_due(d.cleanup, wall));
        assert!(!d.cleanup_due(d.stop, wall));
    }
    #[test]
    fn invalid_and_overflow_clock_conversion() {
        let mono = Instant::now();
        let wall = SystemTime::UNIX_EPOCH;
        assert!(LocalDeadlines::anchored(wall, wall, mono, wall).is_err());
        let stop = wall + Duration::from_secs(2);
        assert!(LocalDeadlines::anchored(stop, wall, mono, wall).is_err());
        assert!(LocalDeadlines::anchored(wall, stop, mono, stop).is_err());
        // Find a representable SystemTime whose Instant conversion overflows.
        if let Some(far) = wall.checked_add(Duration::from_secs(i64::MAX as u64)) {
            assert!(LocalDeadlines::anchored(far, far, mono, wall).is_err());
        }
    }
}
