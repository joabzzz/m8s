//! Bounded schema1 owner intent only: no cron/autoscaling/providers/readiness.
//! Caller supplies bytes/path AND model catalog; no discovery or interpolation.
use m8s_core::MicroUsd;
use m8s_models::ModelCatalog;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::Path,
};

pub const MAX_BYTES: usize = 65_536;
pub const MAX_TIERS: usize = 32;
pub const MAX_MODELS: usize = 256;
pub const MAX_DURATION_SECONDS: u64 = 31 * 86_400;
pub const EXAMPLE: &str = include_str!("../examples/m8s.toml");

/// No user input, path, raw TOML errors or error sources escape this boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("configuration exceeds bounded size")]
    TooLarge,
    #[error("invalid configuration syntax or schema")]
    Syntax,
    #[error("invalid configuration policy or reference")]
    Invalid,
    #[error("invalid supplied model catalog")]
    Catalog,
    #[error("configuration file unavailable")]
    Io,
    #[error("explicit regular configuration file required")]
    UnsafePath,
}
pub type Result<T> = std::result::Result<T, Error>;
fn ensure(ok: bool) -> Result<()> {
    if ok { Ok(()) } else { Err(Error::Invalid) }
}
fn id(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.as_bytes()[0].is_ascii_lowercase()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_.".contains(&b))
}

/// Trusted caller-injected administrative envelope, NOT approval evidence.
/// Defaults reflect the current conservative environment, not immutable product
/// limits. Parsing never grants live-test permission or reserves/spends money.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValidationLimits {
    pub maximum_monthly_micro_usd: MicroUsd,
    pub maximum_total_hourly_micro_usd: MicroUsd,
    pub maximum_instance_hourly_micro_usd: MicroUsd,
}
impl Default for ValidationLimits {
    fn default() -> Self {
        Self {
            maximum_monthly_micro_usd: 20_000_000,
            maximum_total_hourly_micro_usd: 3_000_000,
            maximum_instance_hourly_micro_usd: 3_000_000,
        }
    }
}

