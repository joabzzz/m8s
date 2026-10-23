//! Exact-instance destruction and separate explicit absence observation.
//! The caller must bind the instance to its durable ownership journal before
//! construction. A credential's provider-side scope is not established here.
use super::*;
use std::num::NonZeroU64;

/// One fixed resource, no arbitrary URL, method, body, or automatic retries.
/// Keep this outside the model process; inject only the intended credential.
/// Construction does not consume the credential or contact the provider.
pub struct ScopedDestroyHttps<S> {
    transport: ReadonlyHttps<S>,
    instance: NonZeroU64,
}

/// The provider acknowledged a destroy request. This is deliberately distinct
/// from absence: independent owner reconciliation must still confirm removal.
#[derive(Debug, PartialEq, Eq)]
pub struct DestroyAcknowledged {
    instance: NonZeroU64,
}
impl DestroyAcknowledged {
    pub fn instance_id(&self) -> NonZeroU64 {
        self.instance
    }
}

/// An exact-instance GET explicitly returned `instances: null`. This is scoped
/// to the credential's visibility; use the same owner account that created the
/// journaled resource. Never use a revoked per-instance credential as owner proof.
#[derive(Debug, PartialEq, Eq)]
pub struct ConfirmedResourceAbsence {
    instance: NonZeroU64,
}
impl ConfirmedResourceAbsence {
    pub fn instance_id(&self) -> NonZeroU64 {
        self.instance
    }
}

impl<S: FnMut() -> Result<SecretValue>> ScopedDestroyHttps<S> {
    pub fn new(instance: NonZeroU64, supplier: S) -> Result<Self> {
        Ok(Self {
            transport: ReadonlyHttps::new(supplier)?,
            instance,
        })
    }

    /// One exact GET, independent of DELETE acknowledgement. Present/stopped
    /// rows return None; malformed responses, missing fields and 404 are errors.
    /// Caller must retain the owner account's visibility over the known resource.
    pub fn confirm_absent(&mut self) -> Result<Option<ConfirmedResourceAbsence>> {
        observe_with(
            &mut self.transport.backend,
            &mut self.transport.supplier,
            self.instance,
        )
    }

    /// Exactly one DELETE. Timeout/error leaves the outcome uncertain, never
    /// authorizes deleting a journal row or replacing the rented instance.
    pub fn destroy(&mut self) -> Result<DestroyAcknowledged> {
        destroy_with(
            &mut self.transport.backend,
            &mut self.transport.supplier,
            self.instance,
        )
    }
}

fn destroy_with<B: Backend, S: FnMut() -> Result<SecretValue>>(
    backend: &mut B,
    supplier: &mut S,
    instance: NonZeroU64,
) -> Result<DestroyAcknowledged> {
    let value = exchange_scoped(backend, supplier, instance, reqwest::Method::DELETE)?;
    if value.get("success") != Some(&Value::Bool(true)) {
        return Err(Error::Unavailable);
    }
    Ok(DestroyAcknowledged { instance })
}

fn observe_with<B: Backend, S: FnMut() -> Result<SecretValue>>(
    backend: &mut B,
    supplier: &mut S,
    instance: NonZeroU64,
) -> Result<Option<ConfirmedResourceAbsence>> {
    let value = exchange_scoped(backend, supplier, instance, reqwest::Method::GET)?;
    match value.get("instances") {
        Some(Value::Null) => Ok(Some(ConfirmedResourceAbsence { instance })),
        Some(Value::Object(row))
            if row.get("id").and_then(Value::as_u64) == Some(instance.get()) =>
        {
            Ok(None)
        }
        _ => Err(Error::Unavailable),
    }
}

