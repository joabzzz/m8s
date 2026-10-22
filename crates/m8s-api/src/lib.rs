//! Local, control-only newline JSON protocol (v1). Never carries inference or secret values.
mod client;
use m8s_core::GpuNode;
use serde::{Deserialize, Serialize};
use std::{
    io::{self, BufRead, BufReader, Read},
    os::unix::net::UnixStream,
    path::Path,
};
pub const MAX_FRAME: usize = 65536;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Up { tier: String },
    Scale { tier: String, replicas: u32 },
    Down {},
    Shutdown {},
    ReloadConfig {},
    ReloadConfigCheck {},
    ConfigStatus {},
    Status {},
    Logs {},
    LogsPage { after: u64, limit: u32 },
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

/// One retained local-history event, not inference or secret transport.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LogEvent {
    pub seq: i64,
    pub at: i64,
    pub message: String,
}
/// Separate wire shape; legacy Response and call remain unchanged.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogPageResponse {
    pub ok: bool,
    pub mode: String,
    pub inference: String,
    pub error: Option<String>,
    pub records: Vec<LogEvent>,
    pub next_cursor: u64,
    pub has_more: bool,
}
pub fn call_log_page(path: &Path, after: u64, limit: u32) -> io::Result<LogPageResponse> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid log page");
    if after > i64::MAX as u64 || !(1..=200).contains(&limit) {
        return Err(invalid());
    }
    let response: LogPageResponse = serde_json::from_slice(&client::exchange(
        path,
        &Request::LogsPage { after, limit },
    )?)
    .map_err(|_| invalid())?;
    if response.mode != "mock-only"
        || response.inference != "not-carried"
        || response.records.len() > limit as usize
    {
        return Err(invalid());
    }
    let mut cursor = after;
    for event in &response.records {
        if event.seq <= 0 || event.seq as u64 <= cursor || event.message.len() > 4096 {
            return Err(invalid());
        }
        cursor = event.seq as u64;
    }
    if response.next_cursor != cursor
        || (response.has_more && response.records.len() != limit as usize)
        || (response.ok && response.error.is_some())
        || (!response.ok
            && (response.error.is_none() || !response.records.is_empty() || response.has_more))
    {
        return Err(invalid());
    }
    Ok(response)
}

