//! Opt-in real-provider recovery daemon. It resumes a managed Store left by a
//! prior coordinator and performs observation and cleanup only: no launch, no
//! replacement, no Ready, no broad inventory. Credentials come solely from the
//! explicitly selected file and are read per provider call.
use fs2::FileExt;
use m8s_api::{Request, Response};
use m8sd::{
    Store,
    operation::vast::{CleanupTrigger, HttpsRestartPort, VastDaemon, credential_supplier},
};
use std::{
    ffi::OsString,
    fs,
    io::Write,
    os::unix::{
        fs::{FileTypeExt, MetadataExt, PermissionsExt},
        net::UnixListener,
    },
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
type Result<T> = std::result::Result<T, &'static str>;
const ERROR: &str = "Vast recovery daemon refused; retained state is not absence";
const TICK: Duration = Duration::from_secs(5);

struct Options {
    dir: PathBuf,
    owner: String,
    credential: PathBuf,
    trigger: CleanupTrigger,
}
fn parse(args: &[OsString]) -> Result<Options> {
    let cleanup_now = match args.len() {
        7 => false,
        8 if args[7] == "--cleanup-now" => true,
        _ => return Err(ERROR),
    };
    if args[0] != "--vast-daemon"
        || args[1] != "--state-dir"
        || args[3] != "--owner"
        || args[5] != "--credential-file"
    {
        return Err(ERROR);
    }
    let dir = PathBuf::from(&args[2]);
    let credential = PathBuf::from(&args[6]);
    let owner = args[4].to_str().ok_or(ERROR)?.to_owned();
    if !dir.is_absolute()
        || !credential.is_absolute()
        || owner.is_empty()
        || owner.len() > 64
        || !owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(ERROR);
    }
    Ok(Options {
        dir,
        owner,
        credential,
        // Without a surviving worker the original serving deadline, or an
        // operator down, is enough to bound liability. Immediate cleanup can
        // destroy a resource another live process is still serving, so it is
        // an explicit operator choice.
        trigger: if cleanup_now {
            CleanupTrigger::Immediate
        } else {
            CleanupTrigger::AtDeadline
        },
    })
}
fn now() -> m8sd::Result<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .ok_or(m8sd::Error::Invalid)
}

pub(super) fn run(args: &[OsString]) -> Result<()> {
    let options = parse(args)?;
    // The state directory must already exist: this mode never creates a domain.
    let meta = fs::symlink_metadata(&options.dir).map_err(|_| ERROR)?;
    if !meta.is_dir()
        || meta.file_type().is_symlink()
        || meta.permissions().mode() & 0o077 != 0
        || meta.uid() != unsafe { libc::geteuid() }
    {
        return Err(ERROR);
    }
    // An unusable credential file is a startup failure, not a silent no-op loop.
    let supplier = credential_supplier(options.credential.clone());
    supplier().map_err(|_| ERROR)?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(options.dir.join("daemon.lock"))
        .map_err(|_| ERROR)?;
    lock.try_lock_exclusive().map_err(|_| ERROR)?;
    let store = Store::open(options.dir.join("state.db"), &options.owner).map_err(|_| ERROR)?;
    let mut daemon = VastDaemon::new(store, HttpsRestartPort::new(supplier), options.trigger)
        .map_err(|_| ERROR)?;
    let socket = options.dir.join("control.sock");
    if let Ok(meta) = fs::symlink_metadata(&socket) {
        if !meta.file_type().is_socket() || meta.uid() != unsafe { libc::geteuid() } {
            return Err(ERROR);
        }
        fs::remove_file(&socket).map_err(|_| ERROR)?;
    }
    let listener = UnixListener::bind(&socket).map_err(|_| ERROR)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).map_err(|_| ERROR)?;
    listener.set_nonblocking(true).map_err(|_| ERROR)?;
    eprintln!(
        "m8sd: vast recovery control at {}; launch not composed; no Ready",
        socket.display()
    );
    let mut last = Instant::now() - TICK;
    loop {
        if last.elapsed() >= TICK {
            if daemon.tick(now).is_err() {
                eprintln!("m8sd: vast reconcile uncertain; durable state retained");
            }
            last = Instant::now();
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let Ok(mut io) = super::control_io::ControlIo::new(
                    stream,
                    Instant::now() + Duration::from_secs(2),
                ) else {
                    continue;
                };
                let response = match io
                    .read_frame()
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Request>(&b).ok())
                {
                    Some(request) => daemon.handle(request),
                    None => refusal(),
                };
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
fn refusal() -> Response {
    Response {
        ok: false,
        mode: m8s_api::VAST_DEMO_MODE.into(),
        nodes: vec![],
        tiers: vec![],
        events: vec![],
        error: Some("request refused; empty fields are not absence".into()),
        secure_bootstrap: m8s_api::VAST_DEMO_SECURITY.into(),
    }
}
