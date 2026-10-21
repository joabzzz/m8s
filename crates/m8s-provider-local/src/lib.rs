//! Explicit opt-in synthetic development processes, not secure cloud capacity.
//! No credentials, network client, model download, admission or readiness evidence.
//! See the crate README for the trusted-executable and exclusive-reaper contract.
#![cfg(unix)]
mod storage;
mod timed;
pub use m8s_agent::supervisor::LocalTerminationState;
use m8s_agent::{
    DeadManSwitch, Effect, Outcome,
    supervisor::{ChildSpec, LocalOnlyBoot, RetryTeardown, Supervisor, TeardownDriver},
};
use m8s_core::{Instance, InstanceState, Observation, Offer};
use m8s_provider::{Error, LaunchSpec, Provider, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
pub use timed::{LocalDeadlines, LocalLaunch, LocalStopHandle};

/// Constructor-pinned, nonsecret configuration; never supplied by LaunchSpec.
/// This is NOT an executable sandbox. Use a trusted local synthetic executable
/// that neither daemonizes nor escapes its dedicated process group. No secrets
/// belong in arguments. No Debug/Serialize implementation is provided.
pub struct DevelopmentCommand {
    program: PathBuf,
    args: Vec<OsString>,
}
impl DevelopmentCommand {
    pub fn new(program: PathBuf, args: Vec<OsString>) -> Result<Self> {
        use std::os::unix::ffi::OsStrExt;
        let total = args
            .iter()
            .try_fold(program.as_os_str().as_bytes().len(), |n, a| {
                n.checked_add(a.as_bytes().len())
            });
        if !program.is_absolute()
            || args.len() > 256
            || total.is_none_or(|n| n > 64 * 1024)
            || program.as_os_str().as_bytes().contains(&0)
            || args.iter().any(|a| a.as_bytes().contains(&0))
        {
            return Err(Error::Invalid);
        }
        Ok(Self { program, args })
    }
    /// Versioned raw Unix configuration digest, NOT executable-content identity.
    pub fn fingerprint(&self) -> [u8; 32] {
        use std::os::unix::ffi::OsStrExt;
        let mut hash = Sha256::new();
        hash.update(b"m8s-local-command-v1\0env-empty\0stdio-null\0group-new\0");
        let mut field = |bytes: &[u8]| {
            hash.update((bytes.len() as u64).to_be_bytes());
            hash.update(bytes);
        };
        field(self.program.as_os_str().as_bytes());
        field(&(self.args.len() as u64).to_be_bytes());
        for arg in &self.args {
            field(arg.as_bytes());
        }
        hash.finalize().into()
    }
}

// The local process adapter has no external teardown or security claims. In
// particular this driver can NEVER confirm provider absence; Local observes its
// own retained child after an explicit destroy, independently below.
struct NoExternalEffects;
impl TeardownDriver for NoExternalEffects {
    fn perform(&mut self, _: Effect) -> Outcome {
        Outcome::Failure
    }
}
type Process = Supervisor<NoExternalEffects>;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Status {
    Intent,
    Active,
    Stopped,
    Destroying,
    Absent,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    instance: Instance,
    status: Status,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u32,
    owner: String,
    namespace: String,
    next: u64,
    #[serde(deserialize_with = "unique_records")]
    records: BTreeMap<String, Record>,
}

const MAX_RECORDS: usize = 256;
const MAX_LABEL: usize = 256;
fn unique_records<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<BTreeMap<String, Record>, D::Error> {
    struct Records;
    impl<'de> serde::de::Visitor<'de> for Records {
        type Value = BTreeMap<String, Record>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded records with unique IDs")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> std::result::Result<Self::Value, M::Error> {
            let mut records = BTreeMap::new();
            while let Some(id) = map.next_key::<String>()? {
                if records.len() >= MAX_RECORDS || records.contains_key(&id) {
                    return Err(serde::de::Error::custom("record limit or duplicate ID"));
                }
                records.insert(id, map.next_value::<Record>()?);
            }
            Ok(records)
        }
    }
    d.deserialize_map(Records)
}

