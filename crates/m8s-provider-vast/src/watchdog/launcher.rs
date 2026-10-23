//! Parent half of the fixed m8s CLI worker protocol. No create or key discovery.
use super::*;
use std::{
    os::fd::AsRawFd,
    process::{Child, Command, Stdio},
};

/// Acknowledged cleanup child for one immutable recovery intent and deadline.
/// No public constructor, Clone, Debug or deserialization. The worker remains
/// independently responsible after parent loss; dropping this handle requests
/// early cleanup, never SIGKILL or an assertion that resources are absent.
///
/// ```compile_fail
/// use m8s_provider_vast::watchdog::ArmedCleanup;
/// let _: ArmedCleanup = serde_json::from_str("{}").unwrap();
/// ```
pub struct ArmedCleanup {
    child: Child,
    secret: SecretValue,
    #[cfg(feature = "launch-https")]
    submitted: bool,
    intent: RecoveryIntent,
    destroy_at: u64,
    started: Instant,
    remaining: Duration,
}
impl ArmedCleanup {
    /// Only call from the m8s CLI: re-executes the current binary's fixed worker
    /// route. The already-private plan and original budget must be durable first.
    /// The account key travels solely through anonymous stdin, with an empty
    /// child environment. No provider create is performed by either process.
    pub fn arm_cli(path: &Path, secret: SecretValue) -> m8s_provider::Result<Self> {
        Self::arm_inner(path, secret).map_err(|_| m8s_provider::Error::Unavailable)
    }

    fn arm_inner(path: &Path, secret: SecretValue) -> Result<Self> {
        let (plan, _directory) = load_plan(path)?;
        let now = utc()?;
        let intent = plan.validate(now)?;
        if secret.expose().is_empty()
            || secret.expose().len() > 4096
            || !secret.expose().iter().all(|b| (33..=126).contains(b))
        {
            return Err(());
        }
        let started = Instant::now();
        let remaining = Duration::from_secs(plan.destroy_at.checked_sub(now).ok_or(())?);
        let mut command = Command::new(std::env::current_exe().map_err(|_| ())?);
        command
            .args(["prototype", "vast-cleanup-worker", "--plan"])
            .arg(path)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let child = command.spawn().map_err(|_| ())?;
        let mut handle = Self {
            child,
            secret,
            #[cfg(feature = "launch-https")]
            submitted: false,
            intent,
            destroy_at: plan.destroy_at,
            started,
            remaining,
        };
        // Install custody before any fallible write/read. Failure requests stop
        // through Drop; uncertain arm never authorizes a create retry.
        let mut input = handle.child.stdin.take().ok_or(())?;
        input.write_all(handle.secret.expose()).map_err(|_| ())?;
        drop(input);
        let mut output = handle.child.stdout.take().ok_or(())?;
        read_ack(&mut output, &arm_ack(&plan)?)?;
        if !handle.live()? {
            return Err(());
        }
        Ok(handle)
    }

    fn live(&mut self) -> Result<bool> {
        Ok(self.child.try_wait().map_err(|_| ())?.is_none()
            && self.started.elapsed() < self.remaining
            && utc()? < self.destroy_at)
    }

    /// A live process acknowledgment, not provider absence, billing containment,
    /// admission, or Ready. Recheck immediately before burning the invocation.
    pub fn guards(
        &mut self,
        intent: &RecoveryIntent,
        destroy_at: u64,
    ) -> m8s_provider::Result<bool> {
        let same = self.intent.label() == intent.label()
            && self.intent.image() == intent.image()
            && self.destroy_at == destroy_at;
        self.live()
            .map(|live| same && live)
            .map_err(|_| m8s_provider::Error::Unavailable)
    }

    pub fn request_cleanup(&mut self) {
        // An unreaped child cannot have its PID reused. Never signal after a
        // successful wait/reap, and never send signals to an arbitrary PID.
        if matches!(self.child.try_wait(), Ok(None))
            && let Ok(pid) = i32::try_from(self.child.id())
        {
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
    }

    /// Uses the exact same account credential delivered to the acknowledged
    /// cleanup worker. The coordinator must first burn its durable invocation.
    /// One attempt per handle, including construction/transport errors.
    #[cfg(feature = "launch-https")]
    pub fn dispatch_once(
        &mut self,
        request: crate::readonly::launch::LaunchRequest,
    ) -> m8s_provider::Result<crate::readonly::launch::CreatedCandidate> {
        if self.submitted || !self.guards(request.identity(), self.destroy_at)? {
            return Err(m8s_provider::Error::Unavailable);
        }
        self.submitted = true;
        crate::readonly::launch::OneShotLaunch::new(request, || {
            Ok(SecretValue::new(self.secret.expose().to_vec()))
        })?
        .dispatch()
    }
}
impl Drop for ArmedCleanup {
    fn drop(&mut self) {
        self.request_cleanup();
    }
}

fn read_ack(output: &mut std::process::ChildStdout, expected_ack: &[u8]) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    for expected in expected_ack {
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(());
            }
            let mut fd = libc::pollfd {
                fd: output.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let result =
                unsafe { libc::poll(&mut fd, 1, remaining.as_millis().min(10_000) as i32) };
            if result < 0
                && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
            {
                continue;
            }
            if result <= 0 {
                return Err(());
            }
            let mut byte = [0];
            if output.read(&mut byte).map_err(|_| ())? != 1 || byte[0] != *expected {
                return Err(());
            }
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arm_protocol_rejects_partial_wrong_and_unbound_receipts() {
        for (output, valid) in [
            ("armed synthetic\n", true),
            ("armed\n", false),
            ("armed other\n", false),
            ("armed synthetic", false),
            ("", false),
        ] {
            // Public synthetic protocol text only; no shell, provider, or keys.
            let mut child = Command::new("/usr/bin/printf")
                .args(["%s", output])
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let result = read_ack(&mut child.stdout.take().unwrap(), b"armed synthetic\n");
            assert!(child.wait().unwrap().success());
            assert_eq!(result.is_ok(), valid);
        }
    }
    #[test]
    fn acknowledgement_binds_every_public_plan_field() {
        let value = serde_json::json!({"schema":"m8s.vast-watchdog/v1","owner":"owner","node_uid":"node","tier":"smoke",
            "image":format!("example.invalid/mesh@sha256:{}","a".repeat(64)),"created_at":100,"destroy_at":700});
        let plan: Plan = serde_json::from_value(value.clone()).unwrap();
        let original = arm_ack(&plan).unwrap();
        for field in [
            "schema",
            "owner",
            "node_uid",
            "tier",
            "image",
            "created_at",
            "destroy_at",
        ] {
            let mut changed = value.clone();
            changed[field] = if field.ends_with("_at") {
                serde_json::json!(101)
            } else {
                serde_json::json!("changed")
            };
            let changed: Plan = serde_json::from_value(changed).unwrap();
            assert_ne!(original, arm_ack(&changed).unwrap());
        }
    }
}
