//! Live supervision after a transport-minted bootstrap receipt. No endpoint
//! creation, provider credentials, or readiness assertion. The external resource
//! owner must retain cleanup custody even if this future is cancelled or fails.
use super::{Error, MeshLaunch, NodeMaterial, Result};
use crate::{
    DeadManSwitch,
    supervisor::{
        BootGate, LocalTerminationState, RetryTeardown, Supervisor, TeardownDriver, TeardownState,
    },
};
use iroh::Endpoint;
use m8s_bootstrap::{
    EndpointId,
    mp001::Mp001CertificateVerifier,
    transport::{ControlPolicy, VerifiedBootstrap, accept_control},
};
use std::{cell::RefCell, future::Future, path::Path, time::Duration};
use tokio::time::Instant;

mod bootstrap;
pub use bootstrap::{accept_owner_and_run, redeem_and_run};

pub struct RuntimePlan<'a> {
    pub material_root: &'a Path,
    pub launch: MeshLaunch,
    pub verifier: &'a Mp001CertificateVerifier,
    pub max_lifetime_seconds: i64,
    pub native_id_timeout: Duration,
    pub cleanup_timeout: Duration,
}

/// Local child state and externally confirmed resource state are independent.
#[derive(Debug, PartialEq, Eq)]
pub struct RuntimeExit {
    pub resource: TeardownState,
    pub child: LocalTerminationState,
}

struct ReceiptGate(bool);
impl BootGate for ReceiptGate {
    fn authorize(&mut self) -> bool {
        std::mem::take(&mut self.0)
    }
}

fn validate_plan(endpoint: &Endpoint, plan: &RuntimePlan<'_>) -> Result<()> {
    if endpoint.is_closed()
        || EndpointId(*endpoint.id().as_bytes()) != plan.launch.node
        || plan.cleanup_timeout.is_zero()
        || plan.cleanup_timeout > Duration::from_secs(60)
        || plan.native_id_timeout.is_zero()
        || plan.native_id_timeout > Duration::from_secs(60)
        || plan.max_lifetime_seconds <= 0
        || plan.launch.heartbeat_ttl_seconds == 0
        || i64::try_from(plan.launch.heartbeat_ttl_seconds).is_err()
    {
        return Err(Error::Policy);
    }
    Ok(())
}

fn endpoint_text(id: EndpointId) -> Result<String> {
    iroh::PublicKey::from_bytes(&id.0)
        .map(|id| id.to_string())
        .map_err(|_| Error::Policy)
}

fn lifetime(now: i64, expires: i64, requested: i64) -> Result<i64> {
    if now < 0 || requested <= 0 {
        return Err(Error::Policy);
    }
    expires
        .checked_sub(now)
        .filter(|n| *n > 0)
        .map(|n| n.min(requested))
        .ok_or(Error::Policy)
}

// Monotonic elapsed time prevents wall-clock rollback from extending a lease;
// a forward wall jump or clock failure can only shorten it.
fn observed_time(start: i64, elapsed: u64, wall: i64) -> i64 {
    if wall < 0 {
        return i64::MAX;
    }
    start
        .saturating_add(i64::try_from(elapsed).unwrap_or(i64::MAX))
        .max(wall)
}

struct RuntimeClock<C> {
    start: i64,
    instant: Instant,
    wall: C,
}
impl<C: Fn() -> i64> RuntimeClock<C> {
    fn now(&self) -> i64 {
        observed_time(self.start, self.instant.elapsed().as_secs(), (self.wall)())
    }
}

