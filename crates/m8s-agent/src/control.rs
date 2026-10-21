//! Optional bridge only: no listener, admission gate, or renewed-permission cache.
//! The local target is trusted configuration, never a wire claim or Ready proof.
//!
//! Production evidence cannot be constructed by an agent caller:
//! ```compile_fail
//! use m8s_agent::control::VerifiedHeartbeat;
//! let evidence = VerifiedHeartbeat { sequence: 1, observed_at: 1 };
//! ```
//! Nor can it be reused, even when the first dispatch is refused:
//! ```compile_fail
//! use m8s_agent::{control::VerifiedHeartbeat, supervisor::{Supervisor, TeardownDriver}};
//! fn reuse<D: TeardownDriver>(s: &mut Supervisor<D>, evidence: VerifiedHeartbeat<'_>) {
//!     let _ = s.verified_heartbeat(evidence, 1);
//!     let _ = s.verified_heartbeat(evidence, 1);
//! }
//! ```
use crate::DeadManSwitch;
pub use m8s_bootstrap::{EndpointId, VerifiedHeartbeat};

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("supervisor has no pinned control node")]
    Unbound,
    #[error(transparent)]
    Bootstrap(#[from] m8s_bootstrap::Error),
}

// Private and immutable after start; no public rebinding or per-event node input.
#[derive(Default)]
pub(crate) struct ControlBinding(Option<EndpointId>);
impl ControlBinding {
    pub(crate) fn pinned(node: EndpointId) -> Self {
        Self(Some(node))
    }

    fn with_node<T>(
        &self,
        apply: impl FnOnce(EndpointId) -> Result<T, ControlError>,
    ) -> Result<T, ControlError> {
        apply(self.0.ok_or(ControlError::Unbound)?)
    }

    pub(crate) fn consume(
        &self,
        evidence: VerifiedHeartbeat<'_>,
        now: i64,
        dms: &mut DeadManSwitch,
    ) -> Result<bool, ControlError> {
        self.with_node(|node| {
            // Do not use evidence.node_endpoint() as the expected node. Bootstrap
            // rechecks live ACL/revocation, transport, lease and freshness itself.
            Ok(evidence.consume(node, now, |peer, observed_at| {
                dms.heartbeat(peer, observed_at)
            })?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pure configuration/dispatch helper tests, not manufactured transport
    // evidence or real TLS qualification. No endpoint or socket is constructed.
    #[test]
    fn unbound_never_invokes_callback_or_changes_dms() {
        let mut dms = DeadManSwitch::new("owner".into(), vec![], 0, 10, 100).unwrap();
        let result = ControlBinding::default().with_node(|_| {
            assert!(dms.heartbeat("owner", 9));
            panic!("unbound callback must not run")
        });
        assert!(matches!(result, Err::<(), _>(ControlError::Unbound)));
        assert!(!dms.tick(10).is_empty());
    }

    #[test]
    fn configured_target_is_stable_not_selected_by_event() {
        let node = EndpointId([7; 32]);
        let binding = ControlBinding::pinned(node);
        for _ in 0..2 {
            assert_eq!(binding.with_node(Ok).unwrap(), node);
        }
    }

    #[test]
    fn dms_peer_time_and_latched_deadlines_remain_authoritative() {
        let mut dms = DeadManSwitch::new("owner".into(), vec![], 0, 10, 30).unwrap();
        assert!(!dms.heartbeat("wire-claimed-owner", 9));
        assert!(dms.tick(5).is_empty());
        assert!(!dms.heartbeat("owner", 4));
        // Verified receive time, not a delayed caller's dispatch time, is the
        // renewal timestamp. This pure DMS test grants no production evidence.
        assert!(dms.heartbeat("owner", 6));
        assert!(dms.tick(15).is_empty());
        assert!(!dms.tick(16).is_empty());
        assert!(!dms.heartbeat("owner", 16));
    }
}