#[cfg(test)]
mod log_page_tests {
    use super::*;
    #[test]
    fn strict_page_request() {
        assert!(matches!(
            serde_json::from_str::<Request>(r#"{"command":"logs_page","after":0,"limit":2}"#),
            Ok(Request::LogsPage { after: 0, limit: 2 })
        ));
        for frame in [
            r#"{"command":"logs_page","after":0,"limit":2,"extra":true}"#,
            r#"{"command":"logs_page","after":0,"after":1,"limit":2}"#,
            r#"{"command":"logs_page","after":0,"limit":2,"limit":1}"#,
            r#"{"command":"logs_page","after":-1,"limit":2}"#,
            r#"{"command":"logs_page","command":"status","after":0,"limit":2}"#,
        ] {
            assert!(serde_json::from_str::<Request>(frame).is_err());
        }
    }
}

#[cfg(test)]
mod reload_config_tests {
    use super::*;
    #[test]
    fn reload_config_tag_has_no_paths_or_overrides() {
        assert_eq!(
            serde_json::to_string(&Request::ReloadConfig {}).unwrap(),
            r#"{"command":"reload_config"}"#
        );
        assert!(matches!(
            serde_json::from_str::<Request>(r#"{"command":"reload_config"}"#),
            Ok(Request::ReloadConfig {})
        ));
        for frame in [
            r#"{"command":"reload_config","config":"sensitive-path"}"#,
            r#"{"command":"reload_config","force":true}"#,
            r#"{"command":"reload_config","command":"status"}"#,
        ] {
            assert!(serde_json::from_str::<Request>(frame).is_err());
        }
    }
}

/// Active Mock admission bounds only; not desired state or execution authority.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConfigTierBounds {
    pub id: String,
    pub min: u32,
    pub max: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigStatusResponse {
    pub ok: bool,
    pub mode: String,
    pub scope: String,
    #[serde(deserialize_with = "config_status_error")]
    pub error: Option<String>,
    pub configured: bool,
    pub tiers: Vec<ConfigTierBounds>,
}
fn config_status_error<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}
pub fn call_config_status(path: &Path) -> io::Result<ConfigStatusResponse> {
    decode_config_status(&client::exchange(path, &Request::ConfigStatus {})?)
}
/// Pure bounded decoder shared by clients with independently checked transport.
pub fn decode_config_status(frame: &[u8]) -> io::Result<ConfigStatusResponse> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid config status");
    if frame.len() > MAX_FRAME
        || !frame.ends_with(b"\n")
        || frame[..frame.len().saturating_sub(1)].contains(&b'\n')
    {
        return Err(invalid());
    }
    let response: ConfigStatusResponse = serde_json::from_slice(frame).map_err(|_| invalid())?;
    if response.mode != "mock-only"
        || response.scope != "mock-control-admission-only"
        || response.tiers.len() > 32
        || (response.ok
            && (response.error.is_some() || (response.configured && response.tiers.is_empty())))
        || (!response.ok
            && (response.error.is_none() || response.configured || !response.tiers.is_empty()))
        || (!response.configured && !response.tiers.is_empty())
    {
        return Err(invalid());
    }
    if !valid_bounds(&response.tiers) {
        return Err(invalid());
    }
    Ok(response)
}
#[cfg(test)]
mod config_status_tests {
    use super::*;
    #[test]
    fn strict_path_free_config_status_tag() {
        assert_eq!(
            serde_json::to_string(&Request::ConfigStatus {}).unwrap(),
            r#"{"command":"config_status"}"#
        );
        assert!(matches!(
            serde_json::from_str::<Request>(r#"{"command":"config_status"}"#),
            Ok(Request::ConfigStatus {})
        ));
        for frame in [
            r#"{"command":"config_status","path":"sensitive"}"#,
            r#"{"command":"config_status","command":"status"}"#,
        ] {
            assert!(serde_json::from_str::<Request>(frame).is_err());
        }
    }
}

fn valid_bounds(tiers: &[ConfigTierBounds]) -> bool {
    if tiers.len() > 32 {
        return false;
    }
    let mut previous = "";
    for tier in tiers {
        if tier.id.is_empty()
            || tier.id.len() > 48
            || tier.id.as_str() <= previous
            || !tier
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || tier.min > tier.max
            || tier.max > 16
        {
            return false;
        }
        previous = &tier.id;
    }
    true
}

/// Check-only projected admission bounds, not whole configuration equality or a permit.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReloadCheckResponse {
    pub schema: String,
    pub mode: String,
    pub scope: String,
    pub ok: bool,
    #[serde(deserialize_with = "config_status_error")]
    pub error: Option<String>,
    pub check_only: bool,
    pub applied: bool,
    pub later_reload_guaranteed: bool,
    pub active: Vec<ConfigTierBounds>,
    pub candidate: Vec<ConfigTierBounds>,
    #[serde(deserialize_with = "reload_changed")]
    pub bounds_changed: Option<bool>,
}
fn reload_changed<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
    Option::<bool>::deserialize(d)
}
pub fn decode_reload_check(frame: &[u8]) -> io::Result<ReloadCheckResponse> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid reload check");
    if frame.len() > MAX_FRAME
        || !frame.ends_with(b"\n")
        || frame[..frame.len().saturating_sub(1)].contains(&b'\n')
    {
        return Err(invalid());
    }
    let r: ReloadCheckResponse = serde_json::from_slice(frame).map_err(|_| invalid())?;
    if r.schema != "m8s.reload-check/v1"
        || r.mode != "mock-only"
        || r.scope != "mock-control-admission-only"
        || !r.check_only
        || r.applied
        || r.later_reload_guaranteed
        || !valid_bounds(&r.active)
        || !valid_bounds(&r.candidate)
        || (r.ok
            && (r.error.is_some()
                || r.active.is_empty()
                || r.candidate.is_empty()
                || r.bounds_changed != Some(r.active != r.candidate)))
        || (!r.ok
            && (r.error.as_deref() != Some("reload-check-refused")
                || !r.active.is_empty()
                || !r.candidate.is_empty()
                || r.bounds_changed.is_some()))
    {
        return Err(invalid());
    }
    Ok(r)
}
pub fn call_reload_check(path: &Path) -> io::Result<ReloadCheckResponse> {
    decode_reload_check(&client::exchange(path, &Request::ReloadConfigCheck {})?)
}

