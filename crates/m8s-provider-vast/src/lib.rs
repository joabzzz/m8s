//! Vast mapping defaults to fixture-only; optional explicit readonly-https transport.
//! Read-only transport never mutates or certifies snapshot completeness. Separately
//! enabled scoped cleanup separates acknowledgement from exact absence lookup.
//! API field assumptions come from the supplied S1 research; live validation is outstanding.
use m8s_core::provider::{Error, LaunchSpec, Provider, Result};
use m8s_core::{Instance, InstanceState, Observation, Offer};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[cfg(all(unix, feature = "launch-https"))]
pub mod provision;
pub mod provision_protocol;

const MAX_BODY: usize = 1_048_576;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
}
/// Not Debug/Serialize: raw request/response bodies must not be logged.
pub struct Request {
    pub method: Method,
    pub path: String,
    pub body: Option<Value>,
}
/// Completeness is asserted by a synthetic fixture, not inferred from HTTP status.
/// A future production transport needs independent pagination/completeness validation.
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
    pub complete_instance_snapshot: bool,
}
pub trait Transport {
    /// No retry is performed by the mapping adapter. Production transports must
    /// independently validate completeness and may refuse unsupported operations.
    fn exchange(&mut self, request: Request) -> Result<Reply>;
}
/// Compatibility alias for existing deterministic fixture implementations.
pub use Transport as FixtureTransport;

