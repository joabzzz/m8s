//! Local, control-only newline JSON protocol (v1). Never carries inference or secret values.
mod client;
mod config_status;
mod explain;
mod log_page;
mod protocol;
mod reload_check;
mod validation;
pub use config_status::{
    ConfigStatusResponse, ConfigTierBounds, call_config_status, decode_config_status,
};
pub use explain::{
    ExplainTierResponse, StoredLaunchGates, StoredPhaseCount, call_explain_tier,
    decode_explain_tier,
};
pub use log_page::{LogEvent, LogPageResponse, call_log_page};
pub use protocol::{Request, Response, ScaleTarget};
pub use reload_check::{ReloadCheckResponse, call_reload_check, decode_reload_check};
use std::{
    io::{self, BufRead, BufReader, Read},
    os::unix::net::UnixStream,
    path::Path,
};
use validation::diagnostic_id;
pub const MAX_FRAME: usize = 65536;
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
    let frame = client::exchange(path, request)?;
    serde_json::from_slice(&frame)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid control response"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
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
