# ADR-0004 — Open Provider and Harness adapters; Vast community and Vast secure are separate providers

Status: **accepted** (trait shapes are proposed and will be finalised in P1) · Decided by: Admin (direction), Metal-0 (shape) · Date: 2026-10-20

## Context

Vast.ai is the prototype provider, but Admin wants an open framework that other people extend: Lambda, AWS, GCP, Databricks, RunPod and so on. People care about very different things. Some don't care where the hardware is; some care a lot about verified datacenters or confidential computing. Agents will also choose providers, so each adapter must describe itself.

## Decision

- **`Provider` trait** in its own crate (`m8s-provider`). Each adapter is a separate crate (`m8s-provider-vast`). Sketch:

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &ProviderId;                       // "vast-community", "vast-secure"
    fn capabilities(&self) -> ProviderCapabilities;    // see below
    async fn search(&self, req: &OfferQuery) -> Result<Vec<Offer>>;
    async fn launch(&self, offer: &OfferId, spec: &LaunchSpec) -> Result<InstanceHandle>;
    async fn describe(&self, id: &InstanceId) -> Result<InstanceStatus>;
    async fn list_owned(&self, label: &OwnerLabel) -> Result<Vec<InstanceStatus>>; // orphan sweep
    async fn destroy(&self, id: &InstanceId) -> Result<()>;
    fn self_destruct_credential(&self, id: &InstanceId) -> Option<ScopedCredential>; // ADR-0007
}

pub struct ProviderCapabilities {
    zdr_level: ZdrLevel,            // ADR-0011: L0..L3
    interruptible: bool,
    confidential_gpu: bool,
    regions: RegionModel,           // granularity of location info
    udp_inbound: UdpSupport,
    billing: BillingModel,          // per-second / per-hour, storage-when-stopped
    max_gpus_per_host: u8,
    docs: &'static str,             // agent-readable guidance (markdown)
}
```

- **v0 providers:** `vast-community` (unverified hosts) and `vast-secure` (Vast secure cloud / verified datacenters). They share one implementation crate, but they are **distinct provider IDs** with different capabilities and default filters.
- **`Harness` trait** for agent runtimes that consume the mesh: configure a provider pointing at the mesh, run a task, collect a transcript and metrics. v0 harnesses: **Goose SDK** and **pi**.
- Every adapter ships a short **agent-readable doc**. It covers when to use the adapter, cost profile, privacy level and interruptibility, so an operator agent can choose sensibly.
- **Default: non-interruptible.** Interruptible (bid) capacity is opt-in per tier.

## Consequences

- The core never imports Vast types. Provider-specific knobs go in an opaque `extra` map that each adapter validates.
- Field names will be adjusted once [Spike S1](agor://kb/m8s/research/spike-s1-vast-provider-and-costs.md) reports on the real Vast API.
- Third parties can add a provider without forking m8s. Discovery is compile-time (cargo features) in v0; dynamic plugins come later if needed.
