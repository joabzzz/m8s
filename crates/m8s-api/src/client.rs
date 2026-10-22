//! One bounded exchange. No path/peer authentication or mutation outcome guarantee.
use crate::{MAX_FRAME, Request};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use socket2::{Domain, SockAddr, Socket, Type};
use std::{
    io::{self, Read, Write},
    os::{fd::AsFd, unix::net::UnixStream},
    path::Path,
    time::{Duration, Instant},
};

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid control frame")
}
fn mapped(label: &'static str, e: io::Error) -> io::Error {
    io::Error::new(e.kind(), label)
}
fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "control deadline exceeded"))
}
fn timeout(duration: Duration) -> io::Result<PollTimeout> {
    let ms = duration
        .as_nanos()
        .checked_add(999_999)
        .ok_or_else(invalid)?
        / 1_000_000;
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
fn ready(stream: &UnixStream, interest: PollFlags, deadline: Instant) -> io::Result<PollFlags> {
    loop {
        let mut fds = [PollFd::new(stream.as_fd(), interest)];
        match poll(&mut fds, timeout(remaining(deadline)?)?) {
            Ok(_) => {
                remaining(deadline)?;
                if let Some(e) = events(fds[0].revents(), interest)? {
                    return Ok(e);
                }
            }
            Err(nix::errno::Errno::EINTR) => continue,
            Err(_) => return Err(io::Error::other("control poll unavailable")),
        }
    }
}
struct RequestBuffer {
    bytes: Vec<u8>,
    deadline: Instant,
}
impl Write for RequestBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        remaining(self.deadline)?;
        if bytes.len() > MAX_FRAME - self.bytes.len() {
            return Err(invalid());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        remaining(self.deadline).map(|_| ())
    }
}
fn prepare(request: &Request, deadline: Instant) -> io::Result<Vec<u8>> {
    remaining(deadline)?;
    // These are the only variable-size request fields. Raw bytes already over
    // the frame cap cannot fit JSON; avoid scanning unbounded caller strings.
    if matches!(request, Request::Up {tier} | Request::Scale {tier,..} if tier.len()>MAX_FRAME) {
        return Err(invalid());
    }
    let mut buffer = RequestBuffer {
        bytes: vec![],
        deadline,
    };
    let result = serde_json::to_writer(&mut buffer, request);
    remaining(deadline)?;
    result.map_err(|_| invalid())?;
    buffer.write_all(b"\n")?;
    Ok(buffer.bytes)
}
pub(super) fn exchange(path: &Path, request: &Request) -> io::Result<Vec<u8>> {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(3))
        .ok_or_else(invalid)?;
    let bytes = prepare(request, deadline)?;
    let socket = Socket::new(Domain::UNIX, Type::STREAM, None)
        .map_err(|e| mapped("control socket unavailable", e))?;
    let address = SockAddr::unix(path).map_err(|e| mapped("control address unavailable", e))?;
    socket
        .connect_timeout(&address, remaining(deadline)?)
        .map_err(|e| mapped("control connect unavailable", e))?;
    remaining(deadline)?;
    socket
        .set_nonblocking(true)
        .map_err(|e| mapped("control nonblocking unavailable", e))?;
    let mut stream = UnixStream::from(std::os::fd::OwnedFd::from(socket));
    transfer(&mut stream, &bytes, deadline)
}
fn transfer(stream: &mut UnixStream, bytes: &[u8], deadline: Instant) -> io::Result<Vec<u8>> {
    let mut offset = 0;
    while offset < bytes.len() {
        let e = ready(stream, PollFlags::POLLOUT, deadline)?;
        remaining(deadline)?;
        let result = stream.write(&bytes[offset..]);
        remaining(deadline)?;
        match result {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "control write unavailable",
                ));
            }
            Ok(n) => offset += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if e.intersects(PollFlags::POLLHUP | PollFlags::POLLERR) {
                    return Err(io::Error::other("control write unavailable"));
                }
            }
            Err(e) => return Err(mapped("control write unavailable", e)),
        }
    }
    let mut frame = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let e = ready(stream, PollFlags::POLLIN, deadline)?;
        remaining(deadline)?;
        let result = stream.read(&mut chunk);
        remaining(deadline)?;
        match result {
            Ok(0) => return Err(invalid()),
            Ok(n) => {
                // Preserve read_frame's first-frame semantics, including ignored suffix.
                let end = chunk[..n].iter().position(|b| *b == b'\n').map(|i| i + 1);
                let count = end.unwrap_or(n);
                if count > MAX_FRAME - frame.len() {
                    return Err(invalid());
                }
                frame.extend_from_slice(&chunk[..count]);
                if end.is_some() {
                    return Ok(frame);
                }
                if frame.len() == MAX_FRAME {
                    return Err(invalid());
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if e.intersects(PollFlags::POLLHUP | PollFlags::POLLERR) {
                    return Err(io::Error::other("control read unavailable"));
                }
            }
            Err(e) => return Err(mapped("control read unavailable", e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pair() -> (UnixStream, UnixStream) {
        let (a, b) = UnixStream::pair().unwrap();
        a.set_nonblocking(true).unwrap();
        (a, b)
    }
    #[test]
    fn poll_rounding_deadline_and_invalid_events_are_bounded() {
        assert_eq!(
            timeout(Duration::from_nanos(1)).unwrap(),
            PollTimeout::try_from(1u32).unwrap()
        );
        assert_eq!(
            timeout(Duration::from_millis(2)).unwrap(),
            PollTimeout::try_from(2u32).unwrap()
        );
        assert!(timeout(Duration::MAX).is_err());
        assert!(events(Some(PollFlags::POLLNVAL), PollFlags::POLLIN).is_err());
        assert!(events(None, PollFlags::POLLIN).is_err());
        assert!(
            events(Some(PollFlags::POLLHUP), PollFlags::POLLIN)
                .unwrap()
                .is_some()
        );
        assert_eq!(
            remaining(Instant::now()).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
    }
    #[test]
    fn drip_and_blocked_reads_share_original_deadline_with_positive_control() {
        for drip in [false, true] {
            let (mut client, mut peer) = pair();
            std::thread::scope(|scope| {
                scope.spawn(move || {
                    peer.set_write_timeout(Some(Duration::from_millis(100)))
                        .unwrap();
                    if drip {
                        for _ in 0..12 {
                            if peer.write_all(b"x").is_err() {
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(10));
                        }
                    } else {
                        std::thread::sleep(Duration::from_millis(120));
                    }
                });
                let deadline = Instant::now() + Duration::from_millis(60);
                let e = transfer(&mut client, b"request\n", deadline).unwrap_err();
                assert_eq!(e.kind(), io::ErrorKind::TimedOut);
                drop(client);
            });
        }
        let (mut client, mut peer) = pair();
        peer.write_all(b"{}\n").unwrap();
        drop(peer);
        // Closed peer cannot accept a write; buffered read positive control uses empty sent request.
        assert_eq!(
            transfer(&mut client, b"", Instant::now() + Duration::from_secs(1)).unwrap(),
            b"{}\n"
        );
    }
    #[test]
    fn partial_writes_preserve_offsets_and_blocked_write_times_out() {
        let (client, mut peer) = pair();
        let socket = Socket::from(std::os::fd::OwnedFd::from(client));
        socket.set_send_buffer_size(4096).unwrap();
        let mut client = UnixStream::from(std::os::fd::OwnedFd::from(socket));
        let bytes: Vec<_> = (0..MAX_FRAME).map(|n| (n % 251) as u8).collect();
        std::thread::scope(|scope| {
            let expected = &bytes;
            let reader = scope.spawn(move || {
                peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
                let mut got = vec![0; expected.len()];
                peer.read_exact(&mut got).unwrap();
                assert_eq!(&got, expected);
                peer.write_all(b"{}\n").unwrap();
            });
            assert_eq!(
                transfer(&mut client, &bytes, Instant::now() + Duration::from_secs(1)).unwrap(),
                b"{}\n"
            );
            reader.join().unwrap();
        });
        let (client, mut peer) = pair();
        let socket = Socket::from(std::os::fd::OwnedFd::from(client));
        socket.set_send_buffer_size(4096).unwrap();
        let mut client = UnixStream::from(std::os::fd::OwnedFd::from(socket));
        assert_eq!(
            transfer(
                &mut client,
                &bytes,
                Instant::now() + Duration::from_millis(40)
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::TimedOut
        );
        peer.set_nonblocking(true).unwrap();
        let mut received = [0; 8192];
        assert!(peer.read(&mut received).unwrap() > 0); // Timeout was after real partial progress.
        drop(peer);
    }
    #[test]
    fn write_wait_and_reply_wait_do_not_get_separate_budgets() {
        for (budget, should_succeed) in [(70, false), (1000, true)] {
            let (client, mut peer) = pair();
            let socket = Socket::from(std::os::fd::OwnedFd::from(client));
            socket.set_send_buffer_size(4096).unwrap();
            let mut client = UnixStream::from(std::os::fd::OwnedFd::from(socket));
            let bytes = vec![b'x'; MAX_FRAME];
            std::thread::scope(|scope| {
                let size = bytes.len();
                scope.spawn(move || {
                    peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
                    peer.set_write_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    std::thread::sleep(Duration::from_millis(30));
                    let mut got = vec![0; size];
                    peer.read_exact(&mut got).unwrap();
                    assert!(got.iter().all(|b| *b == b'x'));
                    std::thread::sleep(Duration::from_millis(60));
                    let _ = peer.write_all(b"{}\n");
                });
                let result = transfer(
                    &mut client,
                    &bytes,
                    Instant::now() + Duration::from_millis(budget),
                );
                if should_succeed {
                    assert_eq!(result.unwrap(), b"{}\n");
                } else {
                    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
                }
                drop(client);
            });
        }
    }
}
