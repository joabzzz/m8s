//! Whitelisted ProtoJSON read facts. Unknown VM fields (including cloud-init) are
//! discarded, never Debug/Serialize/logged. Known duplicate fields are rejected.
use crate::{Error, Result};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Page<T> {
    #[serde(default = "Vec::new")]
    pub items: Vec<T>,
    #[serde(default)]
    pub next_page_token: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    #[serde(default)]
    pub id: String,
    pub parent_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, deserialize_with = "unique_labels")]
    pub labels: BTreeMap<String, String>,
}
#[derive(Deserialize)]
pub struct Vm {
    pub metadata: Metadata,
    pub spec: VmSpec,
    pub status: VmStatus,
}
#[derive(Deserialize)]
pub struct VmSpec {
    pub resources: Resources,
}
#[derive(Deserialize)]
pub struct Resources {
    pub platform: String,
    pub preset: String,
}
#[derive(Deserialize)]
pub struct VmStatus {
    pub state: String,
}
#[derive(Deserialize)]
pub struct Platform {
    pub metadata: Metadata,
    pub spec: PlatformSpec,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformSpec {
    #[serde(default)]
    pub gpu_count_quota_type: String,
    pub presets: Vec<Preset>,
}
#[derive(Deserialize)]
pub struct Preset {
    pub name: String,
    pub resources: PresetResources,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetResources {
    #[serde(default)]
    pub gpu_count: u32,
}
#[derive(Deserialize)]
pub struct Quota {
    pub metadata: Metadata,
    pub spec: QuotaSpec,
    pub status: QuotaStatus,
}
#[derive(Deserialize)]
pub struct QuotaSpec {
    pub region: String,
    #[serde(default = "zero")]
    pub limit: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaStatus {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub usage_state: String,
    #[serde(default = "zero")]
    pub usage: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub id: String,
    pub resource_id: String,
    pub status: Option<OperationStatus>,
}
#[derive(Deserialize)]
pub struct OperationStatus {
    // ProtoJSON omits zero for OK.
    #[serde(default)]
    pub code: i32,
}
impl Operation {
    pub(super) fn validate(&self, id: Option<&str>) -> Result<()> {
        if !super::component(&self.id)
            || !super::component(&self.resource_id)
            || id.is_some_and(|id| id != self.resource_id)
            || self.status.as_ref().is_some_and(|s| s.code != 0)
        {
            return Err(Error::Unavailable);
        }
        Ok(())
    }
}
fn unique_labels<'de, D: serde::Deserializer<'de>>(
    de: D,
) -> std::result::Result<BTreeMap<String, String>, D::Error> {
    struct Labels;
    impl<'de> serde::de::Visitor<'de> for Labels {
        type Value = BTreeMap<String, String>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("unique labels")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut labels = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, String>()? {
                if labels.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom("duplicate label"));
                }
            }
            Ok(labels)
        }
    }
    de.deserialize_map(Labels)
}

fn zero() -> String {
    "0".into()
}