/// One owner and one exclusive private durable directory per adapter. Never
/// reset/reuse the directory or run another reaper for its child processes.
/// Reopening retains intents but deliberately cannot adopt numeric PIDs.
/// Drop attempts local group cleanup, but does not record destroy or absence.
pub struct Local {
    command: DevelopmentCommand,
    state: State,
    processes: BTreeMap<String, Process>,
    healthy: bool,
    timed_processes: BTreeMap<String, timed::TimedProcess>,
    #[cfg(test)]
    save_barrier: Option<(
        usize,
        std::sync::mpsc::SyncSender<()>,
        std::sync::mpsc::Receiver<()>,
    )>,
    // Fields drop in declaration order: keep the lock until all supervisors
    // have attempted cleanup and released their exclusive child handles.
    store: Arc<Mutex<storage::Store>>,
}
impl Local {
    /// Unix-only, explicit development opt-in. The caller creates a private
    /// directory, exclusively owned by this OS user, outside secret stores.
    /// The executable and argv are trusted nonsecret local configuration.
    pub fn open_development(
        directory: &Path,
        owner: String,
        command: DevelopmentCommand,
    ) -> Result<Self> {
        if owner.is_empty() || owner.len() > MAX_LABEL {
            return Err(Error::Invalid);
        }
        let (store, bytes) = storage::Store::open(directory)?;
        let namespace = format!("{:x}", Sha256::digest(store.namespace_bytes()));
        let state: State = match bytes {
            Some(bytes) => serde_json::from_slice(&bytes).map_err(|_| Error::Storage)?,
            None => State {
                version: 1,
                owner: owner.clone(),
                namespace,
                next: 0,
                records: BTreeMap::new(),
            },
        };
        if state.owner != owner {
            return Err(Error::Invalid);
        }
        let mut uids = BTreeSet::new();
        if state.version != 1
            || state.namespace.len() != 64
            || !state.namespace.bytes().all(|c| c.is_ascii_hexdigit())
            || state.records.iter().any(|(id, r)| {
                let prefix = format!("local-{}-", state.namespace);
                let sequence = id.strip_prefix(&prefix).and_then(|v| v.parse::<u64>().ok());
                sequence.is_none_or(|n| n == 0 || n > state.next || *id != format!("{prefix}{n}"))
                    || id != &r.instance.id
                    || r.instance.owner != owner
                    || r.instance.tier.is_empty()
                    || r.instance.tier.len() > MAX_LABEL
                    || r.instance.node_uid.is_empty()
                    || r.instance.node_uid.len() > MAX_LABEL
                    || !uids.insert(r.instance.node_uid.clone())
            })
        {
            return Err(Error::Storage);
        }
        let mut local = Self {
            store: Arc::new(Mutex::new(store)),
            command,
            state,
            processes: BTreeMap::new(),
            healthy: true,
            timed_processes: BTreeMap::new(),
            #[cfg(test)]
            save_barrier: None,
        };
        local.save()?;
        Ok(local)
    }
    fn check(&self) -> Result<()> {
        if self.healthy {
            self.store.lock().map_err(|_| Error::Storage)?.validate()
        } else {
            Err(Error::Unavailable)
        }
    }
    // Fail closed after ANY write failure, even if rename succeeded but fsync
    // failed. The in-memory state must never authorize a later launch retry.
    fn save(&mut self) -> Result<()> {
        #[cfg(test)]
        if let Some((skip, entered, release)) = self.save_barrier.take() {
            if skip > 0 {
                self.save_barrier = Some((skip - 1, entered, release));
            } else {
                let _ = entered.try_send(());
                release
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .map_err(|_| Error::Storage)?;
            }
        }
        let result = serde_json::to_vec(&self.state)
            .map_err(|_| Error::Storage)
            .and_then(|bytes| self.store.lock().map_err(|_| Error::Storage)?.save(&bytes));
        if result.is_err() {
            self.healthy = false;
        }
        result
    }
    fn observe(&mut self, id: &str) -> Result<Observation> {
        self.check()?;
        let record = self.state.records.get(id).ok_or(Error::Unavailable)?;
        if record.status == Status::Absent {
            return Ok(Observation::Absent);
        }
        // Missing handles, spawn failures and restarts are uncertainty. No PID
        // is persisted, probed, adopted or signalled from this path.
        let exited = if let Some(process) = self.timed_processes.get(id) {
            process.child_exited()?
        } else {
            self.processes
                .get_mut(id)
                .ok_or(Error::Unavailable)?
                .child_exited()
                .map_err(|_| Error::Unavailable)?
        };
        if exited {
            let record = self.state.records.get_mut(id).ok_or(Error::Unavailable)?;
            if record.status == Status::Destroying {
                record.status = Status::Absent;
                self.save()?;
                return Ok(Observation::Absent);
            }
            record.status = Status::Stopped;
            record.instance.state = InstanceState::Stopped;
            self.save()?;
        }
        Ok(Observation::Present(
            self.state.records[id].instance.clone(),
        ))
    }
}

