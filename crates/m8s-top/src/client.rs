//! Strict v1 status reader with one absolute connect/write/read deadline.
use crate::{NodeStatus, StatusSnapshot, display_text};
use m8s_api::{MAX_FRAME, Request};
use m8s_core::{Phase, Timestamp};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use serde::{Deserialize, Deserializer};
use socket2::{Domain, SockAddr, Socket, Type};
use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::{
    fs::{FileTypeExt, MetadataExt},
    net::UnixStream,
};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

/// Fixed provenance only: never stores paths, addresses or an io::Error payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoStage {
    WorkingDirectory,
    PathMetadata,
    SocketCreate,
    Address,
    Connect,
    Nonblocking,
    Poll,
    WriteOption,
    Write,
    ReadOption,
    Read,
}
impl IoStage {
    fn label(self) -> &'static str {
        match self {
            Self::WorkingDirectory => "working-directory",
            Self::PathMetadata => "path-metadata",
            Self::SocketCreate => "socket-create",
            Self::Address => "address",
            Self::Connect => "connect",
            Self::Nonblocking => "nonblocking",
            Self::Poll => "poll",
            Self::WriteOption => "write-option",
            Self::Write => "write",
            Self::ReadOption => "read-option",
            Self::Read => "read",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoKind {
    NotFound,
    PermissionDenied,
    ConnectionRefused,
    ConnectionReset,
    ConnectionAborted,
    NotConnected,
    AddressInUse,
    AddressNotAvailable,
    BrokenPipe,
    WriteZero,
    UnexpectedEof,
    Other,
}
impl IoKind {
    fn label(self) -> &'static str {
        match self {
            Self::NotFound => "not-found",
            Self::PermissionDenied => "permission-denied",
            Self::ConnectionRefused => "connection-refused",
            Self::ConnectionReset => "connection-reset",
            Self::ConnectionAborted => "connection-aborted",
            Self::NotConnected => "not-connected",
            Self::AddressInUse => "address-in-use",
            Self::AddressNotAvailable => "address-not-available",
            Self::BrokenPipe => "broken-pipe",
            Self::WriteZero => "write-zero",
            Self::UnexpectedEof => "unexpected-eof",
            Self::Other => "other",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadError {
    InvalidTimeout,
    Unavailable,
    Io {
        stage: IoStage,
        kind: IoKind,
        os_code: Option<i32>,
    },
    UnsafeSocket,
    Timeout,
    InvalidResponse,
    Rejected,
}
impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidTimeout => f.write_str("invalid status timeout"),
            Self::Unavailable => f.write_str("status socket unavailable"),
            Self::UnsafeSocket => f.write_str("status socket trust checks failed"),
            Self::Timeout => f.write_str("status request timed out"),
            Self::InvalidResponse => f.write_str("invalid status response"),
            Self::Rejected => f.write_str("status request rejected"),
            Self::Io {
                stage,
                kind,
                os_code,
            } => {
                write!(
                    f,
                    "status IO failure: stage={} kind={} os_code=",
                    stage.label(),
                    kind.label()
                )?;
                match os_code {
                    Some(code) => write!(f, "{code}"),
                    None => f.write_str("none"),
                }
            }
        }
    }
}
impl std::error::Error for ReadError {}
fn io_error(stage: IoStage, error: io::Error) -> ReadError {
    let kind = match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => return ReadError::Timeout,
        io::ErrorKind::NotFound => IoKind::NotFound,
        io::ErrorKind::PermissionDenied => IoKind::PermissionDenied,
        io::ErrorKind::ConnectionRefused => IoKind::ConnectionRefused,
        io::ErrorKind::ConnectionReset => IoKind::ConnectionReset,
        io::ErrorKind::ConnectionAborted => IoKind::ConnectionAborted,
        io::ErrorKind::NotConnected => IoKind::NotConnected,
        io::ErrorKind::AddrInUse => IoKind::AddressInUse,
        io::ErrorKind::AddrNotAvailable => IoKind::AddressNotAvailable,
        io::ErrorKind::BrokenPipe => IoKind::BrokenPipe,
        io::ErrorKind::WriteZero => IoKind::WriteZero,
        io::ErrorKind::UnexpectedEof => IoKind::UnexpectedEof,
        _ => IoKind::Other,
    };
    ReadError::Io {
        stage,
        kind,
        os_code: error.raw_os_error(),
    }
}
fn remaining(deadline: Instant) -> Result<Duration, ReadError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or(ReadError::Timeout)
}
pub struct StatusClient {
    timeout: Duration,
}
impl StatusClient {
    pub fn new(timeout: Duration) -> Result<Self, ReadError> {
        if timeout.is_zero() || timeout > Duration::from_secs(10) {
            return Err(ReadError::InvalidTimeout);
        }
        Ok(Self { timeout })
    }
    pub fn status(&self, path: &Path) -> Result<StatusSnapshot, ReadError> {
        let deadline = Instant::now() + self.timeout;
        let mut stream = checked_connect(path, deadline)?;
        read_status(&mut stream, deadline)
    }
    /// Two separately checked connections share ONE absolute timeout budget.
    /// Status failure fails the operation; config failure never erases status.
    pub fn status_with_config(&self, path: &Path) -> Result<StatusSnapshot, ReadError> {
        let deadline = Instant::now() + self.timeout;
        let mut stream = checked_connect(path, deadline)?;
        let mut snapshot = read_status(&mut stream, deadline)?;
        drop(stream);
        let config = (|| {
            let mut stream = checked_connect(path, deadline)?;
            let frame = exchange(&mut stream, deadline, &Request::ConfigStatus {})?;
            let response =
                m8s_api::decode_config_status(&frame).map_err(|_| ReadError::InvalidResponse)?;
            if !response.ok {
                return Err(ReadError::Rejected);
            }
            Ok(crate::ConfigObservation::Available {
                configured: response.configured,
                tiers: response.tiers,
                atomic_with_status: false,
            })
        })()
        .unwrap_or(crate::ConfigObservation::Unknown {
            atomic_with_status: false,
        });
        snapshot.config_observation = Some(config);
        Ok(snapshot)
    }
}
fn checked_connect(path: &Path, deadline: Instant) -> Result<UnixStream, ReadError> {
    let uid = nix::unistd::geteuid().as_raw();
    let checked = validate_socket_path(path, uid)?;
    let socket = Socket::new(Domain::UNIX, Type::STREAM, None)
        .map_err(|e| io_error(IoStage::SocketCreate, e))?;
    let address = SockAddr::unix(&checked.path).map_err(|e| io_error(IoStage::Address, e))?;
    socket
        .connect_timeout(&address, remaining(deadline)?)
        .map_err(|e| io_error(IoStage::Connect, e))?;
    verify_peer_uid(&socket, uid)?;
    let after = validate_socket_path(&checked.path, uid)?;
    if checked != after {
        return Err(ReadError::UnsafeSocket);
    }
    remaining(deadline)?;
    socket
        .set_nonblocking(true)
        .map_err(|e| io_error(IoStage::Nonblocking, e))?;
    let descriptor: std::os::fd::OwnedFd = socket.into();
    let stream = UnixStream::from(descriptor);
    Ok(stream)
}

