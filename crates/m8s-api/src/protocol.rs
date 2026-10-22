//! Legacy control envelopes and additive strict request tags.
use m8s_core::GpuNode;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Up { tier: String },
    Scale { tier: String, replicas: u32 },
    ScaleBatch { targets: Vec<ScaleTarget> },
    Down {},
    Shutdown {},
    ReloadConfig {},
    ReloadConfigCheck {},
    ConfigStatus {},
    ExplainTier { tier: String },
    Status {},
    Logs {},
    LogsPage { after: u64, limit: u32 },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScaleTarget {
    pub tier: String,
    pub replicas: u32,
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

#[cfg(test)]
mod batch_wire_tests {
    use super::*;
    #[test]
    fn additive_batch_tag_nested_fields_and_duplicates_are_strict() {
        let r = Request::ScaleBatch {
            targets: vec![ScaleTarget {
                tier: "a".into(),
                replicas: 0,
            }],
        };
        assert_eq!(
            serde_json::to_string(&r).unwrap(),
            r#"{"command":"scale_batch","targets":[{"tier":"a","replicas":0}]}"#
        );
        for bad in [
            r#"{"command":"scale_batch","targets":[{"tier":"a","replicas":1,"force":true}]}"#,
            r#"{"command":"scale_batch","targets":[{"tier":"a","tier":"b","replicas":1}]}"#,
            r#"{"command":"scale_batch","targets":[],"targets":[]}"#,
        ] {
            assert!(serde_json::from_str::<Request>(bad).is_err());
        }
    }
}
