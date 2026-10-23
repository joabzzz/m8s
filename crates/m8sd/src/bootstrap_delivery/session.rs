//! Owner-side original-stream delivery followed by pinned control heartbeats.
//! The caller retains resource cleanup custody throughout, including cancellation.
use super::{DeliveredBootstrap, DeliveryPlan};
use crate::Store;
use iroh::{Endpoint, EndpointAddr};
use m8s_bootstrap::{
    CONTROL_ALPN, EndpointId, Error, NodeSigner, Result,
    mp001::Mp001CertificateVerifier,
    transport::{accept_boot, connect_boot_to_node, send_heartbeat},
};
use std::{future::Future, time::Duration};
use tokio::time::{Instant, MissedTickBehavior};

/// Trusted provider-observed direct address and Store-bound delivery inputs.
/// This is not a Ready grant or a substitute for a surviving cleanup watchdog.
pub struct OwnerSessionPlan<'a> {
    pub node_address: EndpointAddr,
    pub direction: BootstrapDirection,
    pub delivery: DeliveryPlan<'a>,
    pub bootstrap_timeout: Duration,
}

pub enum BootstrapDirection {
    NodeInitiates,
    OwnerInitiates,
}

fn heartbeat_period(ttl: u64) -> Result<Duration> {
    if ttl == 0 || ttl > 1800 {
        return Err(Error::Refused);
    }
    Ok(Duration::from_millis((ttl * 1000 / 3).max(1)))
}

fn remaining_lifetime(now: i64, expires: i64) -> Result<Duration> {
    expires
        .checked_sub(now)
        .filter(|remaining| now >= 0 && *remaining > 0 && *remaining <= 1800)
        .map(|remaining| Duration::from_secs(remaining as u64))
        .ok_or(Error::Refused)
}

async fn maintain_control(
    control: &iroh::endpoint::Connection,
    mut ticks: tokio::time::Interval,
    expires: i64,
    clock: &impl Fn() -> i64,
) -> Result<()> {
    let mut sequence = 1u64;
    loop {
        ticks.tick().await;
        remaining_lifetime(clock(), expires)?;
        sequence = sequence.checked_add(1).ok_or(Error::Refused)?;
        send_heartbeat(control, sequence).await?;
    }
}

impl Store {
    /// Admit exactly the expected TLS peer, burn/sign/deliver on that original
    /// stream, establish pinned control, then run the caller's bounded work.
    /// Heartbeats stop on cancellation, control failure, work completion, or
    /// certificate expiry. Work must independently verify served model and
    /// owner-routed canary; its output must not be released before owned cleanup.
    /// No endpoint is created and no provider mutation occurs in this function.
    #[allow(clippy::too_many_arguments)]
    pub async fn with_bootstrapped_node<T, F: Future<Output = Result<T>>>(
        &self,
        endpoint: &Endpoint,
        plan: OwnerSessionPlan<'_>,
        signer: &mut impl NodeSigner,
        verifier: &Mp001CertificateVerifier,
        work: impl FnOnce(DeliveredBootstrap) -> F,
        shutdown: impl Future<Output = ()>,
        clock: impl Fn() -> i64,
    ) -> Result<T> {
        let owner = plan.delivery.observation.binding.owner;
        let expected_node = EndpointId(*plan.node_address.id.as_bytes());
        let period = heartbeat_period(plan.delivery.settings.heartbeat_ttl_seconds)?;
        if endpoint.is_closed()
            || EndpointId(*endpoint.id().as_bytes()) != owner
            || expected_node == owner
            || plan.bootstrap_timeout.is_zero()
            || plan.bootstrap_timeout > Duration::from_secs(60)
        {
            return Err(Error::Refused);
        }
        tokio::pin!(shutdown);
        let bootstrap = async {
            if matches!(plan.direction, BootstrapDirection::OwnerInitiates) {
                let connection = connect_boot_to_node(endpoint, plan.node_address.clone()).await?;
                return self
                    .deliver_bootstrap(connection, plan.delivery, signer, verifier, &clock)
                    .await;
            }
            // A bounded number of unrelated peers cannot consume an unbounded
            // listener lifetime. No nonce is read or burned for a wrong peer.
            for _ in 0..8 {
                let incoming = endpoint.accept().await.ok_or(Error::Transport)?;
                let Ok(connection) = accept_boot(incoming, owner).await else {
                    continue;
                };
                if connection.peer_endpoint() != expected_node {
                    continue;
                }
                return self
                    .deliver_bootstrap(connection, plan.delivery, signer, verifier, &clock)
                    .await;
            }
            Err(Error::Refused)
        };
        let delivered = tokio::select! {
            biased;
            () = &mut shutdown => return Err(Error::Refused),
            receipt = tokio::time::timeout(plan.bootstrap_timeout, bootstrap) =>
                receipt.map_err(|_| Error::Transport)??,
        };
        let expires = delivered.claims().expires_at;
        let deadline = Instant::now() + remaining_lifetime(clock(), expires)?;
        let control = tokio::select! {
            biased;
            () = &mut shutdown => return Err(Error::Refused),
            connection = tokio::time::timeout(Duration::from_secs(10), endpoint.connect(plan.node_address, CONTROL_ALPN)) =>
                connection.map_err(|_| Error::Transport)?.map_err(|_| Error::Transport)?,
        };
        // Both endpoint selection and actual connection must agree before work.
        if *control.remote_id().as_bytes() != expected_node.0 || control.alpn() != CONTROL_ALPN {
            control.close(0u32.into(), b"control refused");
            return Err(Error::Refused);
        }
        let result = async {
            tokio::select! {
                biased;
                () = &mut shutdown => return Err(Error::Refused),
                () = tokio::time::sleep_until(deadline) => return Err(Error::Refused),
                result = send_heartbeat(&control, 1) => result?,
            }
            let mut ticks = tokio::time::interval_at(Instant::now() + period, period);
            ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
            let work = work(delivered);
            tokio::pin!(work);
            let heartbeats = maintain_control(&control, ticks, expires, &clock);
            tokio::select! {
                biased;
                () = &mut shutdown => Err(Error::Refused),
                () = tokio::time::sleep_until(deadline) => Err(Error::Refused),
                result = heartbeats => { result?; Err(Error::Transport) },
                result = &mut work => { remaining_lifetime(clock(), expires)?; result },
            }
        }
        .await;
        control.close(0u32.into(), b"owner work complete");
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn heartbeat_and_certificate_deadlines_are_bounded() {
        assert_eq!(heartbeat_period(1).unwrap(), Duration::from_millis(333));
        assert_eq!(heartbeat_period(30).unwrap(), Duration::from_secs(10));
        for ttl in [0, 1801, u64::MAX] {
            assert!(heartbeat_period(ttl).is_err());
        }
        assert_eq!(
            remaining_lifetime(100, 700).unwrap(),
            Duration::from_secs(600)
        );
        for (now, expires) in [
            (-1, 100),
            (100, 100),
            (100, 99),
            (0, 1801),
            (i64::MIN, i64::MAX),
        ] {
            assert!(remaining_lifetime(now, expires).is_err());
        }
    }
}
