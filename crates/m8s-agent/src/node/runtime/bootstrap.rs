//! One authenticated redemption followed by native supervision. Cleanup custody
//! stays with the caller on every return, including pre-spawn failure.
use super::{RuntimeExit, RuntimePlan};
use crate::node::{Error, Result};
use crate::{Effect, Outcome, supervisor::TeardownDriver};
use iroh::{Endpoint, EndpointAddr};
use m8s_bootstrap::{Redemption, transport::redeem_verified_over_iroh};
use std::future::Future;

struct BorrowedDriver<'a, D>(&'a mut D);
impl<D: TeardownDriver> TeardownDriver for BorrowedDriver<'_, D> {
    fn perform(&mut self, effect: Effect) -> Outcome {
        self.0.perform(effect)
    }
}

/// Uses the same authenticated endpoint for bootstrap and live control. The
/// caller must already have established scoped destruction capability before
/// asserting it in `request`; this function does not manufacture that authority.
/// Only the original receipt can pass to native startup. No retries of redemption
/// or signing. An error attempts one bounded cleanup batch and leaves the driver
/// with its caller for independent retries and explicit absence confirmation.
/// Cancellation also leaves that externally owned driver available; the owner
/// must supervise cancellation rather than interpreting dropped work as absence.
pub async fn redeem_and_run<D: TeardownDriver>(
    endpoint: &Endpoint,
    owner: EndpointAddr,
    request: Redemption,
    plan: RuntimePlan<'_>,
    driver: &mut D,
    shutdown: impl Future<Output = ()>,
    clock: impl Fn() -> i64,
) -> Result<RuntimeExit> {
    tokio::pin!(shutdown);
    let attempt = async {
        super::validate_plan(endpoint, &plan)?;
        if !request.self_destruct_present
            || request.node != plan.launch.node
            || *owner.id.as_bytes() != plan.launch.owner.0
        {
            return Err(Error::Policy);
        }
        let receipt = tokio::select! {
            biased;
            () = &mut shutdown => return Err(Error::Policy),
            receipt = redeem_verified_over_iroh(endpoint, owner, request, plan.verifier, &clock) =>
                receipt.map_err(|_| Error::Policy)?,
        };
        super::run(
            endpoint,
            receipt,
            plan,
            BorrowedDriver(&mut *driver),
            &mut shutdown,
            clock,
        )
        .await
    }
    .await;
    if attempt.is_err() {
        cleanup_failed_start(driver);
    }
    attempt
}

fn cleanup_failed_start(driver: &mut impl TeardownDriver) {
    // Destruction must not be postponed by drain/wipe failure. Success of Destroy
    // is never interpreted as ConfirmedAbsent. The owner retains the driver.
    for effect in [
        Effect::Drain,
        Effect::Wipe,
        Effect::Destroy,
        Effect::ConfirmAbsent,
    ] {
        let _ = driver.perform(effect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_prepare_cleanup_attempts_destroy_despite_earlier_failures() {
        struct Driver(Vec<Effect>);
        impl TeardownDriver for Driver {
            fn perform(&mut self, effect: Effect) -> Outcome {
                self.0.push(effect);
                Outcome::Failure
            }
        }
        let mut driver = Driver(vec![]);
        cleanup_failed_start(&mut driver);
        assert_eq!(
            driver.0,
            [
                Effect::Drain,
                Effect::Wipe,
                Effect::Destroy,
                Effect::ConfirmAbsent
            ]
        );
        // Custody survives the failed attempt and remains usable for retry.
        cleanup_failed_start(&mut driver);
        assert_eq!(driver.0.len(), 8);
    }
}