#[derive(PartialEq, Eq)]
struct CheckedSocket {
    path: PathBuf,
    parent: (u64, u64, u32, u32),
    socket: (u64, u64, u32, u32),
}
fn fingerprint(meta: &std::fs::Metadata) -> (u64, u64, u32, u32) {
    (meta.dev(), meta.ino(), meta.uid(), meta.mode())
}
fn validate_socket_path(path: &Path, uid: u32) -> Result<CheckedSocket, ReadError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| io_error(IoStage::WorkingDirectory, e))?
            .join(path)
    };
    let mut clean = PathBuf::new();
    let mut depth = 0;
    // Do not canonicalize user input: following a symlink and then checking its
    // target would silently weaken the no-symlink policy. Resolve normal path
    // components lexically and inspect every component with lstat instead.
    for component in absolute.components() {
        match component {
            Component::RootDir => clean.push(component.as_os_str()),
            Component::Normal(name) => {
                depth += 1;
                if depth > 128 {
                    return Err(ReadError::UnsafeSocket);
                }
                clean.push(name);
                let meta = std::fs::symlink_metadata(&clean)
                    .map_err(|e| io_error(IoStage::PathMetadata, e))?;
                if meta.file_type().is_symlink() {
                    return Err(ReadError::UnsafeSocket);
                }
                // An untrusted ancestor owner or a non-sticky writable directory
                // could swap path entries even though the final parent is 0700.
                // Root/current-UID sticky roots such as /tmp are allowed: each
                // following entry must itself pass the ownership checks.
                if meta.is_dir()
                    && ((meta.uid() != 0 && meta.uid() != uid)
                        || (meta.mode() & 0o022 != 0 && meta.mode() & 0o1000 == 0))
                {
                    return Err(ReadError::UnsafeSocket);
                }
            }
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) => return Err(ReadError::UnsafeSocket),
        }
    }
    let parent = clean.parent().ok_or(ReadError::UnsafeSocket)?;
    let dir = std::fs::symlink_metadata(parent).map_err(|e| io_error(IoStage::PathMetadata, e))?;
    let socket =
        std::fs::symlink_metadata(&clean).map_err(|e| io_error(IoStage::PathMetadata, e))?;
    if !dir.is_dir()
        || dir.file_type().is_symlink()
        || dir.uid() != uid
        || dir.mode() & 0o7777 != 0o700
        || !socket.file_type().is_socket()
        || socket.uid() != uid
        || socket.mode() & 0o7777 != 0o600
    {
        return Err(ReadError::UnsafeSocket);
    }
    Ok(CheckedSocket {
        path: clean,
        parent: fingerprint(&dir),
        socket: fingerprint(&socket),
    })
}
#[cfg(any(target_os = "linux", target_os = "android"))]
fn verify_peer_uid(socket: &Socket, uid: u32) -> Result<(), ReadError> {
    let peer = nix::sys::socket::getsockopt(socket, nix::sys::socket::sockopt::PeerCredentials)
        .map_err(|_| ReadError::UnsafeSocket)?;
    if peer.uid() != uid {
        return Err(ReadError::UnsafeSocket);
    }
    Ok(())
}
#[cfg(any(
    target_vendor = "apple",
    target_os = "freebsd",
    target_os = "dragonfly"
))]
fn verify_peer_uid(socket: &Socket, uid: u32) -> Result<(), ReadError> {
    let peer = nix::sys::socket::getsockopt(socket, nix::sys::socket::sockopt::LocalPeerCred)
        .map_err(|_| ReadError::UnsafeSocket)?;
    if peer.uid() != uid {
        return Err(ReadError::UnsafeSocket);
    }
    Ok(())
}
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    target_os = "freebsd",
    target_os = "dragonfly"
)))]
fn verify_peer_uid(_: &Socket, _: u32) -> Result<(), ReadError> {
    Err(ReadError::UnsafeSocket) // Unsupported credential paths fail closed.
}