#[cfg(feature = "readonly-https")]
pub mod readonly;
#[cfg(all(unix, feature = "watchdog"))]
pub mod watchdog;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Market {
    Community,
    Secure,
}
impl Market {
    fn datacenter(self) -> bool {
        self == Self::Secure
    }
}
/// Fixture image syntax, not evidence of signature verification. No env, keys,
/// onstart scripts, template, image-login, or arbitrary options are accepted.
pub struct FixtureConfig {
    pub image: String,
    pub disk_gb: u32,
}
pub struct Vast<T: FixtureTransport> {
    market: Market,
    owner: String,
    config: FixtureConfig,
    transport: T,
    // None means request submitted/uncertain; never automatically submit it again.
    launches: BTreeMap<String, LaunchAttempt>,
}
struct LaunchAttempt {
    tier: String,
    offer_id: String,
    result: Option<Instance>,
}
impl<T: FixtureTransport> Vast<T> {
    pub fn fixture_only(
        market: Market,
        owner: String,
        config: FixtureConfig,
        transport: T,
    ) -> Result<Self> {
        if !component(&owner)
            || !(1..=4096).contains(&config.disk_gb)
            || !pinned_image(&config.image)
        {
            return Err(Error::Invalid);
        }
        Ok(Self {
            market,
            owner,
            config,
            transport,
            launches: BTreeMap::new(),
        })
    }
    pub fn into_transport(self) -> T {
        self.transport
    }
    fn exchange(
        &mut self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<(Value, bool)> {
        let reply = self.transport.exchange(Request {
            method,
            path: path.into(),
            body,
        })?;
        if !(200..300).contains(&reply.status) {
            return Err(Error::Unavailable);
        }
        if reply.body.len() > MAX_BODY {
            return Err(Error::Invalid);
        }
        // serde_json::Value otherwise silently keeps the last duplicate key,
        // which could turn a present-instance list into an apparently empty one.
        serde_json::from_slice::<UniqueKeys>(&reply.body).map_err(|_| Error::Invalid)?;
        let value: Value = serde_json::from_slice(&reply.body).map_err(|_| Error::Invalid)?;
        let object = value.as_object().ok_or(Error::Invalid)?;
        if object
            .get("success")
            .is_some_and(|v| v != &Value::Bool(true))
            || object.contains_key("error")
            || object.contains_key("errors")
        {
            return Err(Error::Unavailable);
        }
        Ok((value, reply.complete_instance_snapshot))
    }
    fn snapshot(&mut self) -> Result<Vec<Value>> {
        let (value, complete) = self.exchange(Method::Get, "/api/v0/instances/", None)?;
        if !complete
            || value
                .get("has_more")
                .is_some_and(|v| v != &Value::Bool(false))
            || ["next", "next_page", "cursor"]
                .iter()
                .any(|key| value.get(*key).is_some_and(|v| !v.is_null()))
        {
            return Err(Error::Unavailable);
        }
        let rows = value
            .get("instances")
            .and_then(Value::as_array)
            .ok_or(Error::Invalid)?;
        if rows.len() > 4096 {
            return Err(Error::Invalid);
        }
        for key in ["total", "total_count"] {
            if let Some(total) = value.get(key)
                && total.as_u64() != Some(rows.len() as u64)
            {
                return Err(Error::Unavailable);
            }
        }
        let mut ids = BTreeSet::new();
        for row in rows {
            let id = integer_id(row.get("id").ok_or(Error::Invalid)?)?;
            if !ids.insert(id) {
                return Err(Error::Invalid);
            }
        }
        Ok(rows.clone())
    }
    fn map_instance(&self, row: &Value) -> Result<Instance> {
        let id = integer_id(row.get("id").ok_or(Error::Invalid)?)?;
        let label = text(row, "label")?;
        let (owner, node_uid, tier) = parse_label(label).ok_or(Error::Invalid)?;
        if owner != self.owner {
            return Err(Error::Invalid);
        }
        let state = match text(row, "actual_status")? {
            "running" => InstanceState::Running,
            "created" | "loading" => InstanceState::Starting,
            "stopped" => InstanceState::Stopped,
            "exited" => InstanceState::Exited,
            // Unknown/error/destroyed statuses are not positive absence.
            _ => return Err(Error::Unavailable),
        };
        Ok(Instance {
            id,
            owner: owner.into(),
            node_uid: node_uid.into(),
            tier: tier.into(),
            state,
        })
    }
}
impl<T: FixtureTransport> Provider for Vast<T> {
    fn id(&self) -> &'static str {
        match self.market {
            Market::Community => "vast-community",
            Market::Secure => "vast-secure",
        }
    }
    fn search(&mut self) -> Result<Vec<Offer>> {
        let filters = json!({
            "verified":{"eq":true},"datacenter":{"eq":self.market.datacenter()},
            "rentable":{"eq":true},"external":{"eq":false},"type":{"eq":"ondemand"},
            "inet_down":{"gte":1000},
            "geolocation":{"notin":["CU","IR","KP","SY","RU","BY","CN","UA"]}
        });
        let (value, _) = self.exchange(Method::Post, "/api/v0/bundles/", Some(filters))?;
        let rows = value
            .get("offers")
            .and_then(Value::as_array)
            .ok_or(Error::Invalid)?;
        if rows.len() > 1024 {
            return Err(Error::Invalid);
        }
        let mut offers = vec![];
        let mut ids = BTreeSet::new();
        for row in rows {
            let id = integer_id(row.get("id").ok_or(Error::Invalid)?)?;
            if !ids.insert(id.clone()) {
                return Err(Error::Invalid);
            }
            // Reapply explicit market/default filters: don't trust server defaults.
            if !boolean(row, "verified")?
                || boolean(row, "datacenter")? != self.market.datacenter()
                || !boolean(row, "rentable")?
                || boolean(row, "external")?
                || text(row, "type")? != "ondemand"
            {
                continue;
            }
            let count = row
                .get("num_gpus")
                .and_then(Value::as_u64)
                .ok_or(Error::Invalid)?;
            if count == 0 || count > u32::MAX as u64 {
                return Err(Error::Invalid);
            }
            let reliability = number(row, "reliability", 10_000, false)?;
            // Check the original decimal at higher precision too, so >1 cannot round down to1.
            if number(row, "reliability", 1, true)? > 1 || reliability > 10_000 {
                return Err(Error::Invalid);
            }
            let offer = Offer {
                id,
                country: row
                    .get("geolocation")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                region: None,
                inet_down_mbps: number(row, "inet_down", 1, false)?,
                gpu_count: count as u32,
                // S1 docs specify MB. Use decimal MB conservatively, never overstate VRAM.
                vram_bytes_per_gpu: number(row, "gpu_ram", 1_000_000, false)?,
                hourly_micro_usd: number(row, "dph_total", 1_000_000, true)?,
                interruptible: false,
                reliability_bps: reliability as u16,
            };
            // UA excluded entirely until a trustworthy subdivision source exists.
            if offer.country.as_deref() == Some("UA")
                || offer.inet_down_mbps < 1000
                || m8s_core::policy::check_location(&offer).is_err()
            {
                continue;
            }
            offers.push(offer);
        }
        offers.sort_by(|a, b| {
            a.hourly_micro_usd
                .cmp(&b.hourly_micro_usd)
                .then(b.reliability_bps.cmp(&a.reliability_bps))
        });
        Ok(offers)
    }
    fn launch(&mut self, offer: &Offer, spec: &LaunchSpec) -> Result<Instance> {
        if spec.owner != self.owner || !component(&spec.node_uid) || !component(&spec.tier) {
            return Err(Error::Invalid);
        }
        numeric_path_id(&offer.id)?;
        let label = format!("m8s:v1:{}:{}:{}", spec.owner, spec.node_uid, spec.tier);
        let intent_key = format!("{}:{}", spec.owner, spec.node_uid);
        if let Some(prior) = self.launches.get(&intent_key) {
            if prior.tier != spec.tier || prior.offer_id != offer.id {
                return Err(Error::Invalid);
            }
            return prior.result.clone().ok_or(Error::Unavailable);
        }
        // Mark uncertain BEFORE transport; a failure must not result in automatic retry.
        self.launches.insert(
            intent_key.clone(),
            LaunchAttempt {
                tier: spec.tier.clone(),
                offer_id: offer.id.clone(),
                result: None,
            },
        );
        let (value, _) = self.exchange(
            Method::Put,
            &format!("/api/v0/asks/{}/", offer.id),
            Some(json!({
                "image":self.config.image,"label":label,"disk":self.config.disk_gb,
                "runtype":"args","target_state":"running","cancel_unavail":true
            })),
        )?;
        if value.get("success") != Some(&Value::Bool(true)) {
            return Err(Error::Unavailable);
        }
        let id = integer_id(value.get("new_contract").ok_or(Error::Invalid)?)?;
        let instance = Instance {
            id,
            owner: spec.owner.clone(),
            node_uid: spec.node_uid.clone(),
            tier: spec.tier.clone(),
            state: InstanceState::Starting,
        };
        self.launches
            .get_mut(&intent_key)
            .ok_or(Error::Invalid)?
            .result = Some(instance.clone());
        Ok(instance)
    }
    fn describe(&mut self, id: &str) -> Result<Observation> {
        numeric_path_id(id)?;
        let rows = self.snapshot()?;
        match rows.iter().find(|row| {
            row.get("id")
                .is_some_and(|v| integer_id(v).is_ok_and(|s| s == id))
        }) {
            Some(row) => Ok(Observation::Present(self.map_instance(row)?)),
            // Only an explicitly complete, validated successful snapshot confirms absence.
            None => Ok(Observation::Absent),
        }
    }
    fn list_owned(&mut self, owner: &str) -> Result<Vec<Instance>> {
        if owner != self.owner {
            return Err(Error::Invalid);
        }
        let rows = self.snapshot()?;
        let mut result = vec![];
        for row in rows {
            let Some(label) = row.get("label").and_then(Value::as_str) else {
                continue;
            };
            if let Some((label_owner, _, _)) = parse_label(label)
                && label_owner == owner
            {
                result.push(self.map_instance(&row)?);
            }
        }
        Ok(result)
    }
    fn destroy(&mut self, id: &str) -> Result<()> {
        // Exact ownership check prevents deleting a foreign id even with a broad future key.
        match self.describe(id)? {
            Observation::Absent => return Ok(()),
            Observation::Present(i) if i.owner == self.owner => {}
            _ => return Err(Error::Invalid),
        }
        let (value, _) =
            self.exchange(Method::Delete, &format!("/api/v0/instances/{id}/"), None)?;
        if value.get("success") != Some(&Value::Bool(true)) {
            return Err(Error::Unavailable);
        }
        // Acknowledged only; callers must still describe/poll. Never translate to Absent here.
        Ok(())
    }
}
fn boolean(row: &Value, key: &str) -> Result<bool> {
    row.get(key).and_then(Value::as_bool).ok_or(Error::Invalid)
}
fn text<'a>(row: &'a Value, key: &str) -> Result<&'a str> {
    row.get(key).and_then(Value::as_str).ok_or(Error::Invalid)
}
fn component(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn parse_label(label: &str) -> Option<(&str, &str, &str)> {
    let p: Vec<_> = label.split(':').collect();
    if p.len() == 5 && p[0] == "m8s" && p[1] == "v1" && p[2..].iter().all(|v| component(v)) {
        Some((p[2], p[3], p[4]))
    } else {
        None
    }
}
fn pinned_image(image: &str) -> bool {
    let Some((name, digest)) = image.split_once("@sha256:") else {
        return false;
    };
    !name.is_empty()
        && name.len() <= 256
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"./_-:".contains(&b))
        && digest.len() == 64
        && digest.bytes().all(|b| b.is_ascii_hexdigit())
}
fn numeric_path_id(id: &str) -> Result<()> {
    if id
        .parse::<u64>()
        .is_ok_and(|v| v > 0 && v.to_string() == id)
    {
        Ok(())
    } else {
        Err(Error::Invalid)
    }
}
fn integer_id(v: &Value) -> Result<String> {
    let n = v.as_u64().filter(|n| *n > 0).ok_or(Error::Invalid)?;
    Ok(n.to_string())
}
fn number(row: &Value, key: &str, scale: u64, ceil: bool) -> Result<u64> {
    let n = row
        .get(key)
        .and_then(Value::as_number)
        .ok_or(Error::Invalid)?;
    scaled_decimal(&n.to_string(), scale, ceil)
}
/// Exact decimal/exponent arithmetic. Prices round UP; capacities and bandwidth DOWN.
/// Deliberately never parses an f64 (serde_json arbitrary_precision preserves tokens).
fn scaled_decimal(text: &str, scale: u64, ceil: bool) -> Result<u64> {
    if text.len() > 128 || text.starts_with('-') {
        return Err(Error::Invalid);
    }
    let mut split = text.split(['e', 'E']);
    let mantissa = split.next().ok_or(Error::Invalid)?;
    let exponent = split
        .next()
        .map(|e| e.parse::<i32>().map_err(|_| Error::Invalid))
        .transpose()?
        .unwrap_or(0);
    if split.next().is_some() || !(-128..=128).contains(&exponent) {
        return Err(Error::Invalid);
    }
    let fraction = mantissa.split_once('.').map(|(_, f)| f.len()).unwrap_or(0);
    let digits = mantissa.replace('.', "");
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::Invalid);
    }
    let value = digits.parse::<u128>().map_err(|_| Error::Invalid)?;
    let numerator = value.checked_mul(scale as u128).ok_or(Error::Invalid)?;
    let power = exponent - i32::try_from(fraction).map_err(|_| Error::Invalid)?;
    let scaled = if power >= 0 {
        numerator
            .checked_mul(10u128.checked_pow(power as u32).ok_or(Error::Invalid)?)
            .ok_or(Error::Invalid)?
    } else if power < -38 {
        if ceil && numerator > 0 { 1 } else { 0 }
    } else {
        let divisor = 10u128.pow((-power) as u32);
        if ceil {
            numerator.div_ceil(divisor)
        } else {
            numerator / divisor
        }
    };
    scaled.try_into().map_err(|_| Error::Invalid)
}

