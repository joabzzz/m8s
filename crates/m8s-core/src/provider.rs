//! Provider contracts only; implementations live outside the policy crate.
use crate::{Instance, Observation, Offer};

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