fn poll_failure() -> ReadError {
    ReadError::Io {
        stage: IoStage::Poll,
        kind: IoKind::Other,
        os_code: None,
    }
}
fn poll_timeout(duration: Duration) -> Result<PollTimeout, ReadError> {
    // nix's Duration conversion truncates; round UP to avoid zero-ms spinning.
    // Deadline is rechecked after poll, so rounding cannot authorize late I/O.
    let millis = duration
        .as_nanos()
        .checked_add(999_999)
        .ok_or_else(poll_failure)?
        / 1_000_000;
    PollTimeout::try_from(millis).map_err(|_| poll_failure())
}
fn poll_events(
    events: Option<PollFlags>,
    interest: PollFlags,
) -> Result<Option<PollFlags>, ReadError> {
    let events = events.ok_or_else(poll_failure)?;
    if events.contains(PollFlags::POLLNVAL) {
        return Err(poll_failure());
    }
    Ok(events
        .intersects(interest | PollFlags::POLLHUP | PollFlags::POLLERR)
        .then_some(events))
}
fn wait_ready(
    stream: &UnixStream,
    interest: PollFlags,
    deadline: Instant,
) -> Result<PollFlags, ReadError> {
    loop {
        let timeout = poll_timeout(remaining(deadline)?)?;
        let mut fds = [PollFd::new(stream.as_fd(), interest)];
        match poll(&mut fds, timeout) {
            Ok(_) => {
                remaining(deadline)?;
                if let Some(events) = poll_events(fds[0].revents(), interest)? {
                    return Ok(events);
                }
            }
            Err(nix::errno::Errno::EINTR) => continue,
            Err(error) => {
                return Err(io_error(
                    IoStage::Poll,
                    io::Error::from_raw_os_error(error as i32),
                ));
            }
        }
    }
}
fn read_status(stream: &mut UnixStream, deadline: Instant) -> Result<StatusSnapshot, ReadError> {
    decode_status(&exchange(stream, deadline, &Request::Status {})?)
}
fn exchange(
    stream: &mut UnixStream,
    deadline: Instant,
    request: &Request,
) -> Result<Vec<u8>, ReadError> {
    let mut request = serde_json::to_vec(request).map_err(|_| ReadError::InvalidResponse)?;
    request.push(b'\n');
    let mut offset = 0;
    while offset < request.len() {
        let events = wait_ready(stream, PollFlags::POLLOUT, deadline)?;
        match stream.write(&request[offset..]) {
            Ok(0) => {
                return Err(ReadError::Io {
                    stage: IoStage::Write,
                    kind: IoKind::WriteZero,
                    os_code: None,
                });
            }
            Ok(n) => offset += n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if events.intersects(PollFlags::POLLHUP | PollFlags::POLLERR) {
                    return Err(io_error(
                        IoStage::Write,
                        io::Error::other("terminal readiness without progress"),
                    ));
                }
                continue;
            }
            Err(error) => return Err(io_error(IoStage::Write, error)),
        }
    }
    let mut frame = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let events = wait_ready(stream, PollFlags::POLLIN, deadline)?;
        let count = match stream.read(&mut chunk) {
            Ok(0) => return Err(ReadError::InvalidResponse),
            Ok(n) => n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if events.intersects(PollFlags::POLLHUP | PollFlags::POLLERR) {
                    return Err(io_error(
                        IoStage::Read,
                        io::Error::other("terminal readiness without progress"),
                    ));
                }
                continue;
            }
            Err(error) => return Err(io_error(IoStage::Read, error)),
        };
        frame.extend_from_slice(&chunk[..count]);
        if frame.len() > MAX_FRAME {
            return Err(ReadError::InvalidResponse);
        }
        if let Some(newline) = frame.iter().position(|b| *b == b'\n') {
            if newline + 1 != frame.len() {
                return Err(ReadError::InvalidResponse);
            }
            remaining(deadline)?;
            return Ok(frame);
        }
    }
}
// Mirrors the existing m8s-api v1 envelope, but rejects duplicate/unknown fields
// rather than silently accepting a different schema. No new API version invented.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireStatus {
    ok: bool,
    mode: String,
    nodes: Vec<WireNode>,
    tiers: Vec<(String, u32)>,
    events: Vec<String>,
    #[serde(deserialize_with = "required_option")]
    error: Option<String>,
    secure_bootstrap: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireNode {
    uid: String,
    tier: String,
    owner: String,
    #[serde(deserialize_with = "required_option")]
    instance_id: Option<String>,
    phase: Phase,
    created_at: Timestamp,
}
fn required_option<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::deserialize(d)
}
fn bounded(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max
}

