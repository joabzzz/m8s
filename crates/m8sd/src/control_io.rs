//! Private accepted-client transport budget; not an execution/transaction timeout.
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use std::{
    io::{self, Read, Write},
    os::{fd::AsFd, unix::net::UnixStream},
    time::{Duration, Instant},
};
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid control frame")
}
fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "control deadline exceeded"))
}
fn timeout(d: Duration) -> io::Result<PollTimeout> {
    let ms = d.as_nanos().checked_add(999_999).ok_or_else(invalid)? / 1_000_000;
    PollTimeout::try_from(ms).map_err(|_| invalid())
}
fn events(events: Option<PollFlags>, interest: PollFlags) -> io::Result<Option<PollFlags>> {
    let e = events.ok_or_else(invalid)?;
    if e.contains(PollFlags::POLLNVAL) {
        return Err(io::Error::other("control poll unavailable"));
    }
    Ok(
        e.intersects(interest | PollFlags::POLLHUP | PollFlags::POLLERR)
            .then_some(e),
    )
}
pub(super) struct ControlIo {
    stream: UnixStream,
    deadline: Instant,
}
impl ControlIo {
    pub(super) fn new(stream: UnixStream, deadline: Instant) -> io::Result<Self> {
        remaining(deadline)?;
        stream
            .set_nonblocking(true)
            .map_err(|_| io::Error::other("control setup unavailable"))?;
        remaining(deadline)?;
        Ok(Self { stream, deadline })
    }
    fn ready(&self, interest: PollFlags) -> io::Result<PollFlags> {
        loop {
            let mut fds = [PollFd::new(self.stream.as_fd(), interest)];
            match poll(&mut fds, timeout(remaining(self.deadline)?)?) {
                Ok(_) => {
                    remaining(self.deadline)?;
                    if let Some(e) = events(fds[0].revents(), interest)? {
                        return Ok(e);
                    }
                }
                Err(nix::errno::Errno::EINTR) => continue,
                Err(_) => return Err(io::Error::other("control poll unavailable")),
            }
        }
    }
    pub(super) fn read_frame(&mut self) -> io::Result<Vec<u8>> {
        let mut frame = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = self.read(&mut chunk)?;
            if n == 0 {
                return Err(invalid());
            }
            let end = chunk[..n].iter().position(|b| *b == b'\n').map(|i| i + 1);
            let count = end.unwrap_or(n);
            if count > m8s_api::MAX_FRAME - frame.len() {
                return Err(invalid());
            }
            frame.extend_from_slice(&chunk[..count]);
            if end.is_some() {
                return Ok(frame);
            }
            if frame.len() == m8s_api::MAX_FRAME {
                return Err(invalid());
            }
        }
    }
    fn exchange_io(&mut self, buf: &mut [u8], write: Option<&[u8]>) -> io::Result<usize> {
        remaining(self.deadline)?;
        if write.map_or(buf.is_empty(), |b| b.is_empty()) {
            return Ok(0);
        }
        let interest = if write.is_some() {
            PollFlags::POLLOUT
        } else {
            PollFlags::POLLIN
        };
        loop {
            let e = self.ready(interest)?;
            remaining(self.deadline)?;
            let result = match write {
                Some(bytes) => self.stream.write(bytes),
                None => self.stream.read(buf),
            };
            remaining(self.deadline)?;
            match result {
                Ok(0) if write.is_some() => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "control write unavailable",
                    ));
                }
                Ok(n) => return Ok(n),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if e.intersects(PollFlags::POLLHUP | PollFlags::POLLERR) {
                        return Err(io::Error::other("control I/O unavailable"));
                    }
                }
                Err(e) => return Err(io::Error::new(e.kind(), "control I/O unavailable")),
            }
        }
    }
}
impl Read for ControlIo {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.exchange_io(buf, None)
    }
}
impl Write for ControlIo {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.exchange_io(&mut [], Some(buf))
    }
    fn flush(&mut self) -> io::Result<()> {
        remaining(self.deadline).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pair(budget: Duration) -> (ControlIo, UnixStream) {
        let (a, b) = UnixStream::pair().unwrap();
        (ControlIo::new(a, Instant::now() + budget).unwrap(), b)
    }
    #[test]
    fn poll_rounding_terminal_classification_and_expired_setup_are_closed() {
        assert_eq!(
            timeout(Duration::from_nanos(1)).unwrap(),
            PollTimeout::try_from(1u32).unwrap()
        );
        assert_eq!(
            timeout(Duration::from_millis(2)).unwrap(),
            PollTimeout::try_from(2u32).unwrap()
        );
        assert!(timeout(Duration::MAX).is_err());
        assert!(events(None, PollFlags::POLLIN).is_err());
        assert!(events(Some(PollFlags::POLLNVAL), PollFlags::POLLIN).is_err());
        assert!(
            events(Some(PollFlags::POLLHUP), PollFlags::POLLIN)
                .unwrap()
                .is_some()
        );
        assert!(
            events(Some(PollFlags::POLLERR), PollFlags::POLLOUT)
                .unwrap()
                .is_some()
        );
        assert!(
            events(Some(PollFlags::empty()), PollFlags::POLLIN)
                .unwrap()
                .is_none()
        );
        let (a, _b) = UnixStream::pair().unwrap();
        assert_eq!(
            ControlIo::new(a, Instant::now()).err().unwrap().kind(),
            io::ErrorKind::TimedOut
        );
    }
    #[test]
    fn complete_buffered_close_and_first_frame_cap_are_preserved() {
        for bytes in [b"{}\nignored-suffix".to_vec(), {
            let mut b = vec![b'x'; m8s_api::MAX_FRAME - 1];
            b.push(b'\n');
            b
        }] {
            let expected = bytes[..=bytes.iter().position(|b| *b == b'\n').unwrap()].to_vec();
            let (mut io, mut peer) = pair(Duration::from_secs(2));
            std::thread::scope(|scope| {
                let writer = scope.spawn(move || {
                    peer.set_write_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    peer.write_all(&bytes).unwrap();
                    // Immediately close; no client EOF handshake masking buffered EOF.
                });
                assert_eq!(io.read_frame().unwrap(), expected);
                writer.join().unwrap();
            });
        }
    }
    #[test]
    fn partial_eof_and_overcap_never_become_complete_frames() {
        for bytes in [b"partial".to_vec(), {
            let mut b = vec![b'x'; m8s_api::MAX_FRAME];
            b.push(b'\n');
            b
        }] {
            let (mut io, mut peer) = pair(Duration::from_secs(2));
            std::thread::scope(|scope| {
                let writer = scope.spawn(move || {
                    peer.set_write_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    let _ = peer.write_all(&bytes);
                });
                assert_eq!(
                    io.read_frame().unwrap_err().kind(),
                    io::ErrorKind::InvalidData
                );
                drop(io);
                writer.join().unwrap();
            });
        }
    }
    #[test]
    fn trickle_does_not_renew_absolute_read_budget_with_positive_control() {
        let (mut io, mut peer) = pair(Duration::from_millis(100));
        peer.write_all(b"ok\n").unwrap();
        assert_eq!(io.read_frame().unwrap(), b"ok\n");
        std::thread::scope(|scope| {
            let writer = scope.spawn(move || {
                peer.set_write_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                for _ in 0..15 {
                    if peer.write_all(b"x").is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            });
            let start = Instant::now();
            assert_eq!(io.read_frame().unwrap_err().kind(), io::ErrorKind::TimedOut);
            assert!(start.elapsed() < Duration::from_secs(1));
            drop(io);
            writer.join().unwrap();
        });
    }
    #[test]
    fn stalled_write_and_read_then_write_share_deadline() {
        let (mut io, mut peer) = pair(Duration::from_millis(100));
        let original_deadline = io.deadline;
        peer.write_all(b"request\n").unwrap();
        assert_eq!(io.read_frame().unwrap(), b"request\n");
        assert_eq!(io.deadline, original_deadline);
        let start = Instant::now();
        // Reuse a small bounded buffer until the non-reading peer backpressures.
        let error = loop {
            match io.write_all(&[b'x'; 4096]) {
                Ok(()) => {}
                Err(e) => break e,
            }
        };
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(io.deadline, original_deadline);
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(io.flush().unwrap_err().kind(), io::ErrorKind::TimedOut);
        // The descriptor remains owned and no request or connection is retried.
        drop(io);
        drop(peer);
    }
}
