//! Single loopback HTTP exchange; no redirects, proxy, credentials or TLS substitute.
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream},
    os::fd::AsFd,
    time::{Duration, Instant},
};
const HEADER: usize = 8192;
const BODY: usize = 65536;
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "prototype HTTP refused")
}
fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "prototype deadline exceeded"))
}
fn ready(stream: &TcpStream, interest: PollFlags, deadline: Instant) -> io::Result<PollFlags> {
    loop {
        let ms = remaining(deadline)?
            .as_nanos()
            .checked_add(999999)
            .ok_or_else(invalid)?
            / 1000000;
        let mut fds = [PollFd::new(stream.as_fd(), interest)];
        match poll(&mut fds, PollTimeout::try_from(ms).map_err(|_| invalid())?) {
            Err(nix::errno::Errno::EINTR) => continue,
            Err(_) => return Err(invalid()),
            Ok(_) => {
                remaining(deadline)?;
                let flags = fds[0].revents().ok_or_else(invalid)?;
                if flags.contains(PollFlags::POLLNVAL) {
                    return Err(invalid());
                }
                if flags.intersects(interest | PollFlags::POLLHUP | PollFlags::POLLERR) {
                    return Ok(flags);
                }
            }
        }
    }
}
fn send(stream: &mut TcpStream, bytes: &[u8], deadline: Instant) -> io::Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        let flags = ready(stream, PollFlags::POLLOUT, deadline)?;
        match stream.write(&bytes[offset..]) {
            Ok(0) => return Err(invalid()),
            Ok(n) => offset += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if flags.intersects(PollFlags::POLLHUP | PollFlags::POLLERR) {
                    return Err(invalid());
                }
            }
            Err(_) => return Err(invalid()),
        }
        remaining(deadline)?;
    }
    Ok(())
}
// Content-Length only: mesh's non-streaming JSON response. Chunked/compressed,
// redirects, streaming and ambiguous framing refuse rather than fabricate output.
fn headers(bytes: &[u8]) -> io::Result<usize> {
    if !bytes.is_ascii() {
        return Err(invalid());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let mut lines = text.split("\r\n");
    let status = lines.next().ok_or_else(invalid)?;
    if !(status.starts_with("HTTP/1.1 200 ") || status.starts_with("HTTP/1.0 200 ")) {
        return Err(invalid());
    }
    let mut h = BTreeMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line.split_once(':').ok_or_else(invalid)?;
        if name.is_empty()
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || value.bytes().any(|b| b.is_ascii_control() && b != b'\t')
            || h.insert(name.to_ascii_lowercase(), value.trim()).is_some()
            || h.len() > 64
        {
            return Err(invalid());
        }
    }
    if h.contains_key("transfer-encoding")
        || h.get("content-encoding")
            .is_some_and(|v| !v.eq_ignore_ascii_case("identity"))
        || h.get("content-type").is_none_or(|v| {
            !v.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("application/json")
        })
    {
        return Err(invalid());
    }
    let n = h.get("content-length").ok_or_else(invalid)?;
    if n.is_empty() || n.len() > 5 || !n.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    n.parse::<usize>()
        .ok()
        .filter(|n| *n <= BODY)
        .ok_or_else(invalid)
}
pub(super) fn exchange(port: u16, body: &[u8], deadline: Instant) -> io::Result<Vec<u8>> {
    request(port, "POST /v1/chat/completions", body, deadline)
}
pub(super) fn models(port: u16, deadline: Instant) -> io::Result<Vec<u8>> {
    request(port, "GET /v1/models", &[], deadline)
}
#[cfg(feature = "prototype-vast")]
pub(super) fn status(port: u16, deadline: Instant) -> io::Result<Vec<u8>> {
    request(port, "GET /api/status", &[], deadline)
}
fn request(port: u16, route: &str, body: &[u8], deadline: Instant) -> io::Result<Vec<u8>> {
    if body.len() > BODY {
        return Err(invalid());
    }
    let head = format!(
        "{route} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nAccept: application/json\r\nAccept-Encoding: identity\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    remaining(deadline)?;
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&address, remaining(deadline)?)?;
    remaining(deadline)?;
    stream.set_nonblocking(true)?;
    send(&mut stream, head.as_bytes(), deadline)?;
    send(&mut stream, body, deadline)?;
    let mut bytes = Vec::new();
    let mut frame = None;
    loop {
        let flags = ready(&stream, PollFlags::POLLIN, deadline)?;
        let mut chunk = [0; 4096];
        match stream.read(&mut chunk) {
            Ok(0) => return Err(invalid()),
            Ok(n) => {
                remaining(deadline)?;
                if bytes.len() + n > HEADER + BODY {
                    return Err(invalid());
                }
                bytes.extend_from_slice(&chunk[..n]);
                if frame.is_none() {
                    if let Some(pos) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                        let start = pos + 4;
                        if start > HEADER {
                            return Err(invalid());
                        }
                        frame = Some((start, headers(&bytes[..pos])?));
                    } else if bytes.len() >= HEADER {
                        return Err(invalid());
                    }
                }
                if let Some((start, length)) = frame {
                    if bytes.len() > start + length {
                        return Err(invalid());
                    }
                    if bytes.len() == start + length {
                        remaining(deadline)?;
                        return Ok(bytes.split_off(start));
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if flags.intersects(PollFlags::POLLHUP | PollFlags::POLLERR) {
                    return Err(invalid());
                }
            }
            Err(_) => return Err(invalid()),
        }
    }
}
