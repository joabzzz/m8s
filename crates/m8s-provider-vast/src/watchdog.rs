//! Cleanup worker entrypoint for a fresh, dedicated child process only.
use crate::readonly::{
    destroy::ScopedDestroyHttps,
    recovery::{RecoveryIntent, RecoveryLookup},
};
use m8s_secrets::SecretValue;
use serde::Deserialize;
use serde_json::json;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    num::NonZeroU64,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, ()>;
static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    schema: String,
    owner: String,
    node_uid: String,
    tier: String,
    image: String,
    // Absolute original deadline, never a fresh duration on restart.
    created_at: u64,
    destroy_at: u64,
}
impl Plan {
    fn validate(&self, now: u64) -> Result<RecoveryIntent> {
        if self.schema != "m8s.vast-watchdog/v1"
            || self.created_at > now
            || now.saturating_sub(self.created_at) > 30
            || self.destroy_at <= now
            || self.destroy_at <= self.created_at
            || self.destroy_at - self.created_at > 1800
        {
            return Err(());
        }
        RecoveryIntent::new(&self.owner, &self.node_uid, &self.tier, &self.image).map_err(|_| ())
    }
}

fn utc() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| ())
}

fn regular(file: &File) -> Result<()> {
    let meta = file.metadata().map_err(|_| ())?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o7777 != 0o600
        || meta.nlink() != 1
    {
        return Err(());
    }
    m8s_secrets::check_no_extended_acl(file).map_err(|_| ())
}

fn load_plan(path: &Path) -> Result<(Plan, File)> {
    if !path.is_absolute() || path.canonicalize().map_err(|_| ())? != path {
        return Err(());
    }
    let parent = path.parent().ok_or(())?;
    let directory = File::open(parent).map_err(|_| ())?;
    let meta = directory.metadata().map_err(|_| ())?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o7777 != 0o700 {
        return Err(());
    }
    m8s_secrets::check_no_extended_acl(&directory).map_err(|_| ())?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| ())?;
    regular(&file)?;
    let before = file.metadata().map_err(|_| ())?;
    if before.len() > 8192 {
        return Err(());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(8193)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    let after = file.metadata().map_err(|_| ())?;
    if bytes.len() > 8192
        || after.len() != before.len()
        || after.mtime() != before.mtime()
        || after.mtime_nsec() != before.mtime_nsec()
        || after.ctime() != before.ctime()
        || after.ctime_nsec() != before.ctime_nsec()
    {
        return Err(());
    }
    let plan: Plan = serde_json::from_slice(&bytes).map_err(|_| ())?;
    plan.validate(utc()?)?;
    Ok((plan, directory))
}

fn load(path: &Path) -> Result<(Plan, File)> {
    let (plan, directory) = load_plan(path)?;
    let parent = path.parent().ok_or(())?;
    // Exclusive creation is also the one-use arm latch. Never truncate a prior
    // run or delete a journal to retry. Recovery must retain its original record.
    let journal = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(parent.join("watchdog.events.jsonl"))
        .map_err(|_| ())?;
    regular(&journal)?;
    journal.sync_all().map_err(|_| ())?;
    directory.sync_all().map_err(|_| ())?;
    Ok((plan, journal))
}

fn record(file: &mut File, event: &str, id: Option<NonZeroU64>) -> Result<()> {
    let value = json!({"schema":"m8s.vast-watchdog-event/v1", "event":event,
        "instance_id":id.map(NonZeroU64::get), "observed_at":utc()?});
    serde_json::to_writer(&mut *file, &value).map_err(|_| ())?;
    file.write_all(b"\n").map_err(|_| ())?;
    file.sync_all().map_err(|_| ())
}

/// Wall-clock rollback cannot extend the captured monotonic deadline; a forward
/// jump or explicit signal triggers cleanup early. Neither can renew the plan.
fn cleanup_due(
    elapsed: Duration,
    original_remaining: Duration,
    now: u64,
    destroy_at: u64,
    stop_requested: bool,
) -> bool {
    stop_requested || elapsed >= original_remaining || now >= destroy_at
}

/// Call ONLY in the dedicated worker process: detaches its session, owns signal
/// handlers, consumes credential stdin and runs through the original deadline.
/// No runtime endpoint or provider create is performed here.
pub fn run(path: &Path) -> m8s_core::provider::Result<()> {
    run_inner(path).map_err(|_| m8s_core::provider::Error::Unavailable)
}

fn detach() -> Result<()> {
    // Only a fresh ordinary child may call this; never an existing group leader.
    if unsafe { libc::setsid() } < 0 {
        return Err(());
    }
    unsafe {
        if libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t) == libc::SIG_ERR
            || libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t) == libc::SIG_ERR
            || libc::signal(libc::SIGHUP, stop as *const () as libc::sighandler_t) == libc::SIG_ERR
        {
            return Err(());
        }
    }
    Ok(())
}