/// Validated immutable document; no Deserialize implementation bypasses validation.
/// Catalog source/license/runtime data is not retained or echoed by Config.
/// ```
/// let catalog: m8s_models::ModelCatalog = toml::from_str(m8s_models::EXAMPLE_CATALOG).unwrap();
/// let config = m8s_config::Config::parse(m8s_config::EXAMPLE.as_bytes(), &catalog,
///     &m8s_config::ValidationLimits::default()).unwrap();
/// assert_eq!(config.tiers().count(), 2); // intent only, no effects/qualification
/// ```
/// ```compile_fail
/// let _: m8s_config::Config = toml::from_str("schema=1").unwrap();
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    document: Document,
}
impl Config {
    pub fn parse(bytes: &[u8], catalog: &ModelCatalog, limits: &ValidationLimits) -> Result<Self> {
        if bytes.len() > MAX_BYTES {
            return Err(Error::TooLarge);
        }
        let text = std::str::from_utf8(bytes).map_err(|_| Error::Syntax)?;
        let document: Document = toml::from_str(text).map_err(|_| Error::Syntax)?;
        validate(&document, catalog, limits)?;
        let config = Self { document };
        config.to_toml()?; // All accepted documents have a bounded canonical round trip.
        Ok(config)
    }
    /// Final component must be a non-symlink regular file on Unix. Parent path
    /// resolution is caller-trusted; no path/env/catalog discovery is performed.
    pub fn from_path(
        path: &Path,
        catalog: &ModelCatalog,
        limits: &ValidationLimits,
    ) -> Result<Self> {
        Self::parse(&read_bounded(path)?, catalog, limits)
    }
    pub fn to_toml(&self) -> Result<String> {
        let output = toml::to_string(&self.document).map_err(|_| Error::Invalid)?;
        if output.len() > MAX_BYTES {
            return Err(Error::TooLarge);
        }
        Ok(output)
    }
    pub fn schema(&self) -> u32 {
        self.document.schema
    }
    pub fn budget(&self) -> &Budget {
        &self.document.budget
    }
    pub fn dms(&self) -> Dms {
        Dms {
            ttl_seconds: duration(&self.document.dead_mans_switch.ttl).expect("validated duration"),
            max_lifetime_seconds: duration(&self.document.dead_mans_switch.max_lifetime)
                .expect("validated duration"),
        }
    }
    pub fn minimum_down_mbps(&self) -> u64 {
        self.document.policy.minimum_down_mbps
    }
    /// Requirements, NOT verified runtime/image/location evidence.
    pub fn signed_image_required(&self) -> bool {
        true
    }
    pub fn unknown_location_rejected(&self) -> bool {
        true
    }
    pub fn timezone(&self) -> Option<&str> {
        self.document.owner.as_ref().map(|o| o.timezone.as_str())
    }
    pub fn tiers(&self) -> impl Iterator<Item = (&str, Tier<'_>)> {
        self.document
            .tier
            .iter()
            .map(|(k, v)| (k.as_str(), Tier { spec: v }))
    }
    pub fn tier(&self, name: &str) -> Option<Tier<'_>> {
        self.document.tier.get(name).map(|spec| Tier { spec })
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema: u32,
    #[serde(default)]
    budget: Budget,
    #[serde(default)]
    dead_mans_switch: DmsSpec,
    #[serde(default)]
    policy: Policy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner: Option<Owner>,
    tier: BTreeMap<String, TierSpec>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Budget {
    pub monthly_micro_usd: MicroUsd,
    pub max_total_hourly_micro_usd: MicroUsd,
    pub max_instance_hourly_micro_usd: MicroUsd,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            monthly_micro_usd: 20_000_000,
            max_total_hourly_micro_usd: 3_000_000,
            max_instance_hourly_micro_usd: 1_000_000,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dms {
    pub ttl_seconds: u64,
    pub max_lifetime_seconds: u64,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DmsSpec {
    ttl: String,
    max_lifetime: String,
}
impl Default for DmsSpec {
    fn default() -> Self {
        Self {
            ttl: "1h".into(),
            max_lifetime: "12h".into(),
        }
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Policy {
    minimum_down_mbps: u64,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            minimum_down_mbps: 1000,
        }
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Owner {
    timezone: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderPreference {
    #[serde(rename = "vast-secure")]
    VastSecure,
    #[serde(rename = "vast-community")]
    VastCommunity,
}
fn providers() -> Vec<ProviderPreference> {
    vec![
        ProviderPreference::VastSecure,
        ProviderPreference::VastCommunity,
    ]
}
fn yes() -> bool {
    true
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TierSpec {
    model: String,
    #[serde(default = "providers")]
    provider: Vec<ProviderPreference>,
    #[serde(default)]
    replicas: Replicas,
    #[serde(default)]
    gpu: Gpu,
    #[serde(default)]
    cold: ColdSpec,
    #[serde(default)]
    interruptible: bool,
    #[serde(default = "yes")]
    manual: bool,
    #[serde(default)]
    schedule: Vec<ScheduleSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    queue: Option<QueueSpec>,
}
/// Immutable view available only through validated Config; cannot deserialize.
#[derive(Clone, Copy)]
pub struct Tier<'a> {
    spec: &'a TierSpec,
}
impl<'a> Tier<'a> {
    pub fn model(&self) -> &'a str {
        &self.spec.model
    }
    pub fn providers(&self) -> &'a [ProviderPreference] {
        &self.spec.provider
    }
    pub fn replicas(&self) -> Replicas {
        self.spec.replicas
    }
    pub fn gpu(&self) -> Gpu {
        self.spec.gpu
    }
    pub fn interruptible(&self) -> bool {
        self.spec.interruptible
    }
    pub fn manual(&self) -> bool {
        self.spec.manual
    }
    pub fn schedules(&self) -> impl ExactSizeIterator<Item = Schedule<'a>> + 'a {
        let specs: &'a [ScheduleSpec] = &self.spec.schedule;
        specs.iter().map(|spec| Schedule { spec })
    }
    pub fn queue(&self) -> Option<Queue<'a>> {
        self.spec.queue.as_ref().map(|spec| Queue { spec })
    }
    pub fn cold(&self) -> ColdBehavior<'a> {
        match self.spec.cold.mode {
            ColdMode::Wait => ColdBehavior::Wait {
                timeout_seconds: duration(
                    self.spec
                        .cold
                        .timeout
                        .as_deref()
                        .expect("validated timeout"),
                )
                .expect("validated duration"),
            },
            ColdMode::Reject => ColdBehavior::Reject,
            ColdMode::Fallback => ColdBehavior::Fallback {
                tier: self.spec.cold.tier.as_deref().expect("validated fallback"),
            },
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Replicas {
    pub min: u8,
    pub max: u8,
}
impl Default for Replicas {
    fn default() -> Self {
        Self { min: 0, max: 1 }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Gpu {
    pub count: u8,
    pub min_vram_bytes: u64,
}
impl Default for Gpu {
    fn default() -> Self {
        Self {
            count: 1,
            min_vram_bytes: 1,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColdBehavior<'a> {
    Wait { timeout_seconds: u64 },
    Reject,
    Fallback { tier: &'a str },
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ColdMode {
    Wait,
    Reject,
    Fallback,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ColdSpec {
    mode: ColdMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tier: Option<String>,
}
impl Default for ColdSpec {
    fn default() -> Self {
        Self {
            mode: ColdMode::Wait,
            timeout: Some("15m".into()),
            tier: None,
        }
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScheduleSpec {
    open: String,
    close: String,
}
#[derive(Clone, Copy)]
pub struct Schedule<'a> {
    spec: &'a ScheduleSpec,
}
impl<'a> Schedule<'a> {
    pub fn open(&self) -> &'a str {
        &self.spec.open
    }
    pub fn close(&self) -> &'a str {
        &self.spec.close
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct QueueSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    queue_wait_ms_gt: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    inflight_gt: Option<u64>,
    scale_up_for: String,
    scale_down_idle: String,
}
#[derive(Clone, Copy)]
pub struct Queue<'a> {
    spec: &'a QueueSpec,
}
impl Queue<'_> {
    pub fn queue_wait_ms_gt(&self) -> Option<u64> {
        self.spec.queue_wait_ms_gt
    }
    pub fn inflight_gt(&self) -> Option<u64> {
        self.spec.inflight_gt
    }
    pub fn scale_up_for_seconds(&self) -> u64 {
        duration(&self.spec.scale_up_for).expect("validated duration")
    }
    pub fn scale_down_idle_seconds(&self) -> u64 {
        duration(&self.spec.scale_down_idle).expect("validated duration")
    }
}

fn validate(doc: &Document, catalog: &ModelCatalog, limits: &ValidationLimits) -> Result<()> {
    ensure(doc.schema == 1 && !doc.tier.is_empty() && doc.tier.len() <= MAX_TIERS)?;
    let budget = &doc.budget;
    ensure(limits.maximum_instance_hourly_micro_usd <= limits.maximum_total_hourly_micro_usd)?;
    ensure(
        budget.monthly_micro_usd <= limits.maximum_monthly_micro_usd
            && budget.max_total_hourly_micro_usd <= limits.maximum_total_hourly_micro_usd
            && budget.max_instance_hourly_micro_usd <= limits.maximum_instance_hourly_micro_usd
            && budget.max_instance_hourly_micro_usd <= budget.max_total_hourly_micro_usd,
    )?;
    ensure(duration(&doc.dead_mans_switch.max_lifetime)? >= duration(&doc.dead_mans_switch.ttl)?)?;
    ensure((1000..=1_000_000).contains(&doc.policy.minimum_down_mbps))?;
    if let Some(owner) = &doc.owner {
        ensure(owner.timezone.len() <= 64 && owner.timezone.parse::<chrono_tz::Tz>().is_ok())?;
    }
    if catalog.models.is_empty() || catalog.models.len() > MAX_MODELS {
        return Err(Error::Catalog);
    }
    let mut models = BTreeSet::new();
    for model in &catalog.models {
        if !id(&model.id, 96)
            || !models.insert(model.id.as_str())
            || model.weights_bytes == 0
            || model
                .weights_bytes
                .checked_add(model.runtime_overhead_bytes_per_replica)
                .is_none()
            || model.kv_bytes_per_1024_tokens == 0
            || model.max_context_tokens == 0
            || model.params_billions == 0
            || model.source.is_empty()
            || model.source.len() > 2048
            || model.source.chars().any(char::is_control)
            || model.quant.is_empty()
            || model.quant.len() > 64
            || model.license.is_empty()
            || model.license.len() > 128
            || !matches!(
                model.license_class.as_str(),
                "permissive" | "restricted" | "non-commercial" | "unknown"
            )
            || !matches!(
                model.runtime_status.as_str(),
                "certified" | "community" | "untested"
            )
        {
            return Err(Error::Catalog);
        }
    }
    for (name, tier) in &doc.tier {
        ensure(id(name, 48) && id(&tier.model, 96) && models.contains(tier.model.as_str()))?;
        ensure(
            !tier.provider.is_empty()
                && tier.provider.len() <= 2
                && (tier.provider.len() != 2 || tier.provider[0] != tier.provider[1]),
        )?;
        ensure(tier.replicas.min <= tier.replicas.max && tier.replicas.max <= 16)?;
        ensure(
            (1..=16).contains(&tier.gpu.count)
                && (1..=1_000_000_000_000).contains(&tier.gpu.min_vram_bytes),
        )?;
        match tier.cold.mode {
            ColdMode::Wait => {
                duration(tier.cold.timeout.as_deref().ok_or(Error::Invalid)?)?;
                ensure(tier.cold.tier.is_none())?;
            }
            ColdMode::Reject => ensure(tier.cold.timeout.is_none() && tier.cold.tier.is_none())?,
            ColdMode::Fallback => {
                let target = tier.cold.tier.as_deref().ok_or(Error::Invalid)?;
                ensure(
                    id(target, 48) && doc.tier.contains_key(target) && tier.cold.timeout.is_none(),
                )?;
            }
        }
        ensure(tier.schedule.len() <= 16 && (tier.schedule.is_empty() || doc.owner.is_some()))?;
        for schedule in &tier.schedule {
            cron(&schedule.open)?;
            cron(&schedule.close)?;
            ensure(schedule.open != schedule.close)?;
        }
        if let Some(queue) = &tier.queue {
            ensure(queue.queue_wait_ms_gt.is_some() || queue.inflight_gt.is_some())?;
            if let Some(n) = queue.queue_wait_ms_gt {
                ensure((1..=86_400_000).contains(&n))?;
            }
            if let Some(n) = queue.inflight_gt {
                ensure((1..=1_000_000).contains(&n))?;
            }
            duration(&queue.scale_up_for)?;
            duration(&queue.scale_down_idle)?;
        }
    }
    for start in doc.tier.keys() {
        let mut visited = BTreeSet::new();
        let mut current = start.as_str();
        loop {
            ensure(visited.insert(current))?;
            let cold = &doc.tier.get(current).ok_or(Error::Invalid)?.cold;
            if cold.mode != ColdMode::Fallback {
                break;
            }
            current = cold.tier.as_deref().ok_or(Error::Invalid)?;
        }
    }
    Ok(())
}
/// Positive bounded integer s/m/h/d, no fractional/compound/implicit units/off.
pub fn duration(text: &str) -> Result<u64> {
    ensure((2..=20).contains(&text.len()) && text.is_ascii())?;
    let (digits, unit) = text.split_at(text.len() - 1);
    ensure(
        digits.bytes().all(|b| b.is_ascii_digit()) && (digits == "0" || !digits.starts_with('0')),
    )?;
    let factor = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => return Err(Error::Invalid),
    };
    let result = digits
        .parse::<u64>()
        .map_err(|_| Error::Invalid)?
        .checked_mul(factor)
        .ok_or(Error::Invalid)?;
    ensure((1..=MAX_DURATION_SECONDS).contains(&result))?;
    Ok(result)
}
/// Exact ASCII decimal USD: cost rounds UP; limit rounds DOWN. No f64/exponents.
pub fn usd_cost_to_micro_usd(text: &str) -> Result<MicroUsd> {
    decimal(text, true)
}
pub fn usd_limit_to_micro_usd(text: &str) -> Result<MicroUsd> {
    decimal(text, false)
}
fn decimal(text: &str, ceil: bool) -> Result<u64> {
    ensure(!text.is_empty() && text.len() <= 40)?;
    let mut parts = text.split('.');
    let whole = parts.next().ok_or(Error::Invalid)?;
    let fraction = parts.next();
    ensure(parts.next().is_none())?;
    ensure(
        !whole.is_empty()
            && whole.bytes().all(|b| b.is_ascii_digit())
            && (whole == "0" || !whole.starts_with('0')),
    )?;
    let mut micro = whole
        .parse::<u128>()
        .map_err(|_| Error::Invalid)?
        .checked_mul(1_000_000)
        .ok_or(Error::Invalid)?;
    if let Some(fraction) = fraction {
        ensure(
            !fraction.is_empty()
                && fraction.len() <= 18
                && fraction.bytes().all(|b| b.is_ascii_digit()),
        )?;
        let n = fraction.parse::<u128>().map_err(|_| Error::Invalid)? * 1_000_000;
        let divisor = 10u128.pow(fraction.len() as u32);
        micro = micro
            .checked_add(if ceil {
                n.div_ceil(divisor)
            } else {
                n / divisor
            })
            .ok_or(Error::Invalid)?;
    }
    micro.try_into().map_err(|_| Error::Invalid)
}
fn cron(text: &str) -> Result<()> {
    ensure(!text.is_empty() && text.len() <= 128 && text.is_ascii())?;
    let fields: Vec<_> = text.split(' ').collect();
    ensure(fields.len() == 5)?;
    for (field, (min, max)) in fields
        .into_iter()
        .zip([(0, 59), (0, 23), (1, 31), (1, 12), (0, 6)])
    {
        ensure(!field.is_empty())?;
        for item in field.split(',') {
            let mut step = item.split('/');
            let range = step.next().ok_or(Error::Invalid)?;
            if let Some(n) = step.next() {
                ensure(range == "*" || range.contains('-'))?;
                let n = cron_number(n)?;
                ensure(n > 0 && n <= max - min + 1)?;
            }
            ensure(step.next().is_none())?;
            if range == "*" {
                continue;
            }
            let mut ends = range.split('-');
            let first = cron_number(ends.next().ok_or(Error::Invalid)?)?;
            let last = ends.next().map(cron_number).transpose()?.unwrap_or(first);
            ensure(ends.next().is_none() && min <= first && first <= last && last <= max)?;
        }
    }
    Ok(())
}
fn cron_number(s: &str) -> Result<u64> {
    ensure(!s.is_empty() && s.len() <= 2 && s.bytes().all(|b| b.is_ascii_digit()))?;
    s.parse().map_err(|_| Error::Invalid)
}
#[cfg(unix)]
fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| Error::Io)?;
    let info = file.metadata().map_err(|_| Error::Io)?;
    if !info.is_file() {
        return Err(Error::UnsafePath);
    }
    if info.len() > MAX_BYTES as u64 {
        return Err(Error::TooLarge);
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Io)?;
    if bytes.len() > MAX_BYTES {
        return Err(Error::TooLarge);
    }
    Ok(bytes)
}
#[cfg(not(unix))]
fn read_bounded(_: &Path) -> Result<Vec<u8>> {
    Err(Error::UnsafePath)
}
