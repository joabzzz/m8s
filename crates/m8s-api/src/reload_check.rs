//! Check-only prospective admission snapshot contract.
use crate::validation::{config_status_error, valid_bounds};
use crate::{ConfigTierBounds, MAX_FRAME, Request, client};
use serde::{Deserialize, Serialize};
use std::{io, path::Path};
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