impl Local {
    /// Parent-alive synthetic scheduling only; not Store claim/admission authority.
    pub fn launch_development_until(
        &mut self,
        spec: &LaunchSpec,
        deadlines: LocalDeadlines,
    ) -> Result<LocalLaunch> {
        // Never turn an existing untimed allocation or a retry into fresh ownership.
        if self
            .state
            .records
            .values()
            .any(|r| r.instance.node_uid == spec.node_uid)
        {
            return Err(Error::Invalid);
        }
        let instance = self.launch_inner(&development_offer(), spec, Some(deadlines))?;
        let process = self
            .timed_processes
            .get(&instance.id)
            .ok_or(Error::Unavailable)?;
        Ok(process.launch(instance))
    }
    fn launch_inner(
        &mut self,
        offer: &Offer,
        spec: &LaunchSpec,
        deadlines: Option<LocalDeadlines>,
    ) -> Result<Instance> {
        self.check()?;
        if *offer != development_offer()
            || spec.owner != self.state.owner
            || spec.node_uid.is_empty()
            || spec.tier.is_empty()
            || spec.node_uid.len() > MAX_LABEL
            || spec.tier.len() > MAX_LABEL
        {
            return Err(Error::Invalid);
        }
        if let Some(record) = self
            .state
            .records
            .values()
            .find(|r| r.instance.node_uid == spec.node_uid)
        {
            if record.instance.tier != spec.tier {
                return Err(Error::Invalid);
            }
            let id = record.instance.id.clone();
            return match self.observe(&id)? {
                Observation::Present(i) => Ok(i),
                Observation::Absent => Err(Error::Invalid), // tombstone forbids resurrection
            };
        }
        // Unresolved orphan uncertainty blocks ALL fresh effects, not merely
        // retries of its own UID. Owned handles may still be read/destroyed.
        if self.state.records.iter().any(|(id, r)| {
            r.status != Status::Absent
                && !self.processes.contains_key(id)
                && !self.timed_processes.contains_key(id)
        }) {
            return Err(Error::Unavailable);
        }
        if self.timed_processes.values().any(|p| !p.reaped())
            || (deadlines.is_some() && !self.processes.is_empty())
        {
            return Err(Error::Unavailable);
        }
        if self.state.records.len() >= MAX_RECORDS {
            return Err(Error::Invalid);
        }
        self.state.next = self.state.next.checked_add(1).ok_or(Error::Storage)?;
        let id = format!("local-{}-{}", self.state.namespace, self.state.next);
        let instance = Instance {
            id: id.clone(),
            owner: spec.owner.clone(),
            node_uid: spec.node_uid.clone(),
            tier: spec.tier.clone(),
            state: InstanceState::Starting,
        };
        self.state.records.insert(
            id.clone(),
            Record {
                instance,
                status: Status::Intent,
            },
        );
        self.save()?; // durable intent MUST precede the OS effect
        if let Some(deadlines) = deadlines {
            let (process, started) = timed::TimedProcess::start(
                ChildSpec {
                    program: self.command.program.clone(),
                    args: self.command.args.clone(),
                },
                deadlines,
                Arc::clone(&self.store),
            )?;
            self.timed_processes.insert(id.clone(), process);
            started.recv().map_err(|_| Error::Unavailable)??;
        } else {
            let dms = DeadManSwitch::new("synthetic-local-supervision".into(), vec![], 0, 1, 1)
                .map_err(|_| Error::Invalid)?;
            let process = Process::start(
                ChildSpec {
                    program: self.command.program.clone(),
                    args: self.command.args.clone(),
                },
                &mut LocalOnlyBoot,
                dms,
                NoExternalEffects,
                RetryTeardown::new(1).map_err(|_| Error::Invalid)?,
            )
            .map_err(|_| Error::Unavailable)?;
            self.processes.insert(id.clone(), process);
        }
        let record = self.state.records.get_mut(&id).ok_or(Error::Unavailable)?;
        record.status = Status::Active;
        record.instance.state = InstanceState::Running; // OS process only, NEVER Ready
        let instance = record.instance.clone();
        if let Err(error) = self.save() {
            if let Some(process) = self.timed_processes.get(&id) {
                process.request_stop();
            }
            return Err(error);
        }
        Ok(instance)
    }
}

impl Drop for Local {
    fn drop(&mut self) {
        for process in self.timed_processes.values() {
            process.request_stop();
        }
    }
}

