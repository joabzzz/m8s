//! Local, control-only newline JSON protocol (v1). Never carries inference or secret values.
use m8s_core::GpuNode;
use serde::{Deserialize, Serialize};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};
pub const MAX_FRAME: usize = 65536;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Up { tier: String },
    Scale { tier: String, replicas: u32 },
    Down {},
    Shutdown {},
    Status {},
    Logs {},
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    pub mode: String,
    pub nodes: Vec<GpuNode>,
    pub tiers: Vec<(String, u32)>,
    pub events: Vec<String>,
    pub error: Option<String>,
    pub secure_bootstrap: String,
}
pub fn read_frame(stream: &mut UnixStream) -> io::Result<Vec<u8>> {
    let mut frame = Vec::new();
    BufReader::new(stream.take((MAX_FRAME + 1) as u64)).read_until(b'\n', &mut frame)?;
    if frame.len() > MAX_FRAME || !frame.ends_with(b"\n") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid control frame",
        ));
    }
    Ok(frame)
}
pub fn call(path: &Path, request: &Request) -> io::Result<Response> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    serde_json::to_writer(&mut stream, request)?;
    stream.write_all(b"\n")?;
    let frame = read_frame(&mut stream)?;
    Ok(serde_json::from_slice(&frame)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_fields_and_commands_fail_closed() {
        for frame in [
            r#"{"command":"status","unexpected":true}"#,
            r#"{"command":"down","secret":"not-accepted"}"#,
            r#"{"command":"up","tier":"test","unexpected":1}"#,
            r#"{"command":"infer"}"#,
            r#"{"command":"scale","tier":"test","replicas":-1}"#,
        ] {
            assert!(serde_json::from_str::<Request>(frame).is_err(), "{frame}");
        }
        assert!(serde_json::from_str::<Request>(r#"{"command":"status"}"#).is_ok());
    }
    #[test]
    fn unterminated_or_oversized_frames_are_refused() {
        for data in [b"no-newline".to_vec(), vec![b'x'; MAX_FRAME + 1]] {
            let (mut writer, mut reader) = UnixStream::pair().unwrap();
            let thread = std::thread::spawn(move || {
                let _ = writer.write_all(&data);
            });
            assert!(read_frame(&mut reader).is_err());
            drop(reader);
            thread.join().unwrap();
        }
    }
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;
    #[test]
    fn shutdown_tag_is_additive_and_strict() {
        assert_eq!(
            serde_json::to_string(&Request::Shutdown {}).unwrap(),
            r#"{"command":"shutdown"}"#
        );
        assert!(matches!(
            serde_json::from_str::<Request>(r#"{"command":"shutdown"}"#),
            Ok(Request::Shutdown {})
        ));
        for frame in [
            r#"{"command":"shutdown","tier":"test"}"#,
            r#"{"command":"shutdown","force":true}"#,
        ] {
            assert!(serde_json::from_str::<Request>(frame).is_err());
        }
    }
}
