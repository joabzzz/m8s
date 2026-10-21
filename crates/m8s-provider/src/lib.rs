//! No network transport exists in this crate. The mock can persist across daemon restarts.
use m8s_core::{Instance, InstanceState, Observation, Offer};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("provider unavailable; outcome is uncertain")]
    Unavailable,
    #[error("mock storage error")]
    Storage,
    #[error("invalid launch request")]
    Invalid,
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug)]
pub struct LaunchSpec {
    pub owner: String,
    pub node_uid: String,
    pub tier: String,
}
pub trait Provider {
    fn id(&self) -> &'static str;
    fn search(&mut self) -> Result<Vec<Offer>>;
    /// Implementations must key idempotency by node_uid, not invent an intent on retry.
    fn launch(&mut self, offer: &Offer, spec: &LaunchSpec) -> Result<Instance>;
    fn describe(&mut self, id: &str) -> Result<Observation>;
    /// Successful result is a complete owner-scoped snapshot.
    fn list_owned(&mut self, owner: &str) -> Result<Vec<Instance>>;
    /// An acknowledgement is NOT confirmation of absence.
    fn destroy(&mut self, id: &str) -> Result<()>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Search,
    LaunchBefore,
    LaunchAfter,
    Describe,
    List,
    Destroy,
}
#[derive(Default, Serialize, Deserialize)]
struct State {
    next_id: u64,
    instances: BTreeMap<String, Instance>,
}
#[derive(Default)]
pub struct Mock {
    state: State,
    path: Option<PathBuf>,
    pub faults: Vec<Fault>,
    /// If true, acknowledgement does not remove the instance, as with eventual consistency.
    pub defer_destroy: bool,
    pub destroy_calls: Vec<String>,
}
impl Mock {
    pub fn persistent(path: PathBuf) -> Result<Self> {
        let state = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| Error::Storage)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(_) => return Err(Error::Storage),
        };
        Ok(Self {
            state,
            path: Some(path),
            ..Self::default()
        })
    }
    fn save(&self) -> Result<()> {
        if let Some(path) = &self.path {
            let tmp = path.with_extension("tmp");
            let bytes = serde_json::to_vec(&self.state).map_err(|_| Error::Storage)?;
            use std::io::Write;
            let mut f = fs::File::create(&tmp).map_err(|_| Error::Storage)?;
            f.write_all(&bytes).map_err(|_| Error::Storage)?;
            f.sync_all().map_err(|_| Error::Storage)?;
            fs::rename(tmp, path).map_err(|_| Error::Storage)?;
            if let Some(parent) = path.parent() {
                fs::File::open(parent)
                    .and_then(|d| d.sync_all())
                    .map_err(|_| Error::Storage)?;
            }
        }
        Ok(())
    }
    fn fail(&mut self, fault: Fault) -> Result<()> {
        if self.faults.first() == Some(&fault) {
            self.faults.remove(0);
            Err(Error::Unavailable)
        } else {
            Ok(())
        }
    }
    pub fn insert(&mut self, instance: Instance) -> Result<()> {
        self.state.instances.insert(instance.id.clone(), instance);
        self.save()
    }
    pub fn remove(&mut self, id: &str) -> Result<()> {
        self.state.instances.remove(id);
        self.save()
    }
}
impl Provider for Mock {
    fn id(&self) -> &'static str {
        "mock"
    }
    fn search(&mut self) -> Result<Vec<Offer>> {
        self.fail(Fault::Search)?;
        Ok(vec![Offer {
            id: "mock-24g".into(),
            country: Some("US".into()),
            region: Some("US-local".into()),
            inet_down_mbps: 1000,
            gpu_count: 1,
            vram_bytes_per_gpu: 24_000_000_000,
            hourly_micro_usd: 100_000,
            interruptible: false,
            reliability_bps: 10_000,
        }])
    }
    fn launch(&mut self, _: &Offer, spec: &LaunchSpec) -> Result<Instance> {
        self.fail(Fault::LaunchBefore)?;
        if let Some(i) = self
            .state
            .instances
            .values()
            .find(|i| i.node_uid == spec.node_uid && i.owner == spec.owner)
        {
            return Ok(i.clone());
        }
        if spec.owner.is_empty() || spec.node_uid.is_empty() {
            return Err(Error::Invalid);
        }
        self.state.next_id = self.state.next_id.checked_add(1).ok_or(Error::Storage)?;
        let instance = Instance {
            id: format!("mock-{}", self.state.next_id),
            owner: spec.owner.clone(),
            node_uid: spec.node_uid.clone(),
            tier: spec.tier.clone(),
            state: InstanceState::Running,
        };
        self.insert(instance.clone())?;
        self.fail(Fault::LaunchAfter)?;
        Ok(instance)
    }
    fn describe(&mut self, id: &str) -> Result<Observation> {
        self.fail(Fault::Describe)?;
        Ok(self
            .state
            .instances
            .get(id)
            .cloned()
            .map(Observation::Present)
            .unwrap_or(Observation::Absent))
    }
    fn list_owned(&mut self, owner: &str) -> Result<Vec<Instance>> {
        self.fail(Fault::List)?;
        Ok(self
            .state
            .instances
            .values()
            .filter(|i| i.owner == owner)
            .cloned()
            .collect())
    }
    fn destroy(&mut self, id: &str) -> Result<()> {
        self.destroy_calls.push(id.into());
        self.fail(Fault::Destroy)?;
        if !self.defer_destroy {
            self.remove(id)?;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uncertain_launch_is_adoptable_and_idempotent() {
        let mut p = Mock::default();
        let o = p.search().unwrap().remove(0);
        let s = LaunchSpec {
            owner: "owner".into(),
            node_uid: "uid".into(),
            tier: "tier".into(),
        };
        p.faults.push(Fault::LaunchAfter);
        assert!(p.launch(&o, &s).is_err());
        let i = p.launch(&o, &s).unwrap();
        assert_eq!(p.list_owned("owner").unwrap().len(), 1);
        p.defer_destroy = true;
        p.destroy(&i.id).unwrap();
        assert!(matches!(
            p.describe(&i.id).unwrap(),
            Observation::Present(_)
        ));
        p.faults.push(Fault::Describe);
        assert!(p.describe(&i.id).is_err());
    }
}
