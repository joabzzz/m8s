//! Provider-independent domain types. Time is Unix seconds, money is micro-USD.
use serde::{Deserialize, Serialize};

pub type Timestamp = i64;
pub type MicroUsd = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Pending,
    Searching,
    Launching,
    Booting,
    Bootstrapped,
    Loading,
    Ready,
    Draining,
    Destroying,
    Gone,
    Failed,
    Orphaned,
    Paused,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuNode {
    pub uid: String,
    pub tier: String,
    pub owner: String,
    pub instance_id: Option<String>,
    pub phase: Phase,
    pub created_at: Timestamp,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub id: String,
    pub country: Option<String>,
    /// Explicit subregion; Ukraine offers must resolve this to exclude occupied regions.
    pub region: Option<String>,
    pub inet_down_mbps: u64,
    pub gpu_count: u32,
    pub vram_bytes_per_gpu: u64,
    pub hourly_micro_usd: MicroUsd,
    pub interruptible: bool,
    pub reliability_bps: u16,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstanceState {
    Starting,
    Running,
    Stopped,
    Exited,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    pub id: String,
    pub owner: String,
    pub node_uid: String,
    pub tier: String,
    pub state: InstanceState,
}
/// Only Absent is positive confirmation. Transport errors must remain errors.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Observation {
    Present(Instance),
    Absent,
}

pub mod policy;
