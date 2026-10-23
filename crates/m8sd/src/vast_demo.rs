//! Opt-in one-attempt controller over the genuine bounded Vast CLI coordinator.
//! No mock provider, replacement, credential handling or deadline renewal.
mod options;
use fs2::FileExt;
use m8s_api::{Request, Response};
use std::{
    fs,
    io::{BufRead, Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::UnixListener,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, &'static str>;
const ERROR: &str = "Vast demo controller refused; retained state is not absence";

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| ERROR)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| ERROR)?;
    fs::File::open(path.parent().ok_or(ERROR)?)
        .and_then(|f| f.sync_all())
        .map_err(|_| ERROR)
}
struct Controller {
    options: options::Options,
    child: Option<Child>,
    desired: u32,
}
impl Controller {
    fn up(&mut self, tier: &str) -> Result<()> {
        if tier != self.options.plan.tier || self.options.root.join("down-requested").exists() {
            return Err(ERROR);
        }
        if let Some(child) = &mut self.child
            && child.try_wait().map_err(|_| ERROR)?.is_none()
        {
            return Ok(());
        }
        // The durable single-use claim precedes spawn and is never reset, even
        // when exec fails or a daemon restarts before provider CREATE is known.
        write_new(&self.options.root.join("launch-claimed"), b"claimed\n")?;
        let log = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.options.root.join("coordinator.log"))
            .map_err(|_| ERROR)?;
        let mut child = Command::new(&self.options.cli)
            .args(["prototype", "launch", "--plan"])
            .arg(self.options.root.join("launch-plan.json"))
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| ERROR)?;
        let output = child.stderr.take().ok_or(ERROR)?;
        let _reader = std::thread::spawn(move || capture_stages(output, log));
        self.child = Some(child);
        self.desired = 1;
        Ok(())
    }
    fn down(&mut self) -> Result<()> {
        let path = self.options.root.join("down-requested");
        if !path.exists() {
            write_new(&path, b"down\n")?;
        }
        self.desired = 0;
        Ok(())
    }
    fn request(&mut self, request: Request) -> Result<()> {
        match request {
            Request::Up { tier } => self.up(&tier),
            Request::Scale { tier, replicas: 1 } => self.up(&tier),
            Request::Scale { tier, replicas: 0 } if tier == self.options.plan.tier => self.down(),
            Request::Down {} => self.down(),
            Request::Status {} | Request::Logs {} => Ok(()),
            // No batching, replacement, reload, config, broad cleanup or shutdown
            // shortcut: at most one admitted attempt, with its original deadline.
            _ => Err(ERROR),
        }
    }
    fn response(&mut self, ok: bool) -> Response {
        if let Some(child) = &mut self.child
            && let Ok(Some(status)) = child.try_wait()
        {
            let path = self.options.root.join("coordinator-exit.json");
            if !path.exists() {
                let bytes = serde_json::to_vec(&serde_json::json!({"success":status.success()}))
                    .unwrap_or_default();
                let _ = write_new(&path, &bytes);
            }
        }
        let mut response = Response {
            ok,
            mode: m8s_api::VAST_DEMO_MODE.into(),
            nodes: vec![],
            tiers: vec![(self.options.plan.tier.clone(), self.desired)],
            events: vec![],
            error: (!ok).then(|| ERROR.into()),
            secure_bootstrap: m8s_api::VAST_DEMO_SECURITY.into(),
        };
        let db = self.options.plan.directory.join("state.db");
        if db.exists() {
            match m8sd::Store::open(&db, &self.options.plan.owner)
                .and_then(|s| s.managed_control_snapshot())
            {
                Ok(snapshot) => response.nodes = snapshot.nodes,
                Err(_) => {
                    response.ok = false;
                    response.error = Some(ERROR.into());
                }
            }
        }
        response.events = stages(&self.options.root.join("coordinator.log"));
        if let Ok(bytes) =
            options::read_regular(&self.options.root.join("coordinator-exit.json"), 128, false)
            && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
            && let Some(success) = value.get("success").and_then(serde_json::Value::as_bool)
        {
            response.events.push(
                if success {
                    "coordinator exited successfully; verify its removal receipt"
                } else {
                    "coordinator failed; resource outcome uncertain and liability retained"
                }
                .into(),
            );
        }
        if self.options.root.join("launch-claimed").exists() && !db.exists() {
            response
                .events
                .push("attempt claimed; provider outcome not yet observed".into());
        }
        response
    }
}
fn capture_stages(output: impl std::io::Read, mut log: fs::File) {
    let mut reader = std::io::BufReader::new(output);
    let mut total = 0;
    loop {
        let mut line = Vec::new();
        // Retain only bounded static stage labels, not native logs, JSON output,
        // provider replies, errors with paths, or completion text.
        let n = std::io::Read::by_ref(&mut reader)
            .take(257)
            .read_until(b'\n', &mut line);
        let Ok(n) = n else {
            return;
        };
        if n == 0 {
            return;
        }
        if line.len() > 256 || !line.ends_with(b"\n") {
            // Drain an oversized line without allocating it or closing the
            // coordinator's stderr pipe. Observability must not interrupt its
            // cleanup path with a broken pipe.
            while !line.ends_with(b"\n") {
                line.clear();
                match std::io::Read::by_ref(&mut reader)
                    .take(257)
                    .read_until(b'\n', &mut line)
                {
                    Ok(0) | Err(_) => return,
                    Ok(_) => (),
                }
            }
            continue;
        }
        if safe_stage(&line) {
            if total + line.len() > 16384 {
                continue;
            }
            total += line.len();
            if log.write_all(&line).and_then(|()| log.sync_data()).is_err() {
                return;
            }
        }
    }
}
fn safe_stage(line: &[u8]) -> bool {
    if let Some(label) = line
        .strip_prefix(b"preflight stage: ")
        .and_then(|s| s.strip_suffix(b"\n"))
    {
        return [
            "vast-sizing-validation",
            "vast-account-read",
            "vast-account-identity",
            "vast-credit-check",
            "vast-complete-inventory",
            "vast-volume-inventory",
            "vast-offer-read",
            "vast-offer-price-qualification",
        ]
        .iter()
        .any(|allowed| label == allowed.as_bytes());
    }
    let Some(label) = line
        .strip_prefix(b"prototype stage: ")
        .and_then(|s| s.strip_suffix(b"\n"))
    else {
        return false;
    };
    [
        "certified-owner-client-ready",
        "provisioning-key-created",
        "selected-credential-read",
        "provider-client-initialized",
        "fresh-account-and-offer-qualified",
        "cleanup-armed",
        "exact-instance-network-observed",
        "provisioning-host-key-pinned",
        "cleanup-requested",
        "exact-instance-absence-confirmed",
        "ssh-provision-sent",
        "provisioned-control-listener-ready",
        "certificate-delivered-control-established",
        "owner-routed-model-observed",
        "owner-routed-inference-completed",
        "completion-held-until-down-or-deadline",
    ]
    .iter()
    .any(|allowed| label == allowed.as_bytes())
}

