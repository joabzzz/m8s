//! Explicit selected plan and executable. No credential discovery or provider I/O.
use super::*;
use serde::Deserialize;
use std::io::Read;

#[derive(Deserialize)]
pub(super) struct Plan {
    pub schema: String,
    pub directory: PathBuf,
    pub control_directory: PathBuf,
    pub owner: String,
    pub tier: String,
}
pub(super) struct Options {
    pub root: PathBuf,
    pub cli: PathBuf,
    pub plan: Plan,
    pub bytes: Vec<u8>,
}
pub(super) fn read_regular(path: &Path, limit: u64, executable: bool) -> Result<Vec<u8>> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| ERROR)?;
    let m = file.metadata().map_err(|_| ERROR)?;
    if !m.is_file()
        || (!executable && m.nlink() != 1)
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o7022 != 0
        || m.len() > limit
        || (executable && m.mode() & 0o111 == 0)
    {
        return Err(ERROR);
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ERROR)?;
    if bytes.len() as u64 > limit {
        return Err(ERROR);
    }
    Ok(bytes)
}
impl Options {
    pub fn parse(args: &[std::ffi::OsString]) -> Result<Self> {
        if args.len() != 7
            || args[0] != "--vast-demo"
            || args[1] != "--state-dir"
            || args[3] != "--plan"
            || args[5] != "--m8s"
        {
            return Err(ERROR);
        }
        let root = PathBuf::from(&args[2]);
        let selected = PathBuf::from(&args[4]);
        let cli = PathBuf::from(&args[6]);
        if [&root, &selected, &cli].iter().any(|p| !p.is_absolute()) {
            return Err(ERROR);
        }
        if selected.canonicalize().map_err(|_| ERROR)? != selected
            || cli.canonicalize().map_err(|_| ERROR)? != cli
        {
            return Err(ERROR);
        }
        let bytes = read_regular(&selected, 32768, false)?;
        let plan: Plan = serde_json::from_slice(&bytes).map_err(|_| ERROR)?;
        let valid_label = |s: &str| {
            !s.is_empty()
                && s.len() <= 64
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        };
        if plan.schema != "m8s.vast-demo/v1"
            || plan.control_directory != root
            || !plan.directory.is_absolute()
            || plan.directory == root
            || !valid_label(&plan.owner)
            || !valid_label(&plan.tier)
        {
            return Err(ERROR);
        }
        Ok(Self {
            root,
            cli,
            plan,
            bytes,
        })
    }
    pub fn initialize(&self) -> Result<fs::File> {
        match fs::symlink_metadata(&self.root) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                use std::os::unix::fs::DirBuilderExt;
                fs::DirBuilder::new()
                    .mode(0o700)
                    .create(&self.root)
                    .map_err(|_| ERROR)?;
            }
            Ok(_) => (),
            Err(_) => return Err(ERROR),
        }
        let m = fs::symlink_metadata(&self.root).map_err(|_| ERROR)?;
        if !m.is_dir()
            || m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o7777 != 0o700
            || self.root.canonicalize().map_err(|_| ERROR)? != self.root
        {
            return Err(ERROR);
        }
        m8sd::verify_private_directory(&self.root).map_err(|_| ERROR)?;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.root.join("daemon.lock"))
            .map_err(|_| ERROR)?;
        let m = lock.metadata().map_err(|_| ERROR)?;
        if !m.is_file()
            || m.nlink() != 1
            || m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o7777 != 0o600
        {
            return Err(ERROR);
        }
        lock.try_lock_exclusive().map_err(|_| ERROR)?;
        let stamp = self.root.join("controller.json");
        // Reject an existing attempt before hashing executables or invoking
        // plan validation. It can never be adopted by a new controller.
        if !stamp.exists() && self.plan.directory.exists() {
            return Err(ERROR);
        }
        let binary = read_regular(&self.cli, 256 * 1024 * 1024, true)?;
        let daemon = std::env::current_exe().map_err(|_| ERROR)?;
        let daemon_bytes = read_regular(&daemon, 256 * 1024 * 1024, true)?;
        use sha2::{Digest, Sha256};
        let fingerprint = serde_json::to_vec(&serde_json::json!({
            "plan_sha256":format!("{:x}", Sha256::digest(&self.bytes)),
            "cli_sha256":format!("{:x}", Sha256::digest(binary)),
            "daemon_sha256":format!("{:x}", Sha256::digest(daemon_bytes)),
            "cli":self.cli
        }))
        .map_err(|_| ERROR)?;
        if stamp.exists() {
            if read_regular(&stamp, 8192, false)? != fingerprint {
                return Err(ERROR);
            }
        } else {
            // Full CLI validation is effect-free. Store only its selected plan;
            // up has not happened, and no provider credential is opened here.
            let selected = self.root.join("launch-plan.json");
            write_new(&selected, &self.bytes)?;
            check_plan(&self.cli, &selected)?;
            write_new(&stamp, &fingerprint)?;
        }
        if read_regular(&self.root.join("launch-plan.json"), 32768, false)? != self.bytes {
            return Err(ERROR);
        }
        Ok(lock)
    }
}
fn check_plan(cli: &Path, plan: &Path) -> Result<()> {
    let mut child = Command::new(cli)
        .args(["prototype", "launch-check", "--plan"])
        .arg(plan)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| ERROR)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success().then_some(()).ok_or(ERROR),
            Ok(None) => (),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ERROR);
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ERROR);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