/// Consume real receipt provenance, check its endpoint against the live control
/// endpoint and native key, then supervise the enforcing native child. The
/// driver must implement bounded, nonblocking scoped cleanup; this API does not
/// manufacture provider destruction authority. It never returns Ready.
pub async fn run<D: TeardownDriver>(
    endpoint: &Endpoint,
    receipt: VerifiedBootstrap,
    plan: RuntimePlan<'_>,
    driver: D,
    shutdown: impl Future<Output = ()>,
    wall_clock: impl Fn() -> i64,
) -> Result<RuntimeExit> {
    validate_plan(endpoint, &plan)?;
    if EndpointId(*endpoint.id().as_bytes()) != receipt.node() {
        return Err(Error::Policy);
    }
    tokio::pin!(shutdown);
    tokio::select! { biased; () = &mut shutdown => return Err(Error::Policy), () = std::future::ready(()) => {} }
    let bundle = receipt.into_bundle();
    let expires = bundle.claims.expires_at;
    let owner = plan.launch.owner;
    let ttl = i64::try_from(plan.launch.heartbeat_ttl_seconds).map_err(|_| Error::Policy)?;
    let peers = bundle
        .acl
        .endpoints()
        .map(endpoint_text)
        .collect::<Result<Vec<_>>>()?;
    let material = NodeMaterial::stage(
        plan.material_root,
        bundle,
        plan.launch,
        plan.verifier,
        wall_clock(),
    )?;
    material.check_native_node_id(plan.verifier, wall_clock(), plan.native_id_timeout)?;
    let instant = Instant::now();
    let now = wall_clock();
    let max_lifetime = lifetime(now, expires, plan.max_lifetime_seconds)?;
    let policy = ControlPolicy::from_acl(
        owner,
        material.control_acl(),
        now.saturating_add(max_lifetime),
    )
    .map_err(|_| Error::Policy)?;
    let dms = DeadManSwitch::new(endpoint_text(owner)?, peers, now, ttl, max_lifetime)
        .map_err(|_| Error::Policy)?;
    // Signals arriving during bounded native preflight must prevent spawn.
    tokio::select! { biased; () = &mut shutdown => return Err(Error::Policy), () = std::future::ready(()) => {} }
    if endpoint.is_closed() {
        return Err(Error::Policy);
    }
    let supervisor = Supervisor::start_mesh(
        material,
        &mut ReceiptGate(true),
        dms,
        driver,
        RetryTeardown::new(4).map_err(|_| Error::Policy)?,
        plan.verifier,
        now,
    )
    .map_err(|_| Error::Policy)?;
    let clock = RuntimeClock {
        start: now,
        instant,
        wall: wall_clock,
    };
    supervise(
        endpoint,
        owner,
        &policy,
        supervisor,
        &mut shutdown,
        &clock,
        plan.cleanup_timeout,
    )
    .await
}

