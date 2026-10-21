use fs2::FileExt;
use m8s_api::{Request, Response};
use m8s_provider::Mock;
use m8sd::Store;
use std::{
    fs,
    io::Write,
    os::unix::{
        fs::{FileTypeExt, PermissionsExt},
        net::UnixListener,
    },
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(
        args.next()
            .ok_or("usage: m8sd <private-state-directory> [owner-label]; mock only")?,
    );
    let owner = args.next().unwrap_or_else(|| "local-mock-owner".into());
    if args.next().is_some() {
        return Err("too many arguments".into());
    }
    match fs::symlink_metadata(&dir) {
        Ok(m)
            if !m.is_dir() || m.file_type().is_symlink() || m.permissions().mode() & 0o077 != 0 =>
        {
            return Err("state directory must be a real private directory (0700)".into());
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(&dir)?;
        }
        Err(e) => return Err(e.into()),
    }
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("daemon.lock"))?;
    lock.try_lock_exclusive()
        .map_err(|_| "another daemon owns this state directory")?;
    let socket = dir.join("control.sock");
    if let Ok(meta) = fs::symlink_metadata(&socket) {
        if !meta.file_type().is_socket() {
            return Err("refusing to replace non-socket control path".into());
        }
        fs::remove_file(&socket)?;
    }
    let listener = UnixListener::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let mut store = Store::open(dir.join("state.db"), &owner)?;
    let mut provider = Mock::persistent(dir.join("mock-provider.json"))?;
    let mut last = Instant::now() - Duration::from_secs(30);
    eprintln!(
        "m8sd: mock-only control at {}; secure bootstrap unavailable",
        socket.display()
    );
    loop {
        if last.elapsed() >= Duration::from_secs(1) {
            if store.reconcile(&mut provider, now()).is_err() {
                eprintln!("m8sd: reconcile uncertain; durable state retained");
            }
            last = Instant::now();
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                stream.set_write_timeout(Some(Duration::from_secs(2)))?;
                let result = (|| -> Result<(), Box<dyn std::error::Error>> {
                    let frame = m8s_api::read_frame(&mut stream)?;
                    match serde_json::from_slice::<Request>(&frame)? {
                        Request::Up { tier } => store.scale(&tier, 1)?,
                        Request::Scale { tier, replicas } => store.scale(&tier, replicas)?,
                        Request::Down {} => store.down()?,
                        Request::Status {} | Request::Logs {} => {}
                    }
                    Ok(())
                })();
                let response = Response {
                    ok: result.is_ok(),
                    mode: "mock-only".into(),
                    nodes: store.nodes()?,
                    tiers: store.tiers()?,
                    events: store.logs()?,
                    error: result.err().map(|_| "control request refused".into()),
                    secure_bootstrap: "unavailable; nodes cannot become Ready".into(),
                };
                // A disconnected client cannot terminate the daemon.
                let _ = (|| -> std::io::Result<()> {
                    serde_json::to_writer(&mut stream, &response)?;
                    stream.write_all(b"\n")
                })();
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50))
            }
            Err(e) => return Err(e.into()),
        }
    }
}
fn main() {
    if let Err(e) = run() {
        eprintln!("m8sd: {e}");
        std::process::exit(1)
    }
}