pub fn decode_status(frame: &[u8]) -> Result<StatusSnapshot, ReadError> {
    if frame.len() > MAX_FRAME
        || !frame.ends_with(b"\n")
        || frame[..frame.len().saturating_sub(1)].contains(&b'\n')
    {
        return Err(ReadError::InvalidResponse);
    }
    let raw: WireStatus = serde_json::from_slice(frame).map_err(|_| ReadError::InvalidResponse)?;
    if !bounded(&raw.mode, 64)
        || !bounded(&raw.secure_bootstrap, 4096)
        || raw.nodes.len() > 256
        || raw.tiers.len() > 256
        || raw.events.len() > 1024
        || raw.events.iter().any(|e| e.len() > 4096)
        || raw.error.as_ref().is_some_and(|e| e.len() > 4096)
        || raw.nodes.iter().any(|n| {
            !bounded(&n.uid, 256)
                || !bounded(&n.tier, 256)
                || !bounded(&n.owner, 256)
                || n.instance_id.as_ref().is_some_and(|id| !bounded(id, 256))
        })
        || raw.tiers.iter().any(|(name, _)| !bounded(name, 256))
    {
        return Err(ReadError::InvalidResponse);
    }
    if !raw.ok {
        return Err(ReadError::Rejected);
    }
    if raw.error.is_some() {
        return Err(ReadError::InvalidResponse);
    }
    let mut seen = std::collections::HashSet::new();
    if raw.nodes.iter().any(|n| !seen.insert(n.uid.as_str())) {
        return Err(ReadError::InvalidResponse);
    }
    let mut pools = std::collections::HashSet::new();
    if raw
        .tiers
        .iter()
        .any(|(name, _)| !pools.insert(name.as_str()))
    {
        return Err(ReadError::InvalidResponse);
    }
    let mut nodes: Vec<_> = raw
        .nodes
        .into_iter()
        .map(|n| {
            // created_at is NOT last_observation. Read it for schema validation only.
            let _created_at = n.created_at;
            NodeStatus {
                uid: display_text(&n.uid),
                pool: display_text(&n.tier),
                reported_phase: n.phase,
                instance_id: n.instance_id.as_deref().map(display_text),
                last_observation: None,
                model_ready: None,
                destroy_pending: matches!(n.phase, Phase::Destroying | Phase::Draining)
                    .then_some(true),
            }
        })
        .collect();
    nodes.sort_by(|a, b| a.uid.cmp(&b.uid));
    Ok(StatusSnapshot {
        schema_version: 1,
        reported_mode: display_text(&raw.mode),
        pools: raw
            .tiers
            .into_iter()
            .map(|(name, desired)| (display_text(&name), desired))
            .collect(),
        nodes,
        bootstrap_verified: None,
        heartbeat_age_seconds: None,
        hourly_micro_usd: None,
        config_observation: None,
    })
}