async fn control_loop<D: TeardownDriver, C: Fn() -> i64>(
    endpoint: &Endpoint,
    owner: EndpointId,
    policy: &ControlPolicy,
    supervisor: &RefCell<Supervisor<D>>,
    clock: &RuntimeClock<C>,
) {
    loop {
        if let Ok(mut session) = accept_control(endpoint, owner, policy, clock.now()).await {
            while let Ok(evidence) = session.receive_heartbeat(|| clock.now()).await {
                if !supervisor
                    .borrow_mut()
                    .verified_heartbeat(evidence, clock.now())
                    .unwrap_or(false)
                {
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn supervise<D: TeardownDriver, C: Fn() -> i64>(
    endpoint: &Endpoint,
    owner: EndpointId,
    policy: &ControlPolicy,
    supervisor: Supervisor<D>,
    shutdown: impl Future<Output = ()>,
    clock: &RuntimeClock<C>,
    cleanup_timeout: Duration,
) -> Result<RuntimeExit> {
    let supervisor = RefCell::new(supervisor);
    // Keep the entire control future pinned across ticks. Cancelling/recreating
    // receive_heartbeat every tick would lose partially received frames.
    let controls = control_loop(endpoint, owner, policy, &supervisor, clock);
    monitor(
        controls,
        shutdown,
        || supervisor.borrow_mut().shutdown(),
        || {
            let mut s = supervisor.borrow_mut();
            if endpoint.is_closed() {
                s.shutdown();
            }
            Ok(RuntimeExit {
                resource: s.tick(clock.now()).map_err(|_| Error::Policy)?,
                child: s.local_termination_state(),
            })
        },
        cleanup_timeout,
    )
    .await
}

async fn monitor(
    controls: impl Future<Output = ()>,
    shutdown: impl Future<Output = ()>,
    mut stop: impl FnMut(),
    mut tick: impl FnMut() -> Result<RuntimeExit>,
    cleanup_timeout: Duration,
) -> Result<RuntimeExit> {
    tokio::pin!(controls, shutdown);
    let mut timer = tokio::time::interval(Duration::from_millis(25));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut stopping = None;
    let mut signalled = false;
    let mut controls_done = false;
    loop {
        tokio::select! {
            biased;
            () = &mut shutdown, if !signalled => {
                signalled = true;
                stop();
                stopping.get_or_insert_with(Instant::now);
            }
            _ = timer.tick() => {
                let state = tick()?;
                if state.resource != TeardownState::Running {
                    stopping.get_or_insert_with(Instant::now);
                }
                if (state.resource == TeardownState::ConfirmedAbsent && state.child == LocalTerminationState::DirectChildReaped)
                    || stopping.is_some_and(|start| start.elapsed() >= cleanup_timeout) {
                    return Ok(state);
                }
            }
            () = &mut controls, if !controls_done => {
                controls_done = true;
                stop();
                stopping.get_or_insert_with(Instant::now);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, future::pending};

    #[tokio::test(start_paused = true)]
    async fn pending_control_does_not_block_ticks_or_get_recreated() {
        struct Guard<'a>(&'a Cell<usize>);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let drops = Cell::new(0);
        let started = Cell::new(0);
        let count = Cell::new(0);
        let controls = async {
            started.set(started.get() + 1);
            let _guard = Guard(&drops);
            pending::<()>().await;
        };
        let exit = monitor(
            controls,
            pending(),
            || panic!("unexpected signal"),
            || {
                assert_eq!(drops.get(), 0);
                count.set(count.get() + 1);
                Ok(RuntimeExit {
                    resource: if count.get() < 5 {
                        TeardownState::Running
                    } else {
                        TeardownState::ConfirmedAbsent
                    },
                    child: LocalTerminationState::DirectChildReaped,
                })
            },
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert_eq!(exit.resource, TeardownState::ConfirmedAbsent);
        assert_eq!(started.get(), 1);
        assert_eq!(drops.get(), 1);
        assert_eq!(count.get(), 5);
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_cleanup_is_bounded_and_local_exit_is_not_absence() {
        let stopped = Cell::new(false);
        let start = Instant::now();
        let exit = monitor(
            pending(),
            std::future::ready(()),
            || stopped.set(true),
            || {
                assert!(stopped.get());
                Ok(RuntimeExit {
                    resource: TeardownState::Pending,
                    child: LocalTerminationState::DirectChildReaped,
                })
            },
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert_eq!(exit.resource, TeardownState::Pending);
        assert_eq!(start.elapsed(), Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn control_task_exit_still_drives_cleanup() {
        let stopped = Cell::new(false);
        let exit = monitor(
            std::future::ready(()),
            pending(),
            || stopped.set(true),
            || {
                Ok(RuntimeExit {
                    resource: if stopped.get() {
                        TeardownState::ConfirmedAbsent
                    } else {
                        TeardownState::Running
                    },
                    child: LocalTerminationState::DirectChildReaped,
                })
            },
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert!(stopped.get());
        assert_eq!(exit.resource, TeardownState::ConfirmedAbsent);
    }
    #[test]
    fn lease_never_outlives_certificate_and_bad_limits_refuse() {
        assert_eq!(lifetime(10, 20, 100).unwrap(), 10);
        assert_eq!(lifetime(10, 20, 2).unwrap(), 2);
        for (now, expires, limit) in [(-1, 20, 1), (20, 20, 1), (21, 20, 1), (10, 20, 0)] {
            assert!(lifetime(now, expires, limit).is_err());
        }
    }
    #[test]
    fn wall_rollback_cannot_freeze_deadline_and_failure_expires_it() {
        assert_eq!(observed_time(100, 20, 50), 120);
        assert_eq!(observed_time(100, 20, 130), 130);
        assert_eq!(observed_time(100, u64::MAX, 100), i64::MAX);
        assert_eq!(observed_time(100, 0, -1), i64::MAX);
    }
    #[test]
    fn receipt_gate_is_single_use() {
        let mut gate = ReceiptGate(true);
        assert!(gate.authorize());
        assert!(!gate.authorize());
    }
}
