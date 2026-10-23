//! Nebius mapping and fixture mutations. Production HTTPS is strictly read-only.
//! See ../../NEBIUS.md for API evidence and the limits of catalog/absence evidence.
use crate::{Error, LaunchSpec, Provider, Result};
use m8s_core::{Instance, InstanceState, Observation, Offer};
use std::collections::{BTreeMap, BTreeSet};

mod catalog;
#[cfg(feature = "nebius-readonly-https")]
pub mod readonly;
pub mod wire;
use wire::{Page, Vm};

const MAX_PAGES: usize = 64;
const MAX_ROWS: usize = 4096;
const OWNER: &str = "m8s-owner";
const NODE: &str = "m8s-node-uid";
const TIER: &str = "m8s-tier";
const VERSION: &str = "m8s-version";

/// Domain transport, independent of HTTP and gRPC. No Debug/Serialize on bodies.
/// Mutations are only implemented by deterministic fixtures in this milestone.
pub trait Transport {
    fn instances(&mut self, project: &str, token: &str) -> Result<Page<Vm>>;
    fn platforms(&mut self, project: &str, token: &str) -> Result<Page<wire::Platform>>;
    fn quotas(&mut self, project: &str, token: &str) -> Result<Page<wire::Quota>>;
    fn create_fixture(&mut self, _: FixtureLaunch) -> Result<wire::Operation> {
        Err(Error::Unavailable)
    }
    fn delete_fixture(&mut self, _: &str) -> Result<wire::Operation> {
        Err(Error::Unavailable)
    }
}

/// Deliberately incomplete VM create spec: fixture protocol, never a network request.
pub struct FixtureLaunch {
    pub project: String,
    pub platform: String,
    pub preset: String,
    pub labels: BTreeMap<String, String>,
}

pub struct Config {
    pub project: String,
    pub region: String,
    pub owner: String,
}

/// No credential state, Debug or Serialize. Caller must persist launch intent before
/// effects. In-memory suppression supplements that durable intent; it cannot replace it.
pub struct Nebius<T> {
    config: Config,
    transport: T,
    fixture_mutations: bool,
    launches: BTreeMap<String, Attempt>,
}
struct Attempt {
    offer: Offer,
    tier: String,
}
impl<T: Transport> Nebius<T> {
    /// Explicit fixture capability. Never pass a production mutating transport here.
    pub fn fixture_only(config: Config, transport: T) -> Result<Self> {
        Self::new(config, transport, true)
    }
    pub fn read_only(config: Config, transport: T) -> Result<Self> {
        Self::new(config, transport, false)
    }
    fn new(config: Config, transport: T, fixture_mutations: bool) -> Result<Self> {
        if !component(&config.project) || !component(&config.owner) {
            return Err(Error::Invalid);
        }
        catalog::country(&config.region)?;
        Ok(Self {
            config,
            transport,
            fixture_mutations,
            launches: BTreeMap::new(),
        })
    }
    pub fn into_transport(self) -> T {
        self.transport
    }