#[cfg(test)]
mod reload_check_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn strict_request_and_projected_response() {
        assert_eq!(
            serde_json::to_string(&Request::ReloadConfigCheck {}).unwrap(),
            r#"{"command":"reload_config_check"}"#
        );
        for s in [
            r#"{"command":"reload_config_check","path":"secret"}"#,
            r#"{"command":"reload_config_check","command":"status"}"#,
        ] {
            assert!(serde_json::from_str::<Request>(s).is_err());
        }
        let good = json!({"schema":"m8s.reload-check/v1","mode":"mock-only","scope":"mock-control-admission-only","ok":true,"error":null,
            "check_only":true,"applied":false,"later_reload_guaranteed":false,"active":[{"id":"a","min":0,"max":1}],"candidate":[{"id":"a","min":0,"max":2}],"bounds_changed":true});
        let frame = |v: &serde_json::Value| {
            let mut b = serde_json::to_vec(v).unwrap();
            b.push(b'\n');
            b
        };
        assert!(decode_reload_check(&frame(&good)).is_ok());
        for (key, bad) in [
            ("mode", json!("SECRET")),
            ("scope", json!("live")),
            ("schema", json!("bad")),
            ("applied", json!(true)),
            ("check_only", json!(false)),
            ("later_reload_guaranteed", json!(true)),
            ("bounds_changed", json!(false)),
            ("error", json!("SECRET")),
            ("active", json!([])),
            ("candidate", json!([{"id":"\u{1b}","min":0,"max":1}])),
            ("candidate", json!([{"id":"a","min":2,"max":1}])),
            ("candidate", json!([{"id":"a","min":0,"max":17}])),
            (
                "candidate",
                json!([{"id":"b","min":0,"max":1},{"id":"a","min":0,"max":1}]),
            ),
            (
                "candidate",
                json!(vec![json!({"id":"a","min":0,"max":1}); 33]),
            ),
            ("extra", json!(true)),
        ] {
            let mut v = good.clone();
            v[key] = bad;
            assert!(decode_reload_check(&frame(&v)).is_err());
        }
        for key in ["error", "bounds_changed"] {
            let mut missing = good.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(decode_reload_check(&frame(&missing)).is_err());
        }
        let mut b = frame(&good);
        b.pop();
        assert!(decode_reload_check(&b).is_err());
        assert!(decode_reload_check(&vec![b'x'; MAX_FRAME + 1]).is_err());
        let s = String::from_utf8(frame(&good))
            .unwrap()
            .replace("\"ok\":true", "\"ok\":true,\"ok\":true");
        assert!(decode_reload_check(s.as_bytes()).is_err());
    }
    #[test]
    fn bounded_maximum_and_opaque_failure() {
        let tiers: Vec<_> = (0..32)
            .map(|n| ConfigTierBounds {
                id: format!("{n:048}"),
                min: 16,
                max: 16,
            })
            .collect();
        let mut r = ReloadCheckResponse {
            schema: "m8s.reload-check/v1".into(),
            mode: "mock-only".into(),
            scope: "mock-control-admission-only".into(),
            ok: true,
            error: None,
            check_only: true,
            applied: false,
            later_reload_guaranteed: false,
            active: tiers.clone(),
            candidate: tiers,
            bounds_changed: Some(false),
        };
        let frame = |r: &ReloadCheckResponse| {
            let mut b = serde_json::to_vec(r).unwrap();
            b.push(b'\n');
            b
        };
        let b = frame(&r);
        assert!(b.len() < 8192);
        assert!(decode_reload_check(&b).is_ok());
        r.ok = false;
        r.error = Some("reload-check-refused".into());
        r.active.clear();
        r.candidate.clear();
        r.bounds_changed = None;
        assert!(decode_reload_check(&frame(&r)).is_ok());
        r.error = Some("secret".into());
        assert!(decode_reload_check(&frame(&r)).is_err());
    }
}