fn exchange_scoped<B: Backend, S: FnMut() -> Result<SecretValue>>(
    backend: &mut B,
    supplier: &mut S,
    instance: NonZeroU64,
    method: reqwest::Method,
) -> Result<Value> {
    let secret = supplier().map_err(|_| Error::Unavailable)?;
    let bytes = secret.expose();
    if bytes.is_empty() || bytes.len() > 4096 || !bytes.iter().all(|b| (33..=126).contains(b)) {
        return Err(Error::Unavailable);
    }
    let bearer = SecretValue::new([b"Bearer ".as_slice(), bytes].concat());
    let mut authorization =
        HeaderValue::from_bytes(bearer.expose()).map_err(|_| Error::Unavailable)?;
    authorization.set_sensitive(true);
    let reply = backend
        .send(WireRequest {
            method,
            url: format!("{ORIGIN}/api/v0/instances/{instance}/"),
            authorization,
            body: None,
        })
        .map_err(|_| Error::Unavailable)?;
    // Bound acknowledgement bodies more tightly than inventory responses.
    const LIMIT: usize = 8192;
    if reply.status != 200
        || reply
            .content_type
            .as_deref()
            .map(|s| s.split(';').next().unwrap_or("").trim())
            != Some("application/json")
        || !matches!(reply.encoding.as_deref(), None | Some("identity"))
        || reply.declared_length.is_some_and(|n| n > LIMIT as u64)
    {
        return Err(Error::Unavailable);
    }
    let mut raw = Vec::new();
    reply
        .body
        .take((LIMIT + 1) as u64)
        .read_to_end(&mut raw)
        .map_err(|_| Error::Unavailable)?;
    if raw.len() > LIMIT || reply.declared_length.is_some_and(|n| n != raw.len() as u64) {
        return Err(Error::Unavailable);
    }
    serde_json::from_slice::<UniqueKeys>(&raw).map_err(|_| Error::Unavailable)?;
    let value: Value = serde_json::from_slice(&raw).map_err(|_| Error::Unavailable)?;
    let object = value.as_object().ok_or(Error::Unavailable)?;
    if object
        .get("success")
        .is_some_and(|v| v != &Value::Bool(true))
        || object.contains_key("error")
        || object.contains_key("errors")
        || reflects_secret(
            &value,
            std::str::from_utf8(bytes).map_err(|_| Error::Unavailable)?,
        )
    {
        return Err(Error::Unavailable);
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{Fake, key};
    use super::*;

    #[test]
    fn explicit_exact_null_is_separate_from_acknowledgement() {
        let instance = NonZeroU64::new(42).unwrap();
        let mut backend = Fake::json(br#"{"instances":null}"#);
        let proof = observe_with(&mut backend, &mut key, instance)
            .unwrap()
            .unwrap();
        assert_eq!(proof.instance_id(), instance);
        let sent = backend.request.unwrap();
        assert_eq!(sent.method, reqwest::Method::GET);
        assert_eq!(sent.url, "https://console.vast.ai/api/v0/instances/42/");
        for body in [
            br#"{}"#.as_slice(),
            br#"{"success":true}"#,
            br#"{"instances":[]}"#,
            br#"{"instances":null,"success":false}"#,
            br#"{"instances":null,"error":"forbidden"}"#,
            br#"{"instances":null,"instances":{"id":42}}"#,
            br#"{"instances":{"id":43}}"#,
        ] {
            assert!(observe_with(&mut Fake::json(body), &mut key, instance).is_err());
        }
        for state in ["running", "stopped", "exited", "offline"] {
            let body =
                serde_json::to_vec(&json!({"instances":{"id":42,"actual_status":state}})).unwrap();
            assert!(
                observe_with(&mut Fake::json(&body), &mut key, instance)
                    .unwrap()
                    .is_none()
            );
        }
        for status in [401, 403, 404, 429, 500] {
            let mut backend = Fake::json(br#"{"instances":null}"#);
            backend.reply.as_mut().unwrap().status = status;
            assert!(observe_with(&mut backend, &mut key, instance).is_err());
        }
    }

    #[test]
    fn construction_does_not_obtain_credentials() {
        let called = std::cell::Cell::new(false);
        let _client = ScopedDestroyHttps::new(NonZeroU64::new(42).unwrap(), || {
            called.set(true);
            key()
        })
        .unwrap();
        assert!(!called.get());
    }

    #[test]
    fn invalid_credentials_never_dispatch() {
        for bytes in [vec![], vec![b'x'; 4097], b"synthetic\nheader".to_vec()] {
            let mut backend = Fake::json(br#"{"success":true}"#);
            let mut supplier = || Ok(SecretValue::new(bytes.clone()));
            assert!(
                destroy_with(&mut backend, &mut supplier, NonZeroU64::new(42).unwrap()).is_err()
            );
            assert_eq!(backend.calls, 0);
        }
    }

    #[test]
    fn unbounded_or_inconsistent_responses_fail_closed() {
        for case in 0..5 {
            let mut backend = Fake::json(br#"{"success":true}"#);
            let reply = backend.reply.as_mut().unwrap();
            match case {
                0 => reply.declared_length = Some(8193),
                1 => reply.declared_length = Some(1),
                2 => reply.content_type = Some("text/html".into()),
                3 => reply.encoding = Some("gzip".into()),
                _ => reply.body = Box::new(std::io::Cursor::new(vec![b' '; 8193])),
            }
            assert!(destroy_with(&mut backend, &mut key, NonZeroU64::new(42).unwrap()).is_err());
            assert_eq!(backend.calls, 1);
        }
    }

    #[test]
    fn exact_resource_sensitive_auth_and_acknowledgement_only() {
        let mut backend =
            Fake::json(br#"{"success":true,"msg":"Instance destroyed successfully"}"#);
        let instance = NonZeroU64::new(42).unwrap();
        let ack = destroy_with(&mut backend, &mut key, instance).unwrap();
        assert_eq!(ack.instance_id(), instance);
        assert_eq!(backend.calls, 1);
        let sent = backend.request.unwrap();
        assert_eq!(sent.url, "https://console.vast.ai/api/v0/instances/42/");
        assert_eq!(sent.method, reqwest::Method::DELETE);
        assert!(sent.authorization.is_sensitive());
        assert!(sent.body.is_none());
    }

    #[test]
    fn uncertain_or_malformed_response_never_acknowledges_or_retries() {
        for body in [
            br#"{}"#.as_slice(),
            br#"{"success":false}"#,
            br#"{"success":true,"success":false}"#,
            br#"{"success":true,"error":"unknown"}"#,
            br#"{"success":true,"msg":"SYNTHETIC-ONLY-FAKE-ACCOUNT-KEY"}"#,
        ] {
            let mut backend = Fake::json(body);
            assert!(destroy_with(&mut backend, &mut key, NonZeroU64::new(42).unwrap()).is_err());
            assert_eq!(backend.calls, 1);
        }
        for status in [202, 204, 301, 401, 403, 404, 429, 500] {
            let mut backend = Fake::json(br#"{"success":true}"#);
            backend.reply.as_mut().unwrap().status = status;
            assert!(destroy_with(&mut backend, &mut key, NonZeroU64::new(42).unwrap()).is_err());
            assert_eq!(backend.calls, 1);
        }
    }
}
