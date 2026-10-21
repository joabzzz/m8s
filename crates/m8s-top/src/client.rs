//! Strict v1 status reader with one absolute connect/write/read deadline.
use crate::{NodeStatus, StatusSnapshot, display_text};
use m8s_api::{MAX_FRAME, Request};
use m8s_core::{Phase, Timestamp};
use serde::{Deserialize, Deserializer};
use socket2::{Domain, SockAddr, Socket, Type};
use std::io::{self, Read, Write};
use std::os::unix::{
    fs::{FileTypeExt, MetadataExt},
    net::UnixStream,
};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadError {
    InvalidTimeout,
    Unavailable,
    UnsafeSocket,
    Timeout,
    InvalidResponse,
    Rejected,
}
impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidTimeout => "invalid status timeout",
            Self::Unavailable => "status socket unavailable",
            Self::UnsafeSocket => "status socket trust checks failed",
            Self::Timeout => "status request timed out",
            Self::InvalidResponse => "invalid status response",
            Self::Rejected => "status request rejected",
        })
    }
}
impl std::error::Error for ReadError {}
fn io_error(error: io::Error) -> ReadError {
    match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => ReadError::Timeout,
        _ => ReadError::Unavailable,
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
        let uid = nix::unistd::geteuid().as_raw();
        let checked = validate_socket_path(path, uid)?;
        let socket = Socket::new(Domain::UNIX, Type::STREAM, None).map_err(io_error)?;
        let address = SockAddr::unix(&checked.path).map_err(io_error)?;
        socket
            .connect_timeout(&address, remaining(deadline)?)
            .map_err(io_error)?;
        verify_peer_uid(&socket, uid)?;
        let after = validate_socket_path(&checked.path, uid)?;
        if checked != after {
            return Err(ReadError::UnsafeSocket);
        }
        remaining(deadline)?;
        let descriptor: std::os::fd::OwnedFd = socket.into();
        let mut stream = UnixStream::from(descriptor);
        read_status(&mut stream, deadline)
    }
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
        std::env::current_dir().map_err(io_error)?.join(path)
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
                let meta = std::fs::symlink_metadata(&clean).map_err(io_error)?;
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
    let dir = std::fs::symlink_metadata(parent).map_err(io_error)?;
    let socket = std::fs::symlink_metadata(&clean).map_err(io_error)?;
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

fn read_status(stream: &mut UnixStream, deadline: Instant) -> Result<StatusSnapshot, ReadError> {
    let mut request =
        serde_json::to_vec(&Request::Status {}).map_err(|_| ReadError::InvalidResponse)?;
    request.push(b'\n');
    let mut offset = 0;
    while offset < request.len() {
        stream
            .set_write_timeout(Some(remaining(deadline)?))
            .map_err(io_error)?;
        match stream.write(&request[offset..]) {
            Ok(0) => return Err(ReadError::Unavailable),
            Ok(n) => offset += n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(io_error(error)),
        }
    }
    let mut frame = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        stream
            .set_read_timeout(Some(remaining(deadline)?))
            .map_err(io_error)?;
        let count = match stream.read(&mut chunk) {
            Ok(0) => return Err(ReadError::InvalidResponse),
            Ok(n) => n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(io_error(error)),
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
            return decode_status(&frame);
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
