//! Native public-ID observation only; never an admission permit or key decoder.
use super::*;
use nix::poll::{PollFd, PollFlags, poll};
use std::{
    io::{self, Read},
    os::fd::{AsFd, OwnedFd},
    os::unix::net::UnixStream,
    process::{Child, Stdio},
    time::{Duration, Instant},
};

const CAP: usize = 256;
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicId {
    node_id: String,
}
struct OwnedChild {
    child: Child,
    reaped: bool,
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        // Only the retained direct child, never an adopted PID/process group.
        if !self.reaped {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
fn remaining(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or(Error::NativeId)
}
fn decode(bytes: &[u8], expected: EndpointId) -> Result<()> {
    let id: PublicId = serde_json::from_slice(bytes).map_err(|_| Error::NativeId)?;
    if id.node_id.len() != 64 {
        return Err(Error::NativeId);
    }
    for (pair, expected) in id.node_id.as_bytes().chunks_exact(2).zip(expected.0) {
        let nibble = |c| match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        };
        let high = nibble(pair[0]).ok_or(Error::NativeId)?;
        let low = nibble(pair[1]).ok_or(Error::NativeId)?;
        if high * 16 + low != expected {
            return Err(Error::NativeId);
        }
    }
    Ok(())
}
impl NodeMaterial {
    /// Run the selected native binary's *offline* public node-ID command once.
    /// No key parsing/generation here; the native command reads the explicit node
    /// key. This observation is not TLS receipt, admission, native fail-closed
    /// certificate enforcement, or proof a later serve process uses the same key.
    /// The coordinator must still compose BootGate and current launch policy.
    pub fn check_native_node_id(
        &self,
        verifier: &Mp001CertificateVerifier,
        now: i64,
        timeout: Duration,
    ) -> Result<()> {
        let deadline = Instant::now().checked_add(timeout).ok_or(Error::NativeId)?;
        if timeout.is_zero() || timeout > Duration::from_secs(60) {
            return Err(Error::NativeId);
        }
        self.recheck(verifier, now)?;
        self.native_id(deadline)?;
        self.recheck(verifier, now)
    }
    pub(super) fn native_id(&self, deadline: Instant) -> Result<()> {
        check(&self.anchors)?;
        remaining(deadline)?;
        let (mut output, child_output) = UnixStream::pair().map_err(|_| Error::NativeId)?;
        output.set_nonblocking(true).map_err(|_| Error::NativeId)?;
        let stdout: OwnedFd = child_output.into();
        let mut child = OwnedChild {
            reaped: false,
            child: Command::new(&self.launch.program)
                .args(["auth", "node-id", "--key-path"])
                .arg(&self.launch.node_key)
                .arg("--json")
                .env_clear()
                .env("HOME", &self.root)
                .env("MESH_LLM_ANALYTICS", "0")
                .env("DO_NOT_TRACK", "1")
                .stdin(Stdio::null())
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| Error::NativeId)?,
        };
        let mut bytes = Vec::with_capacity(CAP);
        let mut eof = false;
        let mut terminal = false;
        loop {
            remaining(deadline)?;
            if !eof {
                let mut buffer = [0; CAP + 1];
                match output.read(&mut buffer) {
                    Ok(0) => eof = true,
                    Ok(n) => {
                        if n > CAP - bytes.len() {
                            return Err(Error::NativeId);
                        }
                        bytes.extend_from_slice(&buffer[..n]);
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock && !terminal => {}
                    Err(_) => return Err(Error::NativeId),
                }
            }
            remaining(deadline)?;
            if let Some(status) = child.child.try_wait().map_err(|_| Error::NativeId)? {
                child.reaped = true;
                if !status.success() {
                    return Err(Error::NativeId);
                }
                if eof {
                    check(&self.anchors)?;
                    decode(&bytes, self.launch.node)?;
                    remaining(deadline)?;
                    return Ok(());
                }
            }
            // A closed stdout does not imply process exit. Bound the wait for
            // exit too, without spinning on persistent HUP or renewing budget.
            let wait = remaining(deadline)?.min(Duration::from_millis(5));
            if eof {
                std::thread::sleep(wait);
            } else {
                let millis = i32::try_from(
                    wait.as_millis() + u128::from(wait.subsec_nanos() % 1_000_000 != 0),
                )
                .map_err(|_| Error::NativeId)?;
                let mut fds = [PollFd::new(output.as_fd(), PollFlags::POLLIN)];
                match poll(
                    &mut fds,
                    u16::try_from(millis).map_err(|_| Error::NativeId)?,
                ) {
                    Ok(_) => {
                        terminal = fds[0]
                            .revents()
                            .is_some_and(|f| f.intersects(PollFlags::POLLHUP | PollFlags::POLLERR));
                        if fds[0]
                            .revents()
                            .is_none_or(|f| f.contains(PollFlags::POLLNVAL))
                        {
                            return Err(Error::NativeId);
                        }
                    }
                    Err(nix::errno::Errno::EINTR) => {}
                    Err(_) => return Err(Error::NativeId),
                }
            }
        }
    }
}
