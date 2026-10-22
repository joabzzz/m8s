//! Stored launch-gate observations, never a launch permit.
use crate::validation::{config_status_error, diagnostic_id, valid_bounds};
use crate::{ConfigTierBounds, MAX_FRAME, Request, client};
use serde::{Deserialize, Serialize};
use std::{io, path::Path};
/// Stored phase metadata, NOT runtime/certificate evidence.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredPhaseCount {
    pub phase: m8s_core::Phase,
    pub count: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredLaunchGates {
    pub tier: String,
    pub evaluated_at: i64,
    #[serde(deserialize_with = "required_diagnostic_option")]
    pub desired: Option<u32>,
    pub stored_node_count: u32,
    pub stored_deleting_count: u32,
    pub stored_phases: Vec<StoredPhaseCount>,
    pub launches_after_cutoff: u32,
    pub future_attempt_present: bool,
    pub launch_rate_available: bool,
    pub rate_limit: u32,
    pub window_seconds: u32,
    pub cutoff_exclusive: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplainTierResponse {
    pub schema: String,
    pub mode: String,
    pub scope: String,
    pub ok: bool,
    #[serde(deserialize_with = "config_status_error")]
    pub error: Option<String>,
    pub readiness: String,
    pub provider_currentness: String,
    pub authorization: String,
    pub configured: bool,
    #[serde(deserialize_with = "required_diagnostic_option")]
    pub configured_tier: Option<ConfigTierBounds>,
    #[serde(deserialize_with = "required_diagnostic_option")]
    pub data: Option<StoredLaunchGates>,
}
fn required_diagnostic_option<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}
/// Pure bounded stored-metadata validation, never a launch permission check.
pub fn decode_explain_tier(frame: &[u8]) -> io::Result<ExplainTierResponse> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid tier explanation");
    if frame.len() > MAX_FRAME
        || !frame.ends_with(b"\n")
        || frame[..frame.len().saturating_sub(1)].contains(&b'\n')
    {
        return Err(invalid());
    }
    let r: ExplainTierResponse = serde_json::from_slice(frame).map_err(|_| invalid())?;
    if r.schema != "m8s.explain-tier/v1"
        || r.mode != "mock-only"
        || r.scope != "stored-launch-gates-only"
        || r.readiness != "not-assessed"
        || r.provider_currentness != "not-assessed"
        || r.authorization != "not-assessed"
    {
        return Err(invalid());
    }
    if !r.ok {
        if r.error.as_deref() != Some("explain-tier-refused")
            || r.data.is_some()
            || r.configured
            || r.configured_tier.is_some()
        {
            return Err(invalid());
        }
        return Ok(r);
    }
    let d = r.data.as_ref().ok_or_else(invalid)?;
    if r.error.is_some()
        || !diagnostic_id(&d.tier)
        || d.evaluated_at < 0
        || d.desired.is_some_and(|n| n > 16)
        || d.stored_deleting_count > d.stored_node_count
        || d.stored_phases.len() > 13
        || d.rate_limit != 3
        || d.window_seconds != 3600
        || d.cutoff_exclusive != d.evaluated_at.saturating_sub(3600)
        || d.launch_rate_available != (!d.future_attempt_present && d.launches_after_cutoff < 3)
        || (d.future_attempt_present && d.launches_after_cutoff == 0)
    {
        return Err(invalid());
    }
    let mut total = 0u32;
    let mut previous = String::new();
    for p in &d.stored_phases {
        let label = serde_json::to_string(&p.phase).map_err(|_| invalid())?;
        if p.count == 0 || label <= previous {
            return Err(invalid());
        }
        previous = label;
        total = total.checked_add(p.count).ok_or_else(invalid)?;
    }
    if total != d.stored_node_count {
        return Err(invalid());
    }
    if let Some(t) = &r.configured_tier
        && (!r.configured || t.id != d.tier || !valid_bounds(std::slice::from_ref(t)))
    {
        return Err(invalid());
    }
    Ok(r)
}
pub fn call_explain_tier(path: &Path, tier: &str) -> io::Result<ExplainTierResponse> {
    if !diagnostic_id(tier) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid tier explanation request",
        ));
    }
    let r = decode_explain_tier(&client::exchange(
        path,
        &Request::ExplainTier { tier: tier.into() },
    )?)?;
    if r.data.as_ref().is_some_and(|d| d.tier != tier) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid tier explanation",
        ));
    }
    Ok(r)
}

