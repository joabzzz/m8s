//! Retained local-history paging contract and bounded client validation.
use crate::{Request, client};
use serde::{Deserialize, Serialize};
use std::{io, path::Path};
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