fn stages(path: &Path) -> Vec<String> {
    options::read_regular(path, 16384, false)
        .ok()
        .map_or_else(Vec::new, |b| {
            b.split_inclusive(|c| *c == b'\n')
                .filter(|s| safe_stage(s))
                .take(128)
                .filter_map(|s| {
                    std::str::from_utf8(s)
                        .ok()
                        .map(str::trim_end)
                        .map(str::to_owned)
                })
                .collect()
        })
}
pub(super) fn run(args: &[std::ffi::OsString]) -> Result<()> {
    let options = options::Options::parse(args)?;
    let _lock = options.initialize()?;
    let socket = options.root.join("control.sock");
    if let Ok(m) = fs::symlink_metadata(&socket) {
        if !m.file_type().is_socket() || m.uid() != unsafe { libc::geteuid() } {
            return Err(ERROR);
        }
        fs::remove_file(&socket).map_err(|_| ERROR)?;
    }
    let listener = UnixListener::bind(&socket).map_err(|_| ERROR)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).map_err(|_| ERROR)?;
    listener.set_nonblocking(true).map_err(|_| ERROR)?;
    // Restart is observation/control only; never replay up or respawn a launcher.
    let desired = u32::from(
        options.root.join("launch-claimed").exists()
            && !options.root.join("down-requested").exists(),
    );
    let mut controller = Controller {
        options,
        child: None,
        desired,
    };
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let Ok(mut io) = super::control_io::ControlIo::new(
                    stream,
                    Instant::now() + Duration::from_secs(2),
                ) else {
                    continue;
                };
                let ok = io
                    .read_frame()
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .is_some_and(|r| controller.request(r).is_ok());
                let response = controller.response(ok);
                if let Ok(mut bytes) = serde_json::to_vec(&response)
                    && bytes.len() < m8s_api::MAX_FRAME
                {
                    bytes.push(b'\n');
                    let _ = io.write_all(&bytes);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50))
            }
            Err(_) => return Err(ERROR),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn oversized_output_is_drained_and_later_stages_are_retained() {
        use std::os::unix::fs::OpenOptionsExt;
        let path = std::env::temp_dir().join(format!(
            "m8s-stage-drain-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let log = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        let mut output = vec![b'x'; 10000];
        output.extend_from_slice(b"\nprototype stage: cleanup-armed\n");
        super::capture_stages(output.as_slice(), log);
        assert_eq!(super::stages(&path), vec!["prototype stage: cleanup-armed"]);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn arbitrary_child_text_cannot_become_an_operational_log() {
        for line in [
            b"m8s: synthetic-private-value\n".as_slice(),
            b"prototype stage: synthetic-private-value\n",
            b"prototype stage: cleanup-armed extra\n",
            b"prototype stage: cleanup-armed\nsecond-line\n",
        ] {
            assert!(!super::safe_stage(line));
        }
        assert!(super::safe_stage(b"prototype stage: cleanup-armed\n"));
        assert!(super::safe_stage(
            b"preflight stage: vast-complete-inventory\n"
        ));
        assert!(!super::safe_stage(
            b"preflight stage: untrusted-private-value\n"
        ));
        assert!(!super::safe_stage(
            b"preflight stage: vast-offer-read\nextra\n"
        ));
    }
}
