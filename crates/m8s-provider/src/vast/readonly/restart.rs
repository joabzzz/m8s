//! Authenticated cleanup of an already journaled exact resource after restart.
//! This synchronous adapter is not an armed worker or CREATE authority.
use super::*;
use crate::vast::watchdog::OwnedResourceAbsence;
pub use m8s_secrets::SecretValue;
use recovery::RecoveryIntent;
use std::num::NonZeroU64;

/// The caller supplies the original durable identity, account and resource
/// binding. Construction performs no I/O. Each operation captures one supplied
/// credential, authenticates its account, and uses that same credential for the
/// exact resource request. No discovery, inventory, retries or launch methods.
pub struct RestartCleanup<S> {
    transport: ReadonlyHttps<S>,
    intent: RecoveryIntent,
    account: NonZeroU64,
    instance: NonZeroU64,
}

impl<S: FnMut() -> Result<SecretValue>> RestartCleanup<S> {
    pub fn new(
        intent: RecoveryIntent,
        account: NonZeroU64,
        instance: NonZeroU64,
        supplier: S,
    ) -> Result<Self> {
        Ok(Self {
            transport: ReadonlyHttps::new(supplier)?,
            intent,
            account,
            instance,
        })
    }

    /// Acknowledgement is not absence. Even failure may mean DELETE took effect.
    pub fn destroy(&mut self) -> Result<destroy::DestroyAcknowledged> {
        authenticated_with(
            &mut self.transport.backend,
            &mut self.transport.supplier,
            self.account,
            |backend, secret| {
                destroy::destroy_with(
                    backend,
                    &mut || Ok(SecretValue::new(secret.expose().to_vec())),
                    self.instance,
                )
            },
        )
    }

    /// Only explicit exact-ID absence yields a fresh receipt for finalization.
    /// Stopped/present resources return None; errors leave liability unresolved.
    pub fn confirm_absent(&mut self) -> Result<Option<OwnedResourceAbsence>> {
        let receipt = authenticated_with(
            &mut self.transport.backend,
            &mut self.transport.supplier,
            self.account,
            |backend, secret| {
                destroy::observe_with(
                    backend,
                    &mut || Ok(SecretValue::new(secret.expose().to_vec())),
                    self.instance,
                )
            },
        )?;
        receipt
            .map(|receipt| {
                let observed_at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .and_then(|d| i64::try_from(d.as_secs()).ok())
                    .ok_or(Error::Unavailable)?;
                Ok(OwnedResourceAbsence {
                    creator_account_id: self.account,
                    instance: receipt.instance_id(),
                    intent: self.intent.clone(),
                    observed_at,
                })
            })
            .transpose()
    }
}

fn authenticated_with<B: Backend, S: FnMut() -> Result<SecretValue>, T>(
    backend: &mut B,
    supplier: &mut S,
    account: NonZeroU64,
    action: impl FnOnce(&mut B, &SecretValue) -> Result<T>,
) -> Result<T> {
    let secret = supplier().map_err(|_| Error::Unavailable)?;
    let mut retained = || Ok(SecretValue::new(secret.expose().to_vec()));
    preflight::authenticate_account_with(backend, &mut retained, account)?;
    action(backend, &secret)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{Fake, key};
    use super::*;
    use std::collections::VecDeque;

    struct Sequence {
        replies: VecDeque<WireReply>,
        requests: Vec<WireRequest>,
    }
    impl Sequence {
        fn new(bodies: &[&[u8]]) -> Self {
            Self {
                replies: bodies
                    .iter()
                    .map(|body| Fake::json(body).reply.unwrap())
                    .collect(),
                requests: vec![],
            }
        }
    }
    impl Backend for Sequence {
        fn send(&mut self, request: WireRequest) -> Result<WireReply> {
            self.requests.push(request);
            self.replies.pop_front().ok_or(Error::Unavailable)
        }
    }
    fn observe(backend: &mut Sequence) -> Result<Option<destroy::ConfirmedResourceAbsence>> {
        authenticated_with(
            backend,
            &mut key,
            NonZeroU64::new(7).unwrap(),
            |backend, secret| {
                destroy::observe_with(
                    backend,
                    &mut || Ok(SecretValue::new(secret.expose().to_vec())),
                    NonZeroU64::new(42).unwrap(),
                )
            },
        )
    }
    #[test]
    fn wrong_missing_or_failed_account_prevents_resource_request() {
        for body in [
            br#"{"id":8}"#.as_slice(),
            br#"{}"#,
            br#"{"id":7,"success":false}"#,
        ] {
            let mut backend = Sequence::new(&[body, br#"{"instances":null}"#]);
            assert!(observe(&mut backend).is_err());
            assert_eq!(backend.requests.len(), 1);
            assert!(backend.requests[0].url.ends_with("/users/current/"));
        }
    }
    #[test]
    fn one_credential_is_captured_for_authentication_and_delete() {
        let mut backend = Sequence::new(&[br#"{"id":7,"credit":-1}"#, br#"{"success":true}"#]);
        let mut calls = 0;
        let mut supplier = || {
            calls += 1;
            if calls > 1 {
                Err(Error::Unavailable)
            } else {
                key()
            }
        };
        let ack = authenticated_with(
            &mut backend,
            &mut supplier,
            NonZeroU64::new(7).unwrap(),
            |backend, secret| {
                destroy::destroy_with(
                    backend,
                    &mut || Ok(SecretValue::new(secret.expose().to_vec())),
                    NonZeroU64::new(42).unwrap(),
                )
            },
        )
        .unwrap();
        assert_eq!(ack.instance_id().get(), 42);
        assert_eq!(calls, 1);
        assert_eq!(backend.requests.len(), 2);
        assert_eq!(
            backend.requests[0].authorization,
            backend.requests[1].authorization
        );
        assert!(
            backend
                .requests
                .iter()
                .all(|r| r.authorization.is_sensitive() && r.body.is_none())
        );
        assert_eq!(backend.requests[1].method, reqwest::Method::DELETE);
        assert_eq!(
            backend.requests[1].url,
            format!("{ORIGIN}/api/v0/instances/42/")
        );
    }
    #[test]
    fn every_observation_reauthenticates_and_only_exact_null_is_absence() {
        for (body, expected) in [
            (br#"{"instances":null}"#.as_slice(), Some(true)),
            (
                br#"{"instances":{"id":42,"actual_status":"stopped"}}"#,
                Some(false),
            ),
            (br#"{"success":true}"#, None),
            (br#"{"instances":[]}"#, None),
            (br#"{"instances":{"id":43}}"#, None),
        ] {
            let mut backend = Sequence::new(&[br#"{"id":7}"#, body, br#"{"id":8}"#]);
            assert_eq!(observe(&mut backend).map(|r| r.is_some()).ok(), expected);
            assert!(observe(&mut backend).is_err());
            assert_eq!(backend.requests.len(), 3);
            assert!(
                backend
                    .requests
                    .iter()
                    .all(|r| r.method == reqwest::Method::GET)
            );
        }
    }
}