    fn inventory(&mut self) -> Result<Vec<Vm>> {
        let rows = collect_pages(|token| self.transport.instances(&self.config.project, token))?;
        let mut ids = BTreeSet::new();
        for row in &rows {
            if !component(&row.metadata.id)
                || row.metadata.parent_id != self.config.project
                || !ids.insert(&row.metadata.id)
            {
                return Err(Error::Invalid);
            }
        }
        Ok(rows)
    }
    fn map_vm(&self, row: &Vm) -> Result<Instance> {
        let labels = &row.metadata.labels;
        let label = |key| {
            labels
                .get(key)
                .filter(|s| component(s))
                .cloned()
                .ok_or(Error::Invalid)
        };
        if labels.get(OWNER) != Some(&self.config.owner)
            || labels.get(VERSION).map(String::as_str) != Some("1")
        {
            return Err(Error::Invalid);
        }
        let state = match row.status.state.as_str() {
            "CREATING" | "STARTING" | "UPDATING" => InstanceState::Starting,
            "RUNNING" => InstanceState::Running,
            "STOPPED" => InstanceState::Stopped,
            // DELETING/STOPPING/ERROR are never absence. Unrepresentable states fail closed.
            _ => return Err(Error::Unavailable),
        };
        Ok(Instance {
            id: row.metadata.id.clone(),
            owner: self.config.owner.clone(),
            node_uid: label(NODE)?,
            tier: label(TIER)?,
            state,
        })
    }
    fn reconcile(&self, rows: &[Vm], spec: &LaunchSpec, offer: &Offer) -> Result<Option<Instance>> {
        let mut found = None;
        let (platform, preset) = catalog::resources(&self.config.region, &offer.id)?;
        for row in rows {
            if row.metadata.labels.get(OWNER) == Some(&self.config.owner)
                && row.metadata.labels.get(NODE) == Some(&spec.node_uid)
            {
                let instance = self.map_vm(row)?;
                if instance.tier != spec.tier
                    || row.spec.resources.platform != platform
                    || row.spec.resources.preset != preset
                    || found.is_some()
                {
                    return Err(Error::Invalid);
                }
                found = Some(instance);
            }
        }
        Ok(found)
    }
}
impl<T: Transport> Provider for Nebius<T> {
    fn id(&self) -> &'static str {
        "nebius"
    }
    fn search(&mut self) -> Result<Vec<Offer>> {
        let platforms =
            collect_pages(|token| self.transport.platforms(&self.config.project, token))?;
        let quotas = collect_pages(|token| self.transport.quotas(&self.config.project, token))?;
        catalog::offers(&self.config, platforms, quotas)
    }
    fn launch(&mut self, offer: &Offer, spec: &LaunchSpec) -> Result<Instance> {
        if !self.fixture_mutations {
            return Err(Error::Unavailable);
        }
        if spec.owner != self.config.owner || !component(&spec.node_uid) || !component(&spec.tier) {
            return Err(Error::Invalid);
        }
        catalog::validate_offer(&self.config, offer)?;
        if let Some(prior) = self.launches.get(&spec.node_uid)
            && (prior.offer != *offer || prior.tier != spec.tier)
        {
            return Err(Error::Invalid);
        }
        // Reconcile even on the first call (restart/adoption); never create on list failure.
        let rows = self.inventory()?;
        if let Some(instance) = self.reconcile(&rows, spec, offer)? {
            self.launches
                .entry(spec.node_uid.clone())
                .or_insert_with(|| Attempt {
                    offer: offer.clone(),
                    tier: spec.tier.clone(),
                });
            return Ok(instance);
        }
        if self.launches.contains_key(&spec.node_uid) {
            return Err(Error::Unavailable);
        }
        let (platform, preset) = catalog::resources(&self.config.region, &offer.id)?;
        let request = FixtureLaunch {
            project: self.config.project.clone(),
            platform: platform.into(),
            preset: preset.into(),
            labels: BTreeMap::from([
                (OWNER.into(), spec.owner.clone()),
                (NODE.into(), spec.node_uid.clone()),
                (TIER.into(), spec.tier.clone()),
                (VERSION.into(), "1".into()),
            ]),
        };
        // Mark uncertain before the transport. Every outcome consumes this attempt.
        self.launches.insert(
            spec.node_uid.clone(),
            Attempt {
                offer: offer.clone(),
                tier: spec.tier.clone(),
            },
        );
        let operation = self.transport.create_fixture(request)?;
        operation.validate(None)?;
        // Operation acceptance is not an observed VM. Only inventory can adopt it.
        let rows = self.inventory()?;
        let instance = self
            .reconcile(&rows, spec, offer)?
            .ok_or(Error::Unavailable)?;
        if instance.id != operation.resource_id {
            return Err(Error::Unavailable);
        }
        Ok(instance)
    }
    fn describe(&mut self, id: &str) -> Result<Observation> {
        if !component(id) {
            return Err(Error::Invalid);
        }
        let rows = self.inventory()?;
        match rows.iter().find(|row| row.metadata.id == id) {
            Some(row) => Ok(Observation::Present(self.map_vm(row)?)),
            None => Ok(Observation::Absent),
        }
    }
    fn list_owned(&mut self, owner: &str) -> Result<Vec<Instance>> {
        if owner != self.config.owner {
            return Err(Error::Invalid);
        }
        self.inventory()?
            .iter()
            .filter(|row| row.metadata.labels.get(OWNER) == Some(&self.config.owner))
            .map(|row| self.map_vm(row))
            .collect()
    }
    fn destroy(&mut self, id: &str) -> Result<()> {
        if !self.fixture_mutations {
            return Err(Error::Unavailable);
        }
        match self.describe(id)? {
            Observation::Absent => return Ok(()),
            Observation::Present(_) => {}
        }
        self.transport.delete_fixture(id)?.validate(Some(id))
    }
}

fn component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn collect_pages<T>(mut fetch: impl FnMut(&str) -> Result<Page<T>>) -> Result<Vec<T>> {
    let mut token = String::new();
    let mut seen = BTreeSet::new();
    let mut rows = Vec::new();
    for _ in 0..MAX_PAGES {
        let page = fetch(&token)?;
        rows.extend(page.items);
        if rows.len() > MAX_ROWS {
            return Err(Error::Unavailable);
        }
        let next = page.next_page_token;
        if next.is_empty() {
            return Ok(rows);
        }
        if next.len() > 4096 || !seen.insert(next.clone()) {
            return Err(Error::Unavailable);
        }
        token = next;
    }
    Err(Error::Unavailable)
}