// Validate uniqueness before using a general-purpose JSON Value. This visitor
// retains no values and emits no provider content in public error messages.
struct UniqueKeys;
impl<'de> serde::Deserialize<'de> for UniqueKeys {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueKeys;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("unambiguous JSON")
            }
            fn visit_bool<E: serde::de::Error>(
                self,
                _: bool,
            ) -> std::result::Result<UniqueKeys, E> {
                Ok(UniqueKeys)
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> std::result::Result<UniqueKeys, E> {
                Ok(UniqueKeys)
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> std::result::Result<UniqueKeys, E> {
                Ok(UniqueKeys)
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> std::result::Result<UniqueKeys, E> {
                Ok(UniqueKeys)
            }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> std::result::Result<UniqueKeys, E> {
                Ok(UniqueKeys)
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<UniqueKeys, E> {
                Ok(UniqueKeys)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<UniqueKeys, A::Error> {
                while seq.next_element::<UniqueKeys>()?.is_some() {}
                Ok(UniqueKeys)
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<UniqueKeys, A::Error> {
                let mut seen = BTreeSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !seen.insert(key) {
                        return Err(serde::de::Error::custom("duplicate JSON key"));
                    }
                    map.next_value::<UniqueKeys>()?;
                }
                Ok(UniqueKeys)
            }
        }
        d.deserialize_any(Visitor)
    }
}

#[cfg(test)]
mod decimal_tests {
    use super::*;
    #[test]
    fn prices_ceil_capacities_floor_without_floats() {
        for (s, scale, up, want) in [
            ("0.1234567", 1_000_000, true, 123457),
            ("0.1234567", 1_000_000, false, 123456),
            ("1e-7", 1_000_000, true, 1),
            ("1e-7", 1_000_000, false, 0),
            ("1.2e3", 1, false, 1200),
            ("0", 1, true, 0),
            ("1e-128", 1, true, 1),
            ("18446744073709551615", 1, false, u64::MAX),
        ] {
            assert_eq!(scaled_decimal(s, scale, up).unwrap(), want);
        }
        for bad in [
            "-0.1",
            "1e129",
            "1e40",
            "18446744073709551616",
            "not-a-number",
        ] {
            assert!(scaled_decimal(bad, 1, true).is_err());
        }
    }
}