fn run_inner(path: &Path) -> Result<()> {
    let (plan, mut journal) = load(path)?;
    // Input is an anonymous pipe, never argv/environment/a credential pathname.
    // The launcher writes the key and closes its pipe before awaiting readiness.
    let mut input = Vec::new();
    std::io::stdin()
        .take(4097)
        .read_to_end(&mut input)
        .map_err(|_| ())?;
    let secret = SecretValue::new(input);
    if secret.expose().is_empty()
        || secret.expose().len() > 4096
        || !secret.expose().iter().all(|b| (33..=126).contains(b))
    {
        return Err(());
    }
    let intent = plan.validate(utc()?)?;
    let mut lookup = RecoveryLookup::new(intent, || Ok(SecretValue::new(secret.expose().to_vec())))
        .map_err(|_| ())?;
    // Own a session so the launching terminal/process group may exit without
    // killing cleanup. The caller must spawn this as an ordinary child, not as
    // an existing process-group leader. A failure to detach refuses before arm.
    detach()?;
    let started = Instant::now();
    let remaining = Duration::from_secs(plan.destroy_at.checked_sub(utc()?).ok_or(())?);
    record(&mut journal, "armed", None)?;
    std::io::stdout()
        .write_all(&arm_ack(&plan)?)
        .map_err(|_| ())?;
    std::io::stdout().flush().map_err(|_| ())?;
    let mut bound = None;
    let mut destroy_acknowledged = false;
    let mut persistence_failed = false;
    let mut cleanup_started = None;
    loop {
        let now = utc().unwrap_or(u64::MAX); // clock failure triggers cleanup
        let due = cleanup_due(
            started.elapsed(),
            remaining,
            now,
            plan.destroy_at,
            STOP.load(Ordering::Relaxed),
        );
        if due && cleanup_started.is_none() {
            cleanup_started = Some(Instant::now());
        }
        if bound.is_none()
            && let Ok(Some(candidate)) = lookup.find()
        {
            let id = candidate.instance_id();
            // Persist before mutation. If persistence fails, keep the
            // positive in-memory binding and clean up; never create again.
            persistence_failed |= record(&mut journal, "exact_resource_bound", Some(id)).is_err();
            bound = Some(id);
        }
        if due {
            if let Some(id) = bound {
                let mut client =
                    ScopedDestroyHttps::new(id, || Ok(SecretValue::new(secret.expose().to_vec())))
                        .map_err(|_| ())?;
                if matches!(client.confirm_absent(), Ok(Some(_))) {
                    record(&mut journal, "provider_confirmed_absent", Some(id))?;
                    return if persistence_failed { Err(()) } else { Ok(()) };
                }
                if !destroy_acknowledged && client.destroy().is_ok() {
                    destroy_acknowledged = true;
                    persistence_failed |=
                        record(&mut journal, "destroy_acknowledged", Some(id)).is_err();
                }
            }
            if cleanup_started.is_some_and(|at| at.elapsed() >= Duration::from_secs(180)) {
                let _ = record(&mut journal, "cleanup_unresolved", bound);
                return Err(());
            }
        }
        // At most one lookup or exact-resource observation cycle every 3s.
        // Each HTTPS call also has a fixed 10s timeout and no transport retries.
        std::thread::sleep(Duration::from_secs(3));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn plan() -> Plan {
        Plan {
            schema: "m8s.vast-watchdog/v1".into(),
            owner: "synthetic-owner".into(),
            node_uid: "synthetic-node".into(),
            tier: "smoke".into(),
            image: format!("example.invalid/mesh@sha256:{}", "a".repeat(64)),
            created_at: 100,
            destroy_at: 700,
        }
    }
    #[test]
    fn stale_or_rebased_plans_refuse() {
        let mut p = plan();
        assert!(p.validate(100).is_ok());
        for now in [99, 131, 700, u64::MAX] {
            assert!(p.validate(now).is_err());
        }
        p.destroy_at = 1901;
        assert!(p.validate(100).is_err());
        p.destroy_at = 100;
        assert!(p.validate(100).is_err());
    }
    #[test]
    fn deadlines_cannot_be_extended_by_wall_clock_rollback() {
        let remaining = Duration::from_secs(600);
        assert!(!cleanup_due(Duration::ZERO, remaining, 100, 700, false));
        assert!(cleanup_due(remaining, remaining, 0, 700, false));
        assert!(cleanup_due(Duration::ZERO, remaining, 700, 700, false));
        assert!(cleanup_due(Duration::ZERO, remaining, 100, 700, true));
    }
    #[test]
    fn unknown_fields_and_credentials_are_not_plan_inputs() {
        let value = json!({"schema":"m8s.vast-watchdog/v1","owner":"o","node_uid":"n",
            "tier":"t","image":"bad","created_at":100,"destroy_at":200,"key":"synthetic"});
        assert!(serde_json::from_value::<Plan>(value).is_err());
    }
}

#[cfg(test)]
mod process_tests;

mod launcher;
pub use launcher::ArmedCleanup;
#[cfg(feature = "launch-https")]
pub use launcher::OwnedResourceAbsence;

fn arm_ack(plan: &Plan) -> Result<Vec<u8>> {
    let mut ack = b"armed ".to_vec();
    serde_json::to_writer(&mut ack, plan).map_err(|_| ())?;
    ack.push(b'\n');
    Ok(ack)
}
