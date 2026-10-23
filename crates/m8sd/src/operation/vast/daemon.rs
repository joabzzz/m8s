//! Control-socket semantics of the real-provider recovery daemon. It owns one
//! authoritative Store and reconciles only persisted managed operations. Launch
//! is not composed here: `up`/scale-up/replacement are refused, and nothing
//! reports Ready. The default mock daemon never reaches this code.
use super::*;
use m8s_api::{Request, Response};

const REFUSED: &str = "vast recovery daemon: launch not composed; request refused, no effect";
const UNAVAILABLE: &str = "snapshot unavailable; outcome uncertain; empty fields are not absence";

pub struct VastDaemon<P> {
    store: Store,
    port: P,
    trigger: CleanupTrigger,
}
impl<P: RestartPort> VastDaemon<P> {
    /// Fails closed on a mock/legacy/empty database: this mode never creates a
    /// managed domain, only resumes one that a prior coordinator established.
    pub fn new(store: Store, port: P, trigger: CleanupTrigger) -> Result<Self> {
        store.managed_control_snapshot()?;
        Ok(Self {
            store,
            port,
            trigger,
        })
    }
    pub fn store(&self) -> &Store {
        &self.store
    }
    /// One reconcile pass. The report, not an empty result, is the only signal.
    pub fn tick(&mut self, clock: impl FnMut() -> Result<i64>) -> Result<Vec<OperationReport>> {
        self.store
            .reconcile_vast(&mut self.port, self.trigger, clock)
    }
    pub fn handle(&mut self, request: Request) -> Response {
        let accepted = match request {
            Request::Status {} | Request::Logs {} => true,
            Request::Down {} => self.store.down().is_ok(),
            // Known tier to zero is a down request; a positive count would be a
            // launch, which this mode does not compose.
            Request::Scale { tier, replicas: 0 } => self.store.scale(&tier, 0).is_ok(),
            _ => false,
        };
        self.response(accepted)
    }
    fn response(&self, ok: bool) -> Response {
        let mut response = Response {
            ok,
            mode: m8s_api::VAST_DEMO_MODE.into(),
            nodes: vec![],
            tiers: vec![],
            events: vec![],
            error: (!ok).then(|| REFUSED.into()),
            secure_bootstrap: m8s_api::VAST_DEMO_SECURITY.into(),
        };
        match self.store.managed_control_snapshot() {
            Ok(snapshot) => {
                response.nodes = snapshot.nodes;
                response.tiers = snapshot.tiers;
                response.events = snapshot.events;
            }
            Err(_) => {
                response.ok = false;
                response.error = Some(UNAVAILABLE.into());
            }
        }
        response
    }
}

#[cfg(test)]
mod tests;