#[cfg(test)]
mod path_tests {
    use super::*;
    use std::os::unix::{fs::PermissionsExt, net::UnixListener};
    #[test]
    fn wrong_owner_policy_rejects_without_chown_or_foreign_uid_access() {
        let dir = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let path = dir.path().canonicalize().unwrap().join("status.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let uid = nix::unistd::geteuid().as_raw();
        assert!(validate_socket_path(&path, uid).is_ok());
        // Inject a synthetic expected owner, never chown or inspect another user.
        assert!(matches!(
            validate_socket_path(&path, uid.wrapping_add(1)),
            Err(ReadError::UnsafeSocket)
        ));
    }
    #[test]
    fn peer_owner_and_replaced_path_checks_use_synthetic_endpoints_only() {
        let (stream, _peer) = UnixStream::pair().unwrap();
        let descriptor: std::os::fd::OwnedFd = stream.into();
        let socket = Socket::from(descriptor);
        let uid = nix::unistd::geteuid().as_raw();
        assert!(verify_peer_uid(&socket, uid).is_ok());
        assert_eq!(
            verify_peer_uid(&socket, uid.wrapping_add(1)),
            Err(ReadError::UnsafeSocket)
        );
        let dir = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let path = dir.path().canonicalize().unwrap().join("status.sock");
        let _old = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let before = validate_socket_path(&path, uid).unwrap();
        std::fs::remove_file(&path).unwrap();
        let _new = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let after = validate_socket_path(&path, uid).unwrap();
        assert!(before != after);
    }
}

#[cfg(test)]
mod io_diagnostic_tests {
    use super::*;
    #[test]
    fn numeric_os_errors_and_static_stage_kind_are_the_only_diagnostics() {
        let stages = [
            IoStage::WorkingDirectory,
            IoStage::PathMetadata,
            IoStage::SocketCreate,
            IoStage::Address,
            IoStage::Connect,
            IoStage::WriteOption,
            IoStage::Write,
            IoStage::ReadOption,
            IoStage::Read,
        ];
        for stage in stages {
            for code in [2, 13, 22, 61, 111, -12345] {
                let error = io_error(stage, io::Error::from_raw_os_error(code));
                assert!(
                    matches!(error, ReadError::Io { stage:s,os_code:Some(c),.. } if s==stage && c==code)
                );
                let text = error.to_string();
                assert!(
                    text.starts_with(&format!("status IO failure: stage={} kind=", stage.label()))
                );
                assert!(text.ends_with(&format!(" os_code={code}")));
                assert!(text.bytes().all(|b| b.is_ascii_graphic() || b == b' '));
            }
            let error = io_error(
                stage,
                io::Error::other("sensitive-path\u{1b}[31m private-argv error-chain"),
            );
            assert_eq!(
                error,
                ReadError::Io {
                    stage,
                    kind: IoKind::Other,
                    os_code: None
                }
            );
            for text in [error.to_string(), format!("{error:?}")] {
                assert!(
                    !text.contains("sensitive")
                        && !text.contains("private")
                        && !text.contains('\u{1b}')
                );
            }
        }
    }
    #[test]
    fn timeout_wouldblock_and_kind_whitelist_do_not_use_error_messages() {
        for kind in [io::ErrorKind::TimedOut, io::ErrorKind::WouldBlock] {
            assert_eq!(
                io_error(IoStage::Read, io::Error::new(kind, "sensitive")),
                ReadError::Timeout
            );
        }
        for (kind, label) in [
            (io::ErrorKind::NotFound, "not-found"),
            (io::ErrorKind::PermissionDenied, "permission-denied"),
            (io::ErrorKind::ConnectionRefused, "connection-refused"),
            (io::ErrorKind::ConnectionReset, "connection-reset"),
            (io::ErrorKind::ConnectionAborted, "connection-aborted"),
            (io::ErrorKind::NotConnected, "not-connected"),
            (io::ErrorKind::AddrInUse, "address-in-use"),
            (io::ErrorKind::AddrNotAvailable, "address-not-available"),
            (io::ErrorKind::BrokenPipe, "broken-pipe"),
            (io::ErrorKind::WriteZero, "write-zero"),
            (io::ErrorKind::UnexpectedEof, "unexpected-eof"),
            (io::ErrorKind::InvalidInput, "other"),
        ] {
            assert_eq!(
                io_error(IoStage::Connect, io::Error::new(kind, "sensitive")).to_string(),
                format!("status IO failure: stage=connect kind={label} os_code=none")
            );
        }
    }
}

#[cfg(test)]
mod poll_transport_tests {
    use super::*;
    #[test]
    fn bounded_poll_conversion_and_invalid_event_seam() {
        assert_eq!(
            poll_timeout(Duration::from_nanos(1)).unwrap().as_millis(),
            Some(1)
        );
        assert_eq!(
            poll_timeout(Duration::from_micros(1001))
                .unwrap()
                .as_millis(),
            Some(2)
        );
        assert_eq!(
            poll_timeout(Duration::from_secs(10)).unwrap().as_millis(),
            Some(10000)
        );
        assert!(poll_timeout(Duration::MAX).is_err());
        assert_eq!(
            poll_events(Some(PollFlags::POLLNVAL), PollFlags::POLLIN),
            Err(poll_failure())
        );
        assert_eq!(poll_events(None, PollFlags::POLLIN), Err(poll_failure()));
        assert_eq!(
            poll_events(Some(PollFlags::POLLHUP), PollFlags::POLLIN),
            Ok(Some(PollFlags::POLLHUP))
        );
        assert_eq!(
            poll_events(Some(PollFlags::POLLERR), PollFlags::POLLIN),
            Ok(Some(PollFlags::POLLERR))
        );
        assert_eq!(
            poll_events(Some(PollFlags::empty()), PollFlags::POLLIN),
            Ok(None)
        );
    }
    #[test]
    fn pending_write_times_out_under_original_deadline_without_resend() {
        let (sender, _receiver) = UnixStream::pair().unwrap();
        let fd: std::os::fd::OwnedFd = sender.into();
        let socket = Socket::from(fd);
        socket.set_send_buffer_size(4096).unwrap(); // owned fixture only
        socket.set_nonblocking(true).unwrap();
        let fd: std::os::fd::OwnedFd = socket.into();
        let mut sender = UnixStream::from(fd);
        let mut blocked = false;
        for _ in 0..256 {
            match sender.write(&[b'x'; 4096]) {
                Ok(n) => assert!(n > 0),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    blocked = true;
                    break;
                }
                Err(_) => panic!("fixture write failed"),
            }
        }
        assert!(blocked, "bounded fixture did not fill socket");
        assert_eq!(
            exchange(
                &mut sender,
                Instant::now() + Duration::from_millis(40),
                &Request::Status {}
            )
            .err(),
            Some(ReadError::Timeout)
        );
        // Positive readiness control: a fresh, unfilled nonblocking socket.
        let (sender, _receiver) = UnixStream::pair().unwrap();
        sender.set_nonblocking(true).unwrap();
        assert!(
            wait_ready(
                &sender,
                PollFlags::POLLOUT,
                Instant::now() + Duration::from_secs(1)
            )
            .is_ok()
        );
    }
}
