//! Explicit synthetic OS qualification; ordinary tests never access Keychain.
#![cfg(all(target_os = "macos", feature = "os-keyring"))]

use m8s_secrets::{
    Error, KeyringDriver, KeyringIdentity, KeyringLockDomain, KeyringStore, MacKeyringDriver,
    SecretName, SecretStore, SecretValue,
};
use std::{
    fs::{DirBuilder, OpenOptions},
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::unix::{
        fs::{DirBuilderExt, MetadataExt},
        net::UnixStream,
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Barrier,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const SERVICE: &str = "m8s-secret-store-v1";
const PREFIX: &str = "m8s-synthetic-os-qualification-";
const TEST: &str = "os_keyring_qualification";
const COUNT: usize = 4;

fn name(s: &str) -> Result<SecretName, Error> {
    SecretName::new(s)
}
fn value() -> SecretValue {
    // Fixed synthetic bytes generated only in memory; never formatted or persisted
    // except through the exact synthetic OS item under qualification.
    SecretValue::new((0u8..32).map(|n| n.wrapping_mul(7)).collect())
}

struct Driver {
    mac: MacKeyringDriver,
    preflight: AtomicBool,
    failed: PathBuf,
    session: Session,
}
impl Driver {
    fn check(&self) -> Result<(), Error> {
        self.session.revalidate()?;
        if self.failed.try_exists()? {
            Err(Error::Backend)
        } else {
            Ok(())
        }
    }
    fn stop(&self) {
        // Non-secret stop signal, not a retry or an OS-store cleanup attempt.
        let _ = marker(&self.failed);
    }
}
impl KeyringDriver for Driver {
    fn identity(&self) -> Option<KeyringIdentity> {
        self.mac.identity()
    }
    fn load(&self) -> Result<Option<SecretValue>, Error> {
        // Invoked ONLY inside KeyringStore's canonical whole-operation lock.
        self.check()?;
        let loaded = match self.mac.load() {
            Ok(v) => v,
            Err(_) => {
                self.stop();
                return Err(Error::Backend);
            }
        };
        if self.preflight.load(Ordering::SeqCst) {
            if loaded.is_some() {
                self.stop();
                return Err(Error::Backend);
            }
            self.preflight.store(false, Ordering::SeqCst);
        }
        // Widen the whole-envelope race window without releasing coordination.
        std::thread::sleep(Duration::from_millis(15));
        Ok(loaded)
    }
    fn save(&self, v: &SecretValue) -> Result<(), Error> {
        self.check()?;
        self.mac.save(v).map_err(|_| {
            self.stop();
            Error::Backend
        })
    }
}
fn store(session: &Session, preflight: bool) -> Result<KeyringStore<Driver>, Error> {
    session.revalidate()?;
    let (dir, account) = (&session.dir, session.account.as_str());
    let identity = KeyringIdentity::new(SERVICE, name(account)?)?;
    let domain = KeyringLockDomain::acknowledge_exclusive_writers(dir, identity)?;
    KeyringStore::coordinated(
        Driver {
            mac: MacKeyringDriver::new(name(account)?),
            preflight: AtomicBool::new(preflight),
            failed: dir.join("failed"),
            session: session.clone(),
        },
        domain,
    )
}
fn write_records(session: &Session, writer: &str) -> Result<(), Error> {
    let mut s = store(session, false)?;
    for i in 0..COUNT {
        s.put(name(&format!("synthetic/{writer}/{i}"))?, value(), None)?;
    }
    Ok(())
}
fn await_file(path: &Path) -> Result<(), Error> {
    let start = Instant::now();
    while !path.try_exists()? {
        if start.elapsed() > Duration::from_secs(10) {
            return Err(Error::Unavailable);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
fn marker(path: &Path) -> Result<(), Error> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?
        .sync_all()?;
    Ok(())
}
const CONTROL_ENV: &str = "M8S_SYNTHETIC_CHILD_CONTROL";
const MAGIC: &[u8; 8] = b"M8SQIPC3";
const ACCOUNT_LEN: usize = PREFIX.len() + 32;
const PATH_LEN: usize = b"/private/tmp/".len() + ACCOUNT_LEN;
const FRAME_LEN: usize = 8 + 16 + 1 + 16 + ACCOUNT_LEN + PATH_LEN + 16;
const _: () = assert!(FRAME_LEN <= 1024);

#[derive(Clone)]
struct Session {
    id: [u8; 16], // Non-secret root-session identity, never a stored value.
    account: String,
    dir: PathBuf,
    device: u64,
    inode: u64,
}
struct RootSession(Session);
struct CreatedSession(Session); // Constructed ONLY after NoEntry + ownership save.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Scenario = 1,
    Writer0 = 2,
    Writer1 = 3,
    Verify = 4,
    Probe = 5,
}
impl Mode {
    fn parse(byte: u8) -> Result<Self, Error> {
        match byte {
            1 => Ok(Self::Scenario),
            2 => Ok(Self::Writer0),
            3 => Ok(Self::Writer1),
            4 => Ok(Self::Verify),
            5 => Ok(Self::Probe),
            _ => Err(Error::InvalidData),
        }
    }
}
fn random_id() -> Result<[u8; 16], Error> {
    let mut bytes = [0u8; 16];
    if unsafe { libc::getentropy(bytes.as_mut_ptr().cast(), bytes.len()) } != 0 {
        return Err(Error::Unavailable);
    }
    Ok(bytes)
}
fn account_for(id: &[u8; 16]) -> String {
    let suffix: String = id.iter().map(|b| format!("{b:02x}")).collect();
    format!("{PREFIX}{suffix}")
}
fn validate_scope(id: &[u8; 16], account: &str, dir: &Path) -> Result<(), Error> {
    // Equality with the derived ASCII representation enforces exactly 32 lower
    // hex digits, not merely a prefix or a normalized/aliased Path comparison.
    if account != account_for(id)
        || dir.as_os_str() != std::ffi::OsStr::new(&format!("/private/tmp/{account}"))
    {
        return Err(Error::InvalidData);
    }
    Ok(())
}
impl Session {
    fn revalidate(&self) -> Result<(), Error> {
        validate_scope(&self.id, &self.account, &self.dir)?;
        let m = std::fs::symlink_metadata(&self.dir)?;
        if !m.is_dir()
            || m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o7777 != 0o700
            || m.dev() != self.device
            || m.ino() != self.inode
        {
            return Err(Error::UnsafePath);
        }
        Ok(())
    }
    fn frame(&self, mode: Mode, challenge: &[u8; 16]) -> Result<[u8; FRAME_LEN], Error> {
        self.revalidate()?;
        let mut bytes = [0u8; FRAME_LEN];
        let parts: [&[u8]; 8] = [
            MAGIC,
            challenge,
            &[mode as u8],
            &self.id,
            self.account.as_bytes(),
            self.dir.as_os_str().as_encoded_bytes(),
            &self.device.to_le_bytes(),
            &self.inode.to_le_bytes(),
        ];
        let mut offset = 0;
        for part in parts {
            bytes[offset..offset + part.len()].copy_from_slice(part);
            offset += part.len();
        }
        Ok(bytes)
    }
    fn decode(bytes: &[u8; FRAME_LEN], challenge: &[u8; 16]) -> Result<(Self, Mode), Error> {
        if &bytes[..8] != MAGIC || &bytes[8..24] != challenge {
            return Err(Error::InvalidData);
        }
        let mode = Mode::parse(bytes[24])?;
        let id = bytes[25..41].try_into().map_err(|_| Error::InvalidData)?;
        let account =
            std::str::from_utf8(&bytes[41..41 + ACCOUNT_LEN]).map_err(|_| Error::InvalidData)?;
        let end = 41 + ACCOUNT_LEN + PATH_LEN;
        let dir = Path::new(
            std::str::from_utf8(&bytes[41 + ACCOUNT_LEN..end]).map_err(|_| Error::InvalidData)?,
        );
        validate_scope(&id, account, dir)?;
        let device = u64::from_le_bytes(
            bytes[end..end + 8]
                .try_into()
                .map_err(|_| Error::InvalidData)?,
        );
        let inode = u64::from_le_bytes(
            bytes[end + 8..]
                .try_into()
                .map_err(|_| Error::InvalidData)?,
        );
        Ok((
            Self {
                id,
                account: account.to_owned(),
                dir: dir.to_owned(),
                device,
                inode,
            },
            mode,
        ))
    }
}
impl RootSession {
    fn fresh() -> Result<Self, Error> {
        let id = random_id()?;
        let account = account_for(&id);
        let dir = PathBuf::from(format!("/private/tmp/{account}"));
        // Exclusive creation; no discovery, adoption, collision retry or cleanup.
        DirBuilder::new().mode(0o700).create(&dir)?;
        let m = std::fs::symlink_metadata(&dir)?;
        let session = Session {
            id,
            account,
            dir,
            device: m.dev(),
            inode: m.ino(),
        };
        session.revalidate()?;
        Ok(Self(session))
    }
    fn scenario(&self) -> Result<Children, Error> {
        spawn_bound(&self.0, Mode::Scenario, TEST, true)
    }
    fn probe(&self) -> Result<Children, Error> {
        spawn_bound(&self.0, Mode::Probe, "no_os_exec_control_handshake", false)
    }
}
impl CreatedSession {
    fn client(&self, mode: Mode) -> Result<Children, Error> {
        if !matches!(mode, Mode::Writer0 | Mode::Writer1 | Mode::Verify) {
            return Err(Error::InvalidData);
        }
        spawn_bound(&self.0, mode, TEST, false)
    }
}
fn entry_selection(legacy: [bool; 3], selector: Option<&std::ffi::OsStr>) -> Result<bool, Error> {
    if legacy.into_iter().any(|present| present) {
        return Err(Error::InvalidData);
    }
    match selector {
        None => Ok(false),
        Some(s) if s == "v3" => Ok(true),
        Some(_) => Err(Error::InvalidData),
    }
}
fn child_selected() -> Result<bool, Error> {
    entry_selection(
        [
            "M8S_SYNTHETIC_MODE",
            "M8S_SYNTHETIC_ACCOUNT",
            "M8S_SYNTHETIC_DOMAIN",
        ]
        .map(|s| std::env::var_os(s).is_some()),
        std::env::var_os(CONTROL_ENV).as_deref(),
    )
}
fn checked_control(fd: OwnedFd, expected_peer: libc::pid_t) -> Result<UnixStream, Error> {
    // Check the OWNED duplicate, not borrowed stdin; no double-close or unchecked
    // UnixStream-from-fd conversion. No Keychain/driver API is involved.
    let raw = fd.as_raw_fd();
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(raw, stat.as_mut_ptr()) } != 0 {
        return Err(Error::Io);
    }
    let stat = unsafe { stat.assume_init() };
    if stat.st_mode & libc::S_IFMT != libc::S_IFSOCK || stat.st_uid != unsafe { libc::geteuid() } {
        return Err(Error::InvalidData);
    }
    let mut kind: libc::c_int = 0;
    let mut length = std::mem::size_of_val(&kind) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            raw,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&mut kind as *mut libc::c_int).cast(),
            &mut length,
        )
    } != 0
        || length as usize != std::mem::size_of_val(&kind)
        || kind != libc::SOCK_STREAM
    {
        return Err(Error::InvalidData);
    }
    let mut address = std::mem::MaybeUninit::<libc::sockaddr_storage>::zeroed();
    let mut length = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    if unsafe { libc::getsockname(raw, address.as_mut_ptr().cast(), &mut length) } != 0 {
        return Err(Error::InvalidData);
    }
    if unsafe { address.assume_init() }.ss_family as libc::c_int != libc::AF_UNIX {
        return Err(Error::InvalidData);
    }
    let (mut uid, mut gid) = (0, 0);
    if unsafe { libc::getpeereid(raw, &mut uid, &mut gid) } != 0
        || uid != unsafe { libc::geteuid() }
    {
        return Err(Error::InvalidData);
    }
    let mut peer: libc::pid_t = 0;
    let mut length = std::mem::size_of_val(&peer) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            raw,
            0,
            libc::LOCAL_PEERPID,
            (&mut peer as *mut libc::pid_t).cast(),
            &mut length,
        )
    } != 0
        || length as usize != std::mem::size_of_val(&peer)
        || peer != expected_peer
    {
        return Err(Error::InvalidData);
    }
    let socket = UnixStream::from(fd);
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    Ok(socket)
}
// One monotonic two-second budget for the WHOLE handshake, not a timeout
// reset by each partial read/write. No retry on IPC error or interrupted call.
struct HandshakeBudget(Instant);
impl HandshakeBudget {
    fn new() -> Self {
        Self(Instant::now() + Duration::from_secs(2))
    }
    fn remaining(&self) -> Result<Duration, Error> {
        self.0
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or(Error::Unavailable)
    }
    fn read(&self, socket: &mut UnixStream, bytes: &mut [u8]) -> Result<(), Error> {
        let mut offset = 0;
        while offset < bytes.len() {
            socket.set_read_timeout(Some(self.remaining()?))?;
            let count = socket.read(&mut bytes[offset..])?;
            if count == 0 {
                return Err(Error::Io);
            }
            offset += count;
        }
        Ok(())
    }
    fn write(&self, socket: &mut UnixStream, bytes: &[u8]) -> Result<(), Error> {
        let mut offset = 0;
        while offset < bytes.len() {
            socket.set_write_timeout(Some(self.remaining()?))?;
            let count = socket.write(&bytes[offset..])?;
            if count == 0 {
                return Err(Error::Io);
            }
            offset += count;
        }
        Ok(())
    }
    fn end(&self, socket: &mut UnixStream) -> Result<(), Error> {
        socket.set_read_timeout(Some(self.remaining()?))?;
        if socket.read(&mut [0u8; 1])? != 0 {
            return Err(Error::InvalidData);
        }
        Ok(())
    }
}
fn receive_bound() -> Result<(Session, Mode), Error> {
    // Duplicate first; only a successful new descriptor is taken into OwnedFd.
    let raw = unsafe { libc::fcntl(0, libc::F_DUPFD_CLOEXEC, 3) };
    if raw < 0 {
        return Err(Error::Unavailable);
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut socket = checked_control(fd, unsafe { libc::getppid() })?;
    let budget = HandshakeBudget::new();
    let challenge = random_id()?;
    budget.write(&mut socket, &challenge)?;
    let mut frame = [0u8; FRAME_LEN]; // Fixed (<1 KiB), no peer-directed allocation.
    budget.read(&mut socket, &mut frame)?;
    budget.end(&mut socket)?; // Reject trailing bytes; parent half-closes response.
    let (session, mode) = Session::decode(&frame, &challenge)?;
    session.revalidate()?;
    budget.write(&mut socket, MAGIC)?; // ACK only after full metadata/directory validation.
    Ok((session, mode))
}
fn open_control(test: &str, group: bool) -> Result<(Children, UnixStream), Error> {
    let (parent, inherited) = UnixStream::pair()?;
    parent.set_read_timeout(Some(Duration::from_secs(2)))?;
    parent.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut command = Command::new(std::env::current_exe()?);
    command.args(["--exact", test, "--test-threads=1"]);
    if test == TEST {
        command.arg("--ignored");
    }
    command
        .env_clear()
        .env(CONTROL_ENV, "v3")
        .stdin(Stdio::from(OwnedFd::from(inherited)))
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if group {
        command.process_group(0);
    }
    // RAII ownership is established immediately after spawn, BEFORE handshake.
    // Any handshake failure kills/reaps ONLY the owned direct child, never PGID.
    let child = command.spawn()?;
    Ok((Children(vec![child]), parent))
}
fn spawn_bound(session: &Session, mode: Mode, test: &str, group: bool) -> Result<Children, Error> {
    session.revalidate()?;
    let (children, mut parent) = open_control(test, group)?;
    let budget = HandshakeBudget::new();
    let mut challenge = [0u8; 16];
    budget.read(&mut parent, &mut challenge)?;
    budget.write(&mut parent, &session.frame(mode, &challenge)?)?;
    parent.shutdown(std::net::Shutdown::Write)?;
    let mut ack = [0u8; 8];
    budget.read(&mut parent, &mut ack)?;
    if &ack != MAGIC {
        return Err(Error::InvalidData);
    }
    Ok(children)
}
struct Children(Vec<Child>);
impl Drop for Children {
    fn drop(&mut self) {
        for c in &mut self.0 {
            let _ = c.kill();
        }
        for c in &mut self.0 {
            let _ = c.wait();
        }
    }
}
fn wait_all(children: &mut Children, budget: Duration) -> Result<(), Error> {
    let start = Instant::now();
    loop {
        let mut done = true;
        for c in &mut children.0 {
            match c.try_wait()? {
                Some(status) if status.success() => {}
                Some(_) => return Err(Error::Backend),
                None => done = false,
            }
        }
        if done {
            return Ok(());
        }
        if start.elapsed() >= budget {
            return Err(Error::Unavailable);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
// Single-child watchdog: ONLY the last successful poll returning None permits
// group signalling. No poll/wait/reap occurs between that observation and signal.
// Exited children may have been reaped by try_wait: their old PGID is NOT owned.
fn scenario_watchdog(
    mut poll: impl FnMut() -> std::io::Result<Option<bool>>,
    mut elapsed: impl FnMut() -> Duration,
    budget: Duration,
    mut signal_owned_group: impl FnMut() -> Result<(), Error>,
    mut pause: impl FnMut(),
) -> Result<(), Error> {
    loop {
        match poll() {
            Ok(Some(true)) => return Ok(()),
            Ok(Some(false)) => return Err(Error::Backend),
            Err(_) => return Err(Error::Io),
            Ok(None) => {
                if elapsed() >= budget {
                    // Leader remains our unreaped child, even if it exits now;
                    // its numeric PID/PGID cannot be reused before the final wait.
                    signal_owned_group()?;
                    return Err(Error::Unavailable);
                }
            }
        }
        pause();
    }
}

#[test]
fn watchdog_reaped_exit_and_io_error_never_signal_group() {
    for (observation, expected) in [
        (Ok(Some(true)), Ok(())),
        (Ok(Some(false)), Err(Error::Backend)),
        (Err(std::io::Error::other("synthetic")), Err(Error::Io)),
    ] {
        let mut observation = Some(observation);
        let result = scenario_watchdog(
            || observation.take().expect("one poll only"),
            || panic!("exit/error must not consult deadline"),
            Duration::ZERO,
            || panic!("reaped/error PGID is not owned"),
            || panic!("exit/error must not pause"),
        );
        assert_eq!(result, expected);
    }
}

#[test]
fn watchdog_timeout_signals_only_after_unreaped_observation() {
    let events = std::cell::RefCell::new(Vec::new());
    let result = scenario_watchdog(
        || {
            events.borrow_mut().push("unreaped");
            Ok(None)
        },
        || {
            events.borrow_mut().push("deadline");
            Duration::from_secs(90)
        },
        Duration::from_secs(90),
        || {
            events.borrow_mut().push("signal");
            Ok(())
        },
        || panic!("no intervening pause or reap"),
    );
    assert_eq!(result, Err(Error::Unavailable));
    assert_eq!(*events.borrow(), ["unreaped", "deadline", "signal"]);
}

#[test]
fn watchdog_exit_after_pending_poll_does_not_signal_old_group() {
    let mut polls = [Ok(None), Ok(Some(false))].into_iter();
    let mut pauses = 0;
    let result = scenario_watchdog(
        || polls.next().expect("two polls only"),
        || Duration::ZERO,
        Duration::from_secs(90),
        || panic!("nonzero exit is not a watchdog timeout"),
        || pauses += 1,
    );
    assert_eq!(result, Err(Error::Backend));
    assert_eq!(pauses, 1);
}

fn verify_records(session: &Session) -> Result<(), Error> {
    let s = store(session, false)?;
    if s.list(0)?.len() != 1 + 4 * COUNT {
        return Err(Error::Backend);
    }
    let ownership = s
        .get(&name("synthetic/ownership")?, 0)?
        .ok_or(Error::Backend)?;
    if ownership.expose() != value().expose() {
        return Err(Error::Backend);
    }
    for kind in ["thread", "process"] {
        for writer in 0..2 {
            for i in 0..COUNT {
                let n = name(&format!("synthetic/{kind}-{writer}/{i}"))?;
                let loaded = s.get(&n, 0)?.ok_or(Error::Backend)?;
                if loaded.expose() != value().expose() {
                    return Err(Error::Backend);
                }
            }
        }
    }
    Ok(())
}
fn scenario(session: &Session) -> Result<(), Error> {
    let dir = &session.dir;
    // Exact initial NoEntry, NOT an empty list/envelope, is required before any
    // write. Mac load can fetch Some bytes; reject/zeroize before decode/save/delete.
    let mut s = store(session, true)?;
    marker(&dir.join("stage-preflight-start"))?;
    if !s.list(0)?.is_empty() {
        return Err(Error::Backend);
    }
    marker(&dir.join("stage-no-entry"))?;
    s.put(name("synthetic/ownership")?, value(), None)?;
    marker(&dir.join("stage-created"))?;
    let created = CreatedSession(session.clone());
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|i| {
            let (session, barrier) = (created.0.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                write_records(&session, &format!("thread-{i}"))
            })
        })
        .collect();
    let mut success = true;
    for t in threads {
        success &= matches!(t.join(), Ok(Ok(())));
    }
    if !success {
        return Err(Error::Backend);
    }

    marker(&dir.join("stage-threads-joined"))?;
    let mut children = Children(Vec::new());
    for mode in [Mode::Writer0, Mode::Writer1] {
        let mut bound = created.client(mode)?;
        children.0.append(&mut bound.0);
    }
    for i in 0..2 {
        await_file(&dir.join(format!("ready-{i}")))?;
    }
    marker(&dir.join("go"))?;
    wait_all(&mut children, Duration::from_secs(30))?;
    drop(children); // All writers have exited BEFORE verification or cleanup.
    marker(&dir.join("stage-processes-reaped"))?;
    drop(s);
    // A newly exec'd read-only client verifies after every writer/store dropped.
    // This is process-restart persistence, not only reconstruction in the writer.
    let mut restart = created.client(Mode::Verify)?;
    wait_all(&mut restart, Duration::from_secs(15))?;
    drop(restart);
    marker(&dir.join("stage-restart-verified"))?;
    let mut s = store(session, false)?;
    verify_records(session)?;
    s.put(name("synthetic/expiry")?, value(), Some(10))?;
    if s.get(&name("synthetic/expiry")?, 9)?.is_none()
        || s.get(&name("synthetic/expiry")?, 10)?.is_some()
        || s.list(9)?.len() != 2 + 4 * COUNT
        || s.list(10)?.len() != 1 + 4 * COUNT
    {
        return Err(Error::Backend);
    }
    marker(&dir.join("stage-verified"))?;
    // Only exact records this run created; no item deletion API or enumeration.
    for kind in ["thread", "process"] {
        for writer in 0..2 {
            for i in 0..COUNT {
                if !s.delete(&name(&format!("synthetic/{kind}-{writer}/{i}"))?)? {
                    return Err(Error::Backend);
                }
            }
        }
    }
    for n in ["synthetic/expiry", "synthetic/ownership"] {
        if !s.delete(&name(n)?)? {
            return Err(Error::Backend);
        }
    }
    if s.delete(&name("synthetic/ownership")?)? {
        return Err(Error::Backend);
    }
    drop(s);
    if !store(session, false)?.list(0)?.is_empty() {
        return Err(Error::Backend);
    }
    marker(&dir.join("stage-empty-envelope"))?;
    Ok(()) // Empty synthetic OS envelope remains; NOT zero OS items.
}

#[test]
#[ignore = "actual synthetic macOS Keychain access; frozen-source review required first"]
fn os_keyring_qualification() -> Result<(), Error> {
    if child_selected()? {
        let (session, mode) = receive_bound()?;
        match mode {
            Mode::Scenario => scenario(&session),
            Mode::Verify => verify_records(&session),
            Mode::Writer0 | Mode::Writer1 => {
                let i = if mode == Mode::Writer0 { 0 } else { 1 };
                drop(store(&session, false)?); // Validates only; no OS call.
                marker(&session.dir.join(format!("ready-{i}")))?;
                await_file(&session.dir.join("go"))?;
                write_records(&session, &format!("process-{i}"))
            }
            Mode::Probe => Err(Error::InvalidData),
        }
    } else {
        let root = RootSession::fresh()?;
        drop(store(&root.0, false)?); // Domain validation only, before OS.
        eprintln!(
            "synthetic qualification account/domain: {} / {}",
            root.0.account,
            root.0.dir.display()
        );
        let mut children = root.scenario()?;
        let group = children.0[0].id();
        let start = Instant::now();
        let result = scenario_watchdog(
            || {
                children.0[0]
                    .try_wait()
                    .map(|status| status.map(|s| s.success()))
            },
            || start.elapsed(),
            Duration::from_secs(90),
            || {
                // Only a true timeout with an unreaped owned leader reaches
                // this closure; final Child wait occurs later in RAII Drop.
                if unsafe { libc::kill(-(group as i32), libc::SIGKILL) } == 0 {
                    Ok(())
                } else {
                    Err(Error::Io)
                }
            },
            || std::thread::sleep(Duration::from_millis(10)),
        );
        if result.is_err() {
            eprintln!(
                "synthetic scenario failed; descendant termination and OS effects may be uncertain; no cleanup attempted"
            );
        }
        result
    }
}

// This normal test has NO path to scenario/store, regardless of IPC/env input.
#[test]
fn no_os_exec_control_handshake() -> Result<(), Error> {
    if child_selected()? {
        let (_, mode) = receive_bound()?;
        return if mode == Mode::Probe {
            Ok(())
        } else {
            Err(Error::InvalidData)
        };
    }
    let root = RootSession::fresh()?;
    let result = (|| {
        let mut children = root.probe()?;
        wait_all(&mut children, Duration::from_secs(5))?;
        drop(children);
        // Real exec negative routes: stale response, malformed header, wrong mode,
        // EOF, and missing control socket. None can ever call a store.
        for offset in [0, 8, 25, 41, 41 + ACCOUNT_LEN, FRAME_LEN - 1] {
            let (mut children, mut parent) = open_control("no_os_exec_control_handshake", false)?;
            let mut challenge = [0u8; 16];
            parent.read_exact(&mut challenge)?;
            let mut frame = root.0.frame(Mode::Probe, &challenge)?;
            frame[offset] ^= 0xff;
            parent.write_all(&frame)?;
            drop(parent);
            assert_eq!(
                wait_all(&mut children, Duration::from_secs(5)),
                Err(Error::Backend)
            );
        }
        let mut children = spawn_bound(
            &root.0,
            Mode::Scenario,
            "no_os_exec_control_handshake",
            false,
        )?;
        assert_eq!(
            wait_all(&mut children, Duration::from_secs(5)),
            Err(Error::Backend)
        );
        drop(children);
        let (mut children, parent) = open_control("no_os_exec_control_handshake", false)?;
        drop(parent); // EOF, no reply/session.
        assert_eq!(
            wait_all(&mut children, Duration::from_secs(5)),
            Err(Error::Backend)
        );
        drop(children);
        let child = Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "no_os_exec_control_handshake",
                "--test-threads=1",
            ])
            .env_clear()
            .env(CONTROL_ENV, "v3")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let mut children = Children(vec![child]);
        assert_eq!(
            wait_all(&mut children, Duration::from_secs(5)),
            Err(Error::Backend)
        );
        Ok(())
    })();
    root.0.revalidate()?;
    // No domain/lock/store was ever constructed by these control-only processes.
    // Remove ONLY this fresh owned empty no-OS fixture after every client reaped.
    std::fs::remove_dir(&root.0.dir)?;
    result
}