#[cfg(test)]
mod explanation_tests {
    use super::*;
    use serde_json::json;
    fn good() -> serde_json::Value {
        json!({"schema":"m8s.explain-tier/v1","mode":"mock-only","scope":"stored-launch-gates-only","ok":true,"error":null,"readiness":"not-assessed","provider_currentness":"not-assessed","authorization":"not-assessed","configured":true,"configured_tier":{"id":"t","min":0,"max":2},"data":{"tier":"t","evaluated_at":3600,"desired":null,"stored_node_count":0,"stored_deleting_count":0,"stored_phases":[],"launches_after_cutoff":0,"future_attempt_present":false,"launch_rate_available":true,"rate_limit":3,"window_seconds":3600,"cutoff_exclusive":0}})
    }
    fn frame(v: &serde_json::Value) -> Vec<u8> {
        let mut b = serde_json::to_vec(v).unwrap();
        b.push(b'\n');
        b
    }
    #[test]
    fn strict_tag_null_context_labels_and_coherence() {
        assert_eq!(
            serde_json::to_string(&Request::ExplainTier { tier: "t".into() }).unwrap(),
            r#"{"command":"explain_tier","tier":"t"}"#
        );
        for s in [
            r#"{"command":"explain_tier","tier":"t","now":0}"#,
            r#"{"command":"explain_tier","tier":"t","tier":"x"}"#,
        ] {
            assert!(serde_json::from_str::<Request>(s).is_err());
        }
        let v = good();
        assert!(decode_explain_tier(&frame(&v)).is_ok());
        for (key, bad) in [
            ("scope", json!("live")),
            ("mode", json!("other")),
            ("schema", json!("bad")),
            ("readiness", json!("Ready")),
            ("provider_currentness", json!("current")),
            ("authorization", json!("approved")),
            ("configured", json!(false)),
            ("data", json!(null)),
            ("unexpected", json!(true)),
        ] {
            let mut x = v.clone();
            x[key] = bad;
            assert!(decode_explain_tier(&frame(&x)).is_err());
        }
        for (key, bad) in [
            ("tier", json!("bad.id")),
            ("evaluated_at", json!(-1)),
            ("desired", json!(17)),
            ("stored_deleting_count", json!(1)),
            ("launches_after_cutoff", json!(3)),
            ("future_attempt_present", json!(true)),
            ("rate_limit", json!(4)),
            ("window_seconds", json!(1)),
            ("cutoff_exclusive", json!(1)),
            ("stored_phases", json!([{"phase":"Ready","count":1}])),
            (
                "stored_phases",
                json!(vec![json!({"phase":"Ready","count":1}); 14]),
            ),
        ] {
            let mut x = v.clone();
            x["data"][key] = bad;
            assert!(decode_explain_tier(&frame(&x)).is_err());
        }
        for key in ["error", "configured_tier", "data"] {
            let mut x = v.clone();
            x.as_object_mut().unwrap().remove(key);
            assert!(decode_explain_tier(&frame(&x)).is_err());
        }
        let mut missing = v.clone();
        missing["data"].as_object_mut().unwrap().remove("desired");
        assert!(decode_explain_tier(&frame(&missing)).is_err());
        let mut unknown = v.clone();
        unknown["configured_tier"] = serde_json::Value::Null;
        assert!(decode_explain_tier(&frame(&unknown)).is_ok());
        unknown["configured"] = json!(false);
        assert!(decode_explain_tier(&frame(&unknown)).is_ok());
        let mut phases = v.clone();
        phases["data"]["stored_node_count"] = json!(2);
        phases["data"]["stored_deleting_count"] = json!(1);
        phases["data"]["stored_phases"] =
            json!([{"phase":"Booting","count":1},{"phase":"Ready","count":1}]);
        assert!(decode_explain_tier(&frame(&phases)).is_ok());
        phases["data"]["stored_phases"]
            .as_array_mut()
            .unwrap()
            .reverse();
        assert!(decode_explain_tier(&frame(&phases)).is_err());
        phases["data"]["stored_phases"] =
            json!([{"phase":"Booting","count":1},{"phase":"Booting","count":1}]);
        assert!(decode_explain_tier(&frame(&phases)).is_err());
        let mut duplicate = String::from_utf8(frame(&v)).unwrap();
        duplicate = duplicate.replace(r#""ok":true"#, r#""ok":true,"ok":true"#);
        assert!(decode_explain_tier(duplicate.as_bytes()).is_err());
        assert!(decode_explain_tier(&vec![b'x'; MAX_FRAME + 1]).is_err());
    }
}
