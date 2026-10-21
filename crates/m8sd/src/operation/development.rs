//! Explicitly sealed synthetic backend. No Provider, callback, executable,
//! credentials, networking, real watchdog or admission-proof constructors.
use super::*;
use sha2::{Digest, Sha256};

/// Fixed SyntheticV1 plan; immutable, bounded and not deserializable.
pub struct SyntheticPlan {
    namespace: String,
    target: String,
    tier: String,
}
impl SyntheticPlan {
    pub fn new(namespace: &str, target: &str, tier: &str) -> Result<Self> {
        for value in [namespace, target, tier] {
            label(value)?;
            if value.len() > 64 {
                return Err(Error::Invalid);
            }
        }
        Ok(Self {
            namespace: namespace.into(),
            target: target.into(),
            tier: tier.into(),
        })
    }
    /// Hashes all intent identity fields plus all plan fields. The input
    /// launch_fingerprint is the OUTPUT slot, not recursively hashed into itself.
    pub fn fingerprint(&self, intent: &CreateIntent) -> Result<[u8; 32]> {
        let mut h = Sha256::new();
        h.update(b"m8s-development-plan\0");
        h.update(1u16.to_be_bytes());
        for field in [
            "SyntheticV1",
            &self.namespace,
            &self.target,
            &self.tier,
            &intent.owner,
            &intent.run_id,
            &intent.request_id,
            &intent.node_uid,
            &intent.operation_id,
        ] {
            label(field)?;
            h.update((field.len() as u32).to_be_bytes());
            h.update(field.as_bytes());
        }
        Ok(h.finalize().into())
    }
}
pub enum DevelopmentOutcome {
    Bound(Box<CreateOperation>),
    Ambiguous,
    NotInvoked,
}
/// Fake resource metadata, never owned-resource absence or settlement proof.
pub struct SyntheticResource {
    binding: ResourceBinding,
    serving_deadline: i64,
    cleanup_deadline: i64,
    removed: bool,
}
impl SyntheticResource {
    pub fn binding(&self) -> &ResourceBinding {
        &self.binding
    }
    pub fn serving_deadline(&self) -> i64 {
        self.serving_deadline
    }
    pub fn cleanup_deadline(&self) -> i64 {
        self.cleanup_deadline
    }
    pub fn is_removed(&self) -> bool {
        self.removed
    }
    pub fn service_eligible(&self, now: i64) -> bool {
        !self.removed && now >= self.binding.observed_at && now < self.serving_deadline
    }
}
#[derive(Default)]
struct Backend {
    calls: u64,
    resources: Vec<SyntheticResource>,
    clock: Option<i64>,
}
impl Backend {
    fn advance(&mut self, now: i64) -> Result<()> {
        if now < 0 || self.clock.is_some_and(|last| now < last) {
            return Err(Error::Invalid);
        }
        self.clock = Some(now);
        for r in &mut self.resources {
            if now >= r.cleanup_deadline {
                r.removed = true;
            }
        }
        Ok(())
    }
    fn invoke(
        &mut self,
        claim: &CreateClaim,
        now: i64,
        #[cfg(test)] fault: ResponseFault,
    ) -> Result<ResourceBinding> {
        self.calls = self.calls.checked_add(1).ok_or(Error::Invalid)?;
        #[cfg(test)]
        if fault == ResponseFault::BackendError {
            return Err(Error::Invalid);
        }
        let digest = claim.intent.launch_fingerprint;
        let mut hex = String::with_capacity(64);
        for byte in digest {
            use std::fmt::Write;
            write!(&mut hex, "{byte:02x}").map_err(|_| Error::Invalid)?;
        }
        let binding = ResourceBinding {
            owner: claim.intent.owner.clone(),
            run_id: claim.intent.run_id.clone(),
            node_uid: claim.intent.node_uid.clone(),
            instance_id: format!("synthetic-v1-{hex}"),
            volumes: VolumeBinding::Ids(vec![format!("synthetic-volume-v1-{hex}")]),
            provenance: "sealed synthetic backend v1, not provider evidence".into(),
            observed_at: now,
            status: EvidenceStatus::Complete,
        };
        self.resources.push(SyntheticResource {
            binding: binding.clone(),
            serving_deadline: claim.serving_deadline,
            cleanup_deadline: claim.cleanup_deadline,
            removed: false,
        });
        Ok(binding)
    }
}
/// Actual executor allocation identity is bound only while issuing the claim.
/// No public callback/Provider/trait or token attachment is accepted.
/// ```compile_fail
/// use m8sd::operation::development::SyntheticPlan;
/// let plan = SyntheticPlan { namespace: "n".into(), target: "t".into(), tier: "x".into() };
/// ```
/// ```compile_fail
/// use m8sd::operation::CreateClaim;
/// fn rebind(claim: &mut CreateClaim) { claim.driver = None; }
/// ```
/// ```compile_fail
/// use m8sd::operation::CreateClaim;
/// let _: CreateClaim = serde_json::from_str("{}").unwrap();
/// ```
/// ```compile_fail
/// use m8sd::operation::development::SyntheticPlan;
/// let _ = SyntheticPlan::new_with_provider("remote", "n", "t", "tier");
/// ```
pub struct DevelopmentExecutor {
    plan: SyntheticPlan,
    token: Arc<DriverOrigin>,
    backend: Backend,
    #[cfg(test)]
    hooks: Hooks,
}
impl DevelopmentExecutor {
    pub fn new_synthetic(plan: SyntheticPlan) -> Result<Self> {
        Ok(Self {
            plan,
            token: Arc::new(DriverOrigin),
            backend: Backend::default(),
            #[cfg(test)]
            hooks: Hooks::default(),
        })
    }
    pub fn prepare(
        &self,
        store: &Store,
        intent: &CreateIntent,
        now: i64,
    ) -> Result<CreateOperation> {
        if self.plan.fingerprint(intent)? != intent.launch_fingerprint {
            return Err(Error::Invalid);
        }
        self.check_tier(store, intent)?;
        store.prepare_create(intent, now)
    }
    pub fn claim(&self, store: &Store, operation_id: &str, now: i64) -> Result<CreateClaim> {
        let r = get(&store.conn, &store.owner, operation_id)?.ok_or(Error::Invalid)?;
        let fingerprint = self.plan.fingerprint(&r.intent)?;
        store.claim_create_bound(
            operation_id,
            now,
            Some((self.token.clone(), fingerprint, &self.plan.tier)),
        )
    }
    fn check_tier(&self, store: &Store, intent: &CreateIntent) -> Result<()> {
        let tier: String = store.conn.query_row(
            "SELECT tier FROM gpu_node WHERE uid=?",
            [&intent.node_uid],
            |r| r.get(0),
        )?;
        if tier != self.plan.tier {
            return Err(Error::Invalid);
        }
        Ok(())
    }
    fn start_invocation(&self, store: &Store, claim: &CreateClaim, now: i64) -> Result<bool> {
        start_bound_invocation(
            store,
            claim,
            InvocationClock::Explicit(now),
            &self.plan.tier,
            #[cfg(test)]
            self.hooks.before_commit_fail,
        )
    }
    /// Consumes claim on EVERY outcome, including failed validation/persistence.
    /// Cancellation before start wins: zero calls. After commit: possible one
    /// even if cancelled in the gap; cleanup remains owed. No retry returned.
    pub fn execute(
        &mut self,
        store: &Store,
        claim: CreateClaim,
        now: i64,
    ) -> Result<DevelopmentOutcome> {
        if !Arc::ptr_eq(&claim.origin, &store.origin)
            || claim
                .driver
                .as_ref()
                .is_none_or(|token| !Arc::ptr_eq(token, &self.token))
            || self.plan.fingerprint(&claim.intent)? != claim.intent.launch_fingerprint
            || now < 0
            || self.backend.clock.is_some_and(|last| now < last)
        {
            return Err(Error::Invalid);
        }
        if !self.start_invocation(store, &claim, now)? {
            return Ok(DevelopmentOutcome::NotInvoked);
        }
        #[cfg(test)]
        {
            pause(&self.hooks.after_commit, self.backend.calls)?;
            if self.hooks.after_commit_unwind {
                panic!("synthetic after-commit unwind");
            }
        }
        self.backend.advance(now)?;
        let reply = self.backend.invoke(
            &claim,
            now,
            #[cfg(test)]
            self.hooks.response,
        );
        #[cfg(test)]
        let reply = reply.map(|mut binding| {
            if self.hooks.response == ResponseFault::MalformedResponse {
                binding.instance_id.push_str("-wrong-target");
            }
            binding
        });
        #[cfg(test)]
        pause(&self.hooks.after_effect, self.backend.calls)?;
        #[cfg(test)]
        if matches!(self.hooks.response, ResponseFault::LostResponse) {
            let _ = store.record_create_result(
                &claim.intent.operation_id,
                &CreateObservation::Ambiguous,
                now,
            );
            return Ok(DevelopmentOutcome::Ambiguous);
        }
        match reply {
            Ok(binding)
                if self
                    .backend
                    .resources
                    .last()
                    .is_none_or(|r| r.binding != binding) =>
            {
                let _ = store.record_create_result(
                    &claim.intent.operation_id,
                    &CreateObservation::Ambiguous,
                    now,
                );
                Ok(DevelopmentOutcome::Ambiguous)
            }
            Ok(binding) => match store.record_create_result(
                &claim.intent.operation_id,
                &CreateObservation::Bound(binding),
                now,
            ) {
                Ok(r) => Ok(DevelopmentOutcome::Bound(Box::new(r))),
                Err(_) => Ok(DevelopmentOutcome::Ambiguous),
            },
            Err(_) => {
                let _ = store.record_create_result(
                    &claim.intent.operation_id,
                    &CreateObservation::Ambiguous,
                    now,
                );
                Ok(DevelopmentOutcome::Ambiguous)
            }
        }
    }
    pub fn invocations(&self) -> u64 {
        self.backend.calls
    }
    pub fn resources(&self) -> &[SyntheticResource] {
        &self.backend.resources
    }
    /// Injected fake-clock transition ONLY. No real timer/watchdog/absence proof.
    pub fn advance(&mut self, now: i64) -> Result<()> {
        self.backend.advance(now)
    }
}
#[cfg(test)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum ResponseFault {
    #[default]
    None,
    BackendError,
    LostResponse,
    MalformedResponse,
}
#[cfg(test)]
struct Pause {
    entered: std::sync::mpsc::SyncSender<u64>,
    resume: std::sync::mpsc::Receiver<()>,
}
#[cfg(test)]
fn pause(pause: &Option<Pause>, calls: u64) -> Result<()> {
    if let Some(p) = pause {
        p.entered.send(calls).map_err(|_| Error::Invalid)?;
        p.resume
            .recv_timeout(std::time::Duration::from_secs(3))
            .map_err(|_| Error::Invalid)?;
    }
    Ok(())
}
#[cfg(test)]
#[derive(Default)]
struct Hooks {
    before_commit_fail: bool,
    after_commit_unwind: bool,
    response: ResponseFault,
    after_commit: Option<Pause>,
    after_effect: Option<Pause>,
}
#[cfg(test)]
mod tests;