#[test]
fn pure_scope_and_external_selection_reject_forged_metadata() -> Result<(), Error> {
    let id = [0u8; 16];
    let account = account_for(&id);
    let exact = format!("/private/tmp/{account}");
    validate_scope(&id, &account, Path::new(&exact))?;
    for invalid in [
        account[..account.len() - 1].to_owned(),
        format!("{account}0"),
        format!("{PREFIX}{}g", "0".repeat(31)),
        format!("{PREFIX}{}A", "0".repeat(31)),
    ] {
        assert_eq!(
            validate_scope(&id, &invalid, Path::new(&format!("/private/tmp/{invalid}"))),
            Err(Error::InvalidData)
        );
    }
    for invalid in [
        format!("/tmp/{account}"),
        format!("/private/tmp/./{account}"),
        format!("/private/tmp/{account}/"),
        format!("/private/tmp/other/../{account}"),
        format!("/private/tmp//{account}"),
    ] {
        assert_eq!(
            validate_scope(&id, &account, Path::new(&invalid)),
            Err(Error::InvalidData)
        );
    }
    for i in 0..3 {
        let mut legacy = [false; 3];
        legacy[i] = true;
        assert_eq!(entry_selection(legacy, None), Err(Error::InvalidData));
        assert_eq!(
            entry_selection(legacy, Some(std::ffi::OsStr::new("v3"))),
            Err(Error::InvalidData)
        );
    }
    assert_eq!(
        entry_selection([false; 3], Some(std::ffi::OsStr::new("scenario"))),
        Err(Error::InvalidData)
    );
    // A zero/stale/missing protocol/session cannot be accepted as current challenge.
    assert!(Session::decode(&[0u8; FRAME_LEN], &[1u8; 16]).is_err());
    assert!(Mode::parse(0).is_err());
    assert!(Mode::parse(255).is_err());
    Ok(())
}

#[test]
fn control_descriptor_type_and_peer_checks_are_fail_closed() -> Result<(), Error> {
    let file = tempfile::tempfile()?;
    assert!(checked_control(OwnedFd::from(file), unsafe { libc::getpid() }).is_err());
    let (socket, _other) = UnixStream::pair()?;
    assert!(checked_control(OwnedFd::from(socket), unsafe { libc::getpid() } + 1).is_err());
    Ok(())
}
