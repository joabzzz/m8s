//! Active Mock admission snapshot contract, not execution authority.
use crate::validation::{config_status_error, valid_bounds};
use crate::{MAX_FRAME, Request, client};
use serde::{Deserialize, Serialize};
use std::{io, path::Path};
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