/// Zero means unmeasured/unsupported, NOT a measured reliability or bandwidth.
/// Normal GPU/location policy should reject this CPU-only development offer.
fn development_offer() -> Offer {
    Offer {
        id: "local-cpu-unmeasured".into(),
        country: None,
        region: None,
        inet_down_mbps: 0,
        gpu_count: 0,
        vram_bytes_per_gpu: 0,
        hourly_micro_usd: 0,
        interruptible: false,
        reliability_bps: 0,
    }
}
impl Provider for Local {
    fn id(&self) -> &'static str {
        "local-development"
    }
    fn search(&mut self) -> Result<Vec<Offer>> {
        self.check()?;
        Ok(vec![development_offer()])
    }
    fn launch(&mut self, offer: &Offer, spec: &LaunchSpec) -> Result<Instance> {
        self.launch_inner(offer, spec, None)
    }
    fn describe(&mut self, id: &str) -> Result<Observation> {
        self.observe(id)
    }
    fn list_owned(&mut self, owner: &str) -> Result<Vec<Instance>> {
        self.check()?;
        if owner != self.state.owner {
            return Ok(vec![]);
        }
        let ids: Vec<_> = self.state.records.keys().cloned().collect();
        let mut instances = vec![];
        for id in ids {
            if let Observation::Present(instance) = self.observe(&id)? {
                instances.push(instance);
            }
        }
        Ok(instances)
    }
    fn destroy(&mut self, id: &str) -> Result<()> {
        if let Some(process) = self.timed_processes.get(id) {
            process.request_stop(); // no filesystem or caller lock before watchdog notification
        }
        self.check()?;
        let record = self.state.records.get_mut(id).ok_or(Error::Unavailable)?;
        if record.status == Status::Absent {
            return Ok(());
        }
        if !self.processes.contains_key(id) && !self.timed_processes.contains_key(id) {
            return Err(Error::Unavailable);
        }
        record.status = Status::Destroying;
        self.save()?; // untimed path persists before effect; timed stop was already requested
        if let Some(process) = self.timed_processes.get(id) {
            return process.acknowledge_stop();
        }
        let process = self.processes.get_mut(id).ok_or(Error::Unavailable)?;
        process.shutdown();
        process.tick(0).map_err(|_| Error::Unavailable)?;
        // Provider tick deliberately does not propagate local cleanup failures:
        // remote containment must keep progressing. This local-only adapter
        // must inspect that separate evidence before acknowledging its effect.
        if matches!(
            process.local_termination_state(),
            LocalTerminationState::Uncertain | LocalTerminationState::Running
        ) {
            return Err(Error::Unavailable);
        }
        // Acknowledgement is not absence; only subsequent observe can persist it.
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::{
        os::unix::fs::PermissionsExt,
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, Instant},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "m8s-local-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
        fn open(&self, args: &[&str]) -> Local {
            self.open_fixture(args, || {}).0
        }
        // Only this test fixture may wait for transient inherited-fd contention.
        // Production remains nonblocking; all non-lock errors fail immediately.
        fn open_fixture(&self, args: &[&str], mut contended: impl FnMut()) -> (Local, usize) {
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut retries = 0;
            loop {
                match Local::open_development(&self.0, "owner-a".into(), command(args)) {
                    Ok(local) => return (local, retries),
                    Err(Error::Unavailable) => {
                        assert!(
                            Instant::now() < deadline,
                            "fixture lock still unavailable after {retries} retries"
                        );
                        retries += 1;
                        contended();
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("unexpected fixture open error: {error:?}"),
                }
            }
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn command(args: &[&str]) -> DevelopmentCommand {
        DevelopmentCommand::new("/bin/sh".into(), args.iter().map(OsString::from).collect())
            .unwrap()
    }
    fn spec() -> LaunchSpec {
        LaunchSpec {
            owner: "owner-a".into(),
            node_uid: "uid".into(),
            tier: "dev".into(),
        }
    }
    fn launch(p: &mut Local) -> Instance {
        let o = p.search().unwrap().remove(0);
        p.launch(&o, &spec()).unwrap()
    }
    fn until(p: &mut Local, id: &str, absent: bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let observed = p.describe(id).unwrap();
            if (absent && observed == Observation::Absent)
                || (!absent
                    && matches!(
                        observed,
                        Observation::Present(Instance {
                            state: InstanceState::Stopped,
                            ..
                        })
                    ))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "local process observation timed out"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn honest_offer_and_exact_owner_idempotency() {
        let directory = Directory::new();
        let mut p = directory.open(&["-c", "exec /bin/sleep 60"]);
        let o = p.search().unwrap().remove(0);
        assert_eq!(o.gpu_count, 0);
        assert_eq!(o.inet_down_mbps, 0);
        assert_eq!(o.reliability_bps, 0);
        assert!(o.country.is_none() && o.region.is_none());
        let i = p.launch(&o, &spec()).unwrap();
        assert_eq!(i.id, p.launch(&o, &spec()).unwrap().id);
        assert_eq!(p.processes.len(), 1);
        assert_eq!(p.list_owned("owner-a").unwrap().len(), 1);
        assert!(p.list_owned("owner").unwrap().is_empty());
        let mut foreign = spec();
        foreign.owner = "owner-b".into();
        assert!(matches!(p.launch(&o, &foreign), Err(Error::Invalid)));
        let mut changed = spec();
        changed.tier = "other".into();
        assert!(matches!(p.launch(&o, &changed), Err(Error::Invalid)));
        let mut fake = o.clone();
        fake.gpu_count = 1;
        assert!(matches!(p.launch(&fake, &spec()), Err(Error::Invalid)));
        assert!(p.destroy("foreign-numeric-123").is_err());
        assert!(p.describe("unknown").is_err());
        p.destroy(&i.id).unwrap();
        assert!(p.state.records[&i.id].status == Status::Destroying);
        until(&mut p, &i.id, true);
        assert!(p.list_owned("owner-a").unwrap().is_empty());
        assert!(p.launch(&o, &spec()).is_err());
        p.destroy(&i.id).unwrap();
    }
    #[test]
    fn natural_exit_is_stopped_not_destroyed() {
        let directory = Directory::new();
        let mut p = directory.open(&["-c", "/bin/sleep 60 & exit 0"]);
        let i = launch(&mut p);
        until(&mut p, &i.id, false);
        assert_eq!(p.list_owned("owner-a").unwrap().len(), 1);
        p.destroy(&i.id).unwrap();
        until(&mut p, &i.id, true);
    }
    #[test]
    fn restart_does_not_adopt_pids_or_infer_absence_from_lost_handles() {
        let directory = Directory::new();
        let mut p = directory.open(&["-c", "exec /bin/sleep 60"]);
        let i = launch(&mut p);
        assert!(Local::open_development(&directory.0, "owner-a".into(), command(&[])).is_err());
        drop(p); // supervisor best-effort cleanup is NOT durable absence evidence
        let mut reopened = directory.open(&["-c", "exit 0"]);
        assert!(reopened.describe(&i.id).is_err());
        assert!(reopened.destroy(&i.id).is_err());
        assert!(reopened.list_owned("owner-a").is_err());
        let o = reopened.search().unwrap().remove(0);
        assert!(reopened.launch(&o, &spec()).is_err());
        assert!(reopened.processes.is_empty());
        assert_eq!(reopened.state.records.len(), 1);
        drop(reopened);
        assert!(matches!(
            Local::open_development(&directory.0, "owner-b".into(), command(&[])),
            Err(Error::Invalid)
        ));
        let bytes = fs::read(directory.0.join("state.json")).unwrap();
        assert!(!String::from_utf8(bytes).unwrap().contains("pid"));
    }
    #[test]
    fn confirmed_absence_tombstone_survives_restart() {
        let directory = Directory::new();
        let mut p = directory.open(&["-c", "exec /bin/sleep 60"]);
        let i = launch(&mut p);
        p.destroy(&i.id).unwrap();
        until(&mut p, &i.id, true);
        drop(p);
        let mut p = directory.open(&["-c", "exec /bin/sleep 60"]);
        assert_eq!(p.describe(&i.id).unwrap(), Observation::Absent);
        assert!(p.list_owned("owner-a").unwrap().is_empty());
        assert!(p.launch(&development_offer(), &spec()).is_err());
        let mut fresh = spec();
        fresh.node_uid = "new-after-tombstone".into();
        let new = p.launch(&development_offer(), &fresh).unwrap();
        p.destroy(&new.id).unwrap();
        until(&mut p, &new.id, true);
    }
    #[test]
    fn spawn_error_retains_intent_without_retrying() {
        let directory = Directory::new();
        let command =
            DevelopmentCommand::new(directory.0.join("not-an-executable"), vec![]).unwrap();
        let mut p = Local::open_development(&directory.0, "owner-a".into(), command).unwrap();
        assert!(p.launch(&development_offer(), &spec()).is_err());
        assert!(p.state.records.values().all(|r| r.status == Status::Intent));
        assert!(p.launch(&development_offer(), &spec()).is_err());
        assert_eq!(p.state.records.len(), 1);
        assert!(p.list_owned("owner-a").is_err());
    }
    #[test]
    fn durable_intent_write_failure_prevents_any_spawn() {
        let directory = Directory::new();
        let mut p = directory.open(&["-c", "exit 0"]);
        fs::write(
            directory.0.join("state.pending"),
            b"synthetic interrupted write",
        )
        .unwrap();
        assert!(matches!(
            p.launch(&development_offer(), &spec()),
            Err(Error::Storage)
        ));
        assert!(p.processes.is_empty());
        assert!(!p.healthy);
        assert!(p.launch(&development_offer(), &spec()).is_err());
        assert!(p.describe("unknown").is_err());
    }
    #[test]
    fn malformed_store_and_relative_command_fail_closed() {
        let directory = Directory::new();
        assert!(DevelopmentCommand::new("sh".into(), vec![]).is_err());
        fs::write(directory.0.join("state.json"), b"not json").unwrap();
        assert!(matches!(
            Local::open_development(&directory.0, "owner-a".into(), command(&[])),
            Err(Error::Storage)
        ));
    }
    #[test]
    fn environment_is_cleared_without_reading_ambient_secrets() {
        let directory = Directory::new();
        // env lists names only, the shell filters its own PWD bookkeeping; no
        // test reads/sets ambient credentials, owner keys, or OS secret stores.
        let output = directory.0.join("environment-names");
        let script = format!(
            "/usr/bin/env | /usr/bin/cut -d= -f1 | /usr/bin/grep -Ev '^(PWD|SHLVL|_)$' > '{}'; /bin/sleep 60 & exit 0",
            output.display()
        );
        let mut p = directory.open(&["-c", &script]);
        let i = launch(&mut p);
        until(&mut p, &i.id, false);
        assert_eq!(fs::read(output).unwrap(), b"");
    }
    #[test]
    fn owned_group_descendant_cannot_continue_after_destroy() {
        let directory = Directory::new();
        let ready = directory.0.join("ready");
        let survived = directory.0.join("survived");
        let script = format!(
            "(/bin/sleep 1; /usr/bin/touch '{}') & /usr/bin/touch '{}'; wait",
            survived.display(),
            ready.display()
        );
        let mut p = directory.open(&["-c", &script]);
        let i = launch(&mut p);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        p.destroy(&i.id).unwrap();
        until(&mut p, &i.id, true);
        std::thread::sleep(Duration::from_millis(1200));
        assert!(!survived.exists());
    }
    #[test]
    fn external_reaping_is_os_uncertainty_never_absence_or_numeric_pid_cleanup() {
        let directory = Directory::new();
        let mut p = directory.open(&["-c", "exit 0"]);
        let i = launch(&mut p);
        let pid = p.processes[&i.id].child_id();
        let mut status = 0;
        // Deliberately violate exclusive reaping to exercise ECHILD. This only
        // selects our retained synthetic child's PID, never a host process scan.
        assert_eq!(
            unsafe { libc::waitpid(pid as i32, &mut status, 0) },
            pid as i32
        );
        assert!(matches!(p.describe(&i.id), Err(Error::Unavailable)));
        assert!(matches!(p.destroy(&i.id), Err(Error::Unavailable)));
        assert!(matches!(p.list_owned("owner-a"), Err(Error::Unavailable)));
        assert!(p.state.records[&i.id].status == Status::Destroying);
    }
    #[test]
    fn natural_empty_group_exit_never_turns_os_denial_into_absence() {
        let directory = Directory::new();
        let mut p = directory.open(&["-c", "exit 0"]);
        let i = launch(&mut p);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match p.describe(&i.id) {
                Ok(Observation::Present(Instance {
                    state: InstanceState::Stopped,
                    ..
                })) => break,
                // macOS sandbox can deny signalling an already-empty group.
                // This is an explicitly supported uncertain result, not a skip
                // or a claim that the empty-group lifecycle was qualified.
                Err(Error::Unavailable) => {
                    assert!(p.state.records[&i.id].status != Status::Absent);
                    assert!(p.destroy(&i.id).is_err());
                    break;
                }
                Ok(Observation::Present(_)) => {}
                _ => panic!("unexpected local observation"),
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn missing_existing_state_cannot_reset_process_identity() {
        let directory = Directory::new();
        let p = directory.open(&["-c", "exit 0"]);
        drop(p);
        fs::remove_file(directory.0.join("state.json")).unwrap();
        assert!(matches!(
            Local::open_development(&directory.0, "owner-a".into(), command(&[])),
            Err(Error::Storage)
        ));
    }
    #[test]
    fn symlink_store_is_not_followed() {
        use std::os::unix::fs::symlink;
        let directory = Directory::new();
        let target = directory.0.join("synthetic-target");
        fs::write(&target, b"synthetic").unwrap();
        symlink(&target, directory.0.join("lock")).unwrap();
        assert!(matches!(
            Local::open_development(&directory.0, "owner-a".into(), command(&[])),
            Err(Error::Storage)
        ));
        assert_eq!(fs::read(target).unwrap(), b"synthetic");
    }
    #[test]
    fn every_unresolved_restart_status_blocks_different_uid() {
        for status in [
            Status::Intent,
            Status::Active,
            Status::Stopped,
            Status::Destroying,
        ] {
            let directory = Directory::new();
            let mut p = directory.open(&["-c", "exit 0"]);
            // Synthetic durable records exercise all restart phases without
            // creating or adopting a numeric process identity.
            let id = format!("local-{}-1", p.state.namespace);
            p.state.next = 1;
            p.state.records.insert(
                id.clone(),
                Record {
                    instance: Instance {
                        id,
                        owner: "owner-a".into(),
                        node_uid: "old-uid".into(),
                        tier: "dev".into(),
                        state: InstanceState::Starting,
                    },
                    status,
                },
            );
            p.save().unwrap();
            drop(p);
            let mut p = directory.open(&["-c", "exec /bin/sleep 60"]);
            let mut fresh = spec();
            fresh.node_uid = "new-uid".into();
            assert!(matches!(
                p.launch(&development_offer(), &fresh),
                Err(Error::Unavailable)
            ));
            assert!(p.processes.is_empty());
            assert_eq!(p.state.next, 1);
            assert_eq!(p.state.records.len(), 1);
        }
    }
    #[test]
    fn unresolved_spawn_intent_blocks_new_uid_but_owned_handle_can_be_destroyed() {
        let directory = Directory::new();
        let mut p = directory.open(&["-c", "exec /bin/sleep 60"]);
        let i = launch(&mut p);
        p.command = DevelopmentCommand::new(directory.0.join("missing"), vec![]).unwrap();
        let mut failed = spec();
        failed.node_uid = "failed".into();
        assert!(p.launch(&development_offer(), &failed).is_err());
        let mut fresh = spec();
        fresh.node_uid = "different".into();
        assert!(matches!(
            p.launch(&development_offer(), &fresh),
            Err(Error::Unavailable)
        ));
        assert!(matches!(p.describe(&i.id), Ok(Observation::Present(_))));
        p.destroy(&i.id).unwrap();
        until(&mut p, &i.id, true);
    }
    fn write_private(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[test]
    fn storage_modes_hardlinks_and_symlinks_fail_closed() {
        use std::os::unix::fs::symlink;
        for name in ["lock", "state.json"] {
            for fault in ["mode", "hardlink", "symlink", "fifo"] {
                let directory = Directory::new();
                drop(directory.open(&["-c", "exit 0"]));
                let path = directory.0.join(name);
                match fault {
                    "mode" => {
                        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap()
                    }
                    "hardlink" => fs::hard_link(&path, directory.0.join("second-link")).unwrap(),
                    "symlink" => {
                        fs::rename(&path, directory.0.join("target")).unwrap();
                        symlink(directory.0.join("target"), &path).unwrap();
                    }
                    _ => {
                        use std::os::unix::ffi::OsStrExt;
                        fs::remove_file(&path).unwrap();
                        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
                        // SAFETY: valid C path under our private synthetic fixture.
                        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
                    }
                }
                assert!(matches!(
                    Local::open_development(&directory.0, "owner-a".into(), command(&[])),
                    Err(Error::Storage)
                ));
            }
        }
        let directory = Directory::new();
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o750)).unwrap();
        assert!(matches!(
            Local::open_development(&directory.0, "owner-a".into(), command(&[])),
            Err(Error::Storage)
        ));
    }
    #[test]
    fn duplicate_json_record_ids_and_limits_are_rejected() {
        let directory = Directory::new();
        drop(directory.open(&["-c", "exit 0"]));
        let path = directory.0.join("state.json");
        let mut state: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        state["next"] = serde_json::json!(1);
        let id = format!("local-{}-1", state["namespace"].as_str().unwrap());
        let record = serde_json::json!({"instance": {"id":id, "owner":"owner-a", "node_uid":"uid", "tier":"dev", "state":"Starting"}, "status":"Intent"});
        let header = serde_json::to_string(&state).unwrap();
        let duplicate = header.replace(
            "\"records\":{}",
            &format!("\"records\":{{\"{id}\":{record},\"{id}\":{record}}}"),
        );
        // Either record alone is a valid intent; duplicate-key rejection,
        // not an invalid identity after BTreeMap overwrite, must reject this.
        let mut single = state.clone();
        single["records"][&id] = record.clone();
        assert!(serde_json::from_value::<State>(single).is_ok());
        let error = serde_json::from_str::<State>(&duplicate).err().unwrap();
        assert!(error.to_string().contains("duplicate ID"));
        write_private(&path, duplicate.as_bytes());
        assert!(matches!(
            Local::open_development(&directory.0, "owner-a".into(), command(&[])),
            Err(Error::Storage)
        ));
        let records = (0..=MAX_RECORDS)
            .map(|n| (n.to_string(), record.clone()))
            .collect::<serde_json::Map<_, _>>();
        state["records"] = serde_json::Value::Object(records);
        write_private(&path, &serde_json::to_vec(&state).unwrap());
        assert!(matches!(
            Local::open_development(&directory.0, "owner-a".into(), command(&[])),
            Err(Error::Storage)
        ));
        write_private(&path, &vec![b' '; storage::MAX_BYTES + 1]);
        assert!(matches!(
            Local::open_development(&directory.0, "owner-a".into(), command(&[])),
            Err(Error::Storage)
        ));
    }
    #[test]
    fn anchor_or_lock_replacement_blocks_effects_without_repair() {
        for replace_name in [Some("lock"), Some("state.json"), None] {
            let directory = Directory::new();
            let mut p = directory.open(&["-c", "exec /bin/sleep 60"]);
            if let Some(name) = replace_name {
                let content = fs::read(directory.0.join(name)).unwrap();
                fs::rename(directory.0.join(name), directory.0.join("original-file")).unwrap();
                write_private(&directory.0.join(name), &content);
            } else {
                let moved = directory.0.with_extension("moved");
                fs::rename(&directory.0, &moved).unwrap();
                fs::create_dir(&directory.0).unwrap();
                fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o700)).unwrap();
                assert!(matches!(
                    p.launch(&development_offer(), &spec()),
                    Err(Error::Storage)
                ));
                assert!(p.processes.is_empty());
                drop(p);
                fs::remove_dir_all(moved).unwrap();
                continue;
            }
            assert!(matches!(
                p.launch(&development_offer(), &spec()),
                Err(Error::Storage)
            ));
            assert!(p.processes.is_empty());
            assert!(directory.0.join("original-file").exists());
        }
    }
    #[test]
    fn directory_symlink_even_with_trailing_separator_is_rejected() {
        use std::os::unix::fs::symlink;
        let directory = Directory::new();
        let alias = directory.0.with_extension("alias");
        symlink(&directory.0, &alias).unwrap();
        for path in [
            alias.clone(),
            PathBuf::from(format!("{}/", alias.display())),
        ] {
            assert!(matches!(
                Local::open_development(&path, "owner-a".into(), command(&[])),
                Err(Error::Storage)
            ));
        }
        fs::remove_file(alias).unwrap();
        assert!(!directory.0.join("lock").exists());
    }
    #[test]
    fn cloexec_lock_is_inherited_until_exec_and_fixture_waits_only_for_contention() {
        use std::io::{Read, Write};
        use std::os::{
            fd::{AsRawFd, FromRawFd},
            unix::process::CommandExt,
        };
        use std::process::{Child, Command, Stdio};
        fn pipe() -> (fs::File, fs::File) {
            let mut fds = [-1; 2];
            // SAFETY: two valid writable integers; both resulting fds are owned
            // exactly once by File and closed on exec, including failure paths.
            assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
            let read = unsafe { fs::File::from_raw_fd(fds[0]) };
            let write = unsafe { fs::File::from_raw_fd(fds[1]) };
            for f in [&read, &write] {
                assert_eq!(
                    unsafe { libc::fcntl(f.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) },
                    0
                );
            }
            (read, write)
        }
        struct ProbeChild(Child);
        impl Drop for ProbeChild {
            fn drop(&mut self) {
                // Only the retained direct synthetic child, never an arbitrary
                // numeric PID or host process scan. Also cleans up on panic.
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let directory = Directory::new();
        let local = directory.open(&["-c", "exit 0"]);
        let (mut ready, child_ready) = pipe();
        let (child_release, release) = pipe();
        let spawning = std::thread::spawn(move || {
            let ready_fd = child_ready.as_raw_fd();
            let release_fd = child_release.as_raw_fd();
            let mut command = Command::new("/bin/sleep");
            command
                .arg("60")
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            // SAFETY: child pre-exec code uses ONLY async-signal-safe libc
            // read/write and stack scalars, no Rust locks/allocations/destructors.
            // The pipes park exactly between fork and exec, deterministically
            // exposing the inherited open-file-description despite CLOEXEC.
            unsafe {
                command.pre_exec(move || {
                    let byte = [1u8];
                    let mut release = [0u8];
                    let mut ready = libc::pollfd {
                        fd: release_fd,
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    if libc::write(ready_fd, byte.as_ptr().cast(), 1) != 1
                        || libc::poll(&mut ready, 1, 2000) != 1
                        || libc::read(release_fd, release.as_mut_ptr().cast(), 1) != 1
                    {
                        return Err(std::io::Error::from_raw_os_error(libc::EIO));
                    }
                    Ok(())
                });
            }
            let child = ProbeChild(command.spawn().unwrap());
            drop((child_ready, child_release));
            child
        });
        let mut byte = [0u8];
        let mut poll = libc::pollfd {
            fd: ready.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll only inspects our live owned pipe; a bounded wait prevents
        // a failed fixture spawn from hanging the test runner.
        assert_eq!(unsafe { libc::poll(&mut poll, 1, 2000) }, 1);
        ready.read_exact(&mut byte).unwrap(); // child has inherited the locked fd
        drop(local); // no live Local owner remains in the parent
        assert!(matches!(
            Local::open_development(&directory.0, "owner-a".into(), command(&[])),
            Err(Error::Unavailable)
        ));
        let mut release = Some(release);
        let (reopened, retries) = directory.open_fixture(&["-c", "exit 0"], || {
            if let Some(mut release) = release.take() {
                release.write_all(&[1]).unwrap(); // allow exec, NOT process exit
            }
        });
        assert!(retries >= 1);
        let mut child = spawning.join().unwrap();
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "exec released the inherited lock while the child is still alive"
        );
        // Successfully reopened before child exit proves FD_CLOEXEC worked; it
        // does not prevent the demonstrated pre-exec inheritance interval.
        drop(reopened);
    }
    #[test]
    fn fixture_does_not_retry_or_swallow_storage_errors() {
        let directory = Directory::new();
        drop(directory.open(&["-c", "exit 0"]));
        write_private(
            &directory.0.join("state.json"),
            b"malformed synthetic state",
        );
        let result = std::panic::catch_unwind(|| {
            directory.open_fixture(&[], || panic!("Storage must never enter contention retry"));
        });
        let panic = result.expect_err("storage corruption must fail the fixture");
        assert!(
            panic
                .downcast_ref::<String>()
                .unwrap()
                .contains("unexpected fixture open error: Storage")
        );
    }
}
