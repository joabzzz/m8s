mod log_page;
mod mock_config;
mod mock_shutdown;
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
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
const HELP: &str = concat!(
    "m8sd <private-state-dir> [nonsecret-owner-label]\n",
    "m8sd <private-state-dir> [nonsecret-owner-label] --config ABS --catalog ABS\n",
    "Legacy: no config flags, Store-valid tiers and existing cap16. Configured: paired flags once, whole inputs validated before state effects.\n",
    "Configured up selects max(1,min), within max; positive scale must be within min..max. Known-tier scale0 and emergency down remain allowed even min>0.\n",
    "Max0 tiers are disabled: no up/positive scale. No automatic startup scaling or live reload. Restart retains desired; incompatible retained tiers/counts refuse without clamp/reset.\n",
    "Normal Store initialization/migrations still apply. Provider is hardwired Mock; mode mock-only, bootstrap unavailable, no Ready or authorization/deployment claim.\n",
    "Local shutdown commits down, requires confirmed Mock absence, then exits; refusal keeps daemon running with retained state.\n",
    "Standalone --help only; no config/state/file effects.\n",
);
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "--help" {
        let mut stdout = std::io::stdout().lock();
        stdout
            .write_all(HELP.as_bytes())
            .and_then(|()| stdout.flush())
            .map_err(|_| "daemon-help-output-unavailable")?;
        return Ok(());
    }
    let options = mock_config::Options::parse(args)?;
    // Whole config/catalog validation and configured identifier/bound
    // representability precede state/lock/socket/provider effects. Snapshot is
    // immutable here; persisted desired is checked after normal Store opening.
    let policy = options.policy()?;
    let dir = options.dir;
    let owner = options.owner;
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
    // Configured reopen checks persisted desired before provider opening,
    // reconciliation or even replacing the old socket. No clamp/reset/repair.
    let configured_store = if let Some(policy) = &policy {
        let store = Store::open(dir.join("state.db"), &owner)
            .map_err(|_| "configured mock state unavailable; state retained")?;
        let tiers = store
            .tiers()
            .map_err(|_| "configured mock state unavailable; state retained")?;
        policy.persisted(&tiers)?;
        Some(store)
    } else {
        None
    };
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
    let mut store = match configured_store {
        Some(store) => store,
        None => Store::open(dir.join("state.db"), &owner)?,
    };
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
                // On macOS a peer that disconnects immediately can make socket
                // option setup fail with EINVAL. That is a client failure, not
                // grounds to stop reconciliation for every other local client.
                // BSD may inherit O_NONBLOCK from the listener. A client can
                // validate its peer before writing; do not treat that normal
                // pause as an empty/failed request. Per-client I/O stays bounded.
                if stream.set_nonblocking(false).is_err()
                    || stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .is_err()
                    || stream
                        .set_write_timeout(Some(Duration::from_secs(2)))
                        .is_err()
                {
                    continue;
                }
                let mut page_request = None;
                let mut shutdown_requested = false;
                let mut shutdown_complete = false;
                let result = (|| -> Result<(), Box<dyn std::error::Error>> {
                    let frame = m8s_api::read_frame(&mut stream)?;
                    match serde_json::from_slice::<Request>(&frame)? {
                        Request::Up { tier } => {
                            let replicas = policy.as_ref().map_or(Ok(1), |p| p.up(&tier))?;
                            store.scale(&tier, replicas)?;
                        }
                        Request::Scale { tier, replicas } => {
                            let replicas = policy
                                .as_ref()
                                .map_or(Ok(replicas), |p| p.scale(&tier, replicas))?;
                            store.scale(&tier, replicas)?;
                        }
                        Request::Down {} => store.down()?,
                        Request::Shutdown {} => {
                            shutdown_requested = true;
                            mock_shutdown::shutdown(&mut store, &mut provider, now())?;
                            shutdown_complete = true;
                        }
                        Request::LogsPage { after, limit } => page_request = Some((after, limit)),
                        Request::Status {} | Request::Logs {} => {}
                    }
                    Ok(())
                })();
                if let Some((after, limit)) = page_request {
                    let frame = log_page::frame(&store, after, limit);
                    let _ = stream.write_all(&frame);
                    continue;
                }
                let snapshot = (|| -> m8sd::Result<Response> {
                    Ok(Response {
                        ok: result.is_ok(),
                        mode: "mock-only".into(),
                        nodes: store.nodes()?,
                        tiers: store.tiers()?,
                        events: store.logs()?,
                        error: result.err().map(|_| "control request refused".into()),
                        secure_bootstrap: "unavailable; nodes cannot become Ready".into(),
                    })
                })();
                let response = match snapshot {
                    Ok(response) => response,
                    Err(_) if shutdown_requested => {
                        // Failed observation is uncertainty, never an empty
                        // successful status or grounds to exit after an ACK.
                        shutdown_complete = false;
                        Response {
                            ok: false,
                            mode: "mock-only".into(),
                            nodes: vec![],
                            tiers: vec![],
                            events: vec![],
                            error: Some(
                                concat!(
                                    "mock shutdown refused; snapshot unavailable; ",
                                    "empty fields are not absence"
                                )
                                .into(),
                            ),
                            secure_bootstrap: "unavailable; nodes cannot become Ready".into(),
                        }
                    }
                    Err(error) => return Err(error.into()),
                };
                // Shutdown is already durable/confirmed. A disconnected client
                // cannot undo it; all other request disconnect behavior is unchanged.
                let _ = (|| -> std::io::Result<()> {
                    serde_json::to_writer(&mut stream, &response)?;
                    stream.write_all(b"\n")
                })();
                if shutdown_complete {
                    return Ok(()); // Drop listener, Store and process lock; no unlink/reset.
                }
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
