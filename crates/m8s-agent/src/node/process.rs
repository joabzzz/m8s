//! Private bounded direct-child capture shared by native-ID and Docker adapters.
use super::{Error, Result};
use nix::poll::{PollFd, PollFlags, poll};
use std::{
    io::{self, Read},
    os::fd::{AsFd, OwnedFd},
    os::unix::net::UnixStream,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
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
pub(super) fn capture(mut command: Command, deadline: Instant, cap: usize) -> Result<Vec<u8>> {
    if cap == 0 || cap > 16 * 1024 {
        return Err(Error::NativeId);
    }
    remaining(deadline)?;
    let (mut output, child_output) = UnixStream::pair().map_err(|_| Error::NativeId)?;
    output.set_nonblocking(true).map_err(|_| Error::NativeId)?;
    let stdout: OwnedFd = child_output.into();
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::null());
    let mut child = OwnedChild {
        reaped: false,
        child: command.spawn().map_err(|_| Error::NativeId)?,
    };
    // Command retains its configured Stdio handles after spawn. Close the
    // parent writer immediately so a completed child can produce real EOF.
    drop(command);
    let mut bytes = Vec::with_capacity(cap);
    let mut eof = false;
    let mut terminal = false;
    loop {
        remaining(deadline)?;
        if !eof {
            let mut buffer = [0; 1024];
            let available = (cap - bytes.len() + 1).min(buffer.len());
            match output.read(&mut buffer[..available]) {
                Ok(0) => eof = true,
                Ok(n) => {
                    if n > cap - bytes.len() {
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
                remaining(deadline)?;
                return Ok(bytes);
            }
        }
        // A closed stdout does not imply process exit. Bound the wait for
        // exit too, without spinning on persistent HUP or renewing budget.
        let wait = remaining(deadline)?.min(Duration::from_millis(5));
        if eof {
            std::thread::sleep(wait);
        } else {
            let millis =
                i32::try_from(wait.as_millis() + u128::from(wait.subsec_nanos() % 1_000_000 != 0))
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
