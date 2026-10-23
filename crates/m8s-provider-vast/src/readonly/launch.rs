//! One-use provider dispatch for a durable, independently guarded launch.
//! The owning coordinator must persist its claim and arm cleanup BEFORE calling.
//! No API retry, default image, credentials in JSON, shell, or arbitrary env.
use super::*;
use recovery::RecoveryIntent;
use std::num::NonZeroU64;

/// Public-only launch parameters. The SSH key is public; bootstrap material is
/// later delivered through a pinned SSH connection, never provider metadata.
pub struct LaunchRequest {
    offer: NonZeroU64,
    identity: RecoveryIntent,
    ssh_public_key: String,
    disk_gb: u32,
}
impl LaunchRequest {
    pub fn new(
        offer: NonZeroU64,
        identity: RecoveryIntent,
        ssh_public_key: String,
        disk_gb: u32,
    ) -> Result<Self> {
        // Full wire-key checking is repeated in the image's provisioning server.
        // Restrict the request to one canonical Ed25519 key with no comments,
        // environment assignments, shell quoting, or authorized_keys options.
        let encoded = ssh_public_key
            .strip_prefix("ssh-ed25519 ")
            .ok_or(Error::Invalid)?;
        if !(1..=20).contains(&disk_gb)
            || encoded.len() != 68
            || !encoded.starts_with("AAAAC3NzaC1lZDI1NTE5AAAAI")
            || !encoded
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+/".contains(&b))
        {
            return Err(Error::Invalid);
        }
        Ok(Self {
            offer,
            identity,
            ssh_public_key,
            disk_gb,
        })
    }
    pub fn identity(&self) -> &RecoveryIntent {
        &self.identity
    }
    /// Canonical public launch parameters for durable intent fingerprinting.
    /// Includes the offer, image, ownership label, disk and provisioning public
    /// key. No account credential or bootstrap secret can enter this encoding.
    pub fn canonical_intent(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(&json!({
            "schema":"m8s.vast-launch-intent/v1",
            "offer": self.offer.get(), "image": self.identity.image(),
            "label": self.identity.label(), "disk_gb": self.disk_gb,
            "ssh_public_key": self.ssh_public_key,
            "tcp_port":2222, "udp_port":40100, "runtype":"args"
        }))
        .map_err(|_| Error::Invalid)
    }
}

/// A successful create response is only a candidate ID, never observed presence,
/// a bound nonce, served model, Ready, or a provider-confirmed resource snapshot.
pub struct CreatedCandidate {
    instance: NonZeroU64,
}
impl CreatedCandidate {
    pub fn instance_id(&self) -> NonZeroU64 {
        self.instance
    }
}

/// Consumed by dispatch, including on error. A failed response is uncertain;
/// recover the ORIGINAL exact identity, never construct another create attempt.
/// This type alone does not enforce durable one-shot budget/claim custody.
pub struct OneShotLaunch<S> {
    transport: ReadonlyHttps<S>,
    request: LaunchRequest,
}
impl<S: FnMut() -> Result<SecretValue>> OneShotLaunch<S> {
    pub fn new(request: LaunchRequest, supplier: S) -> Result<Self> {
        Ok(Self {
            transport: ReadonlyHttps::new(supplier)?,
            request,
        })
    }
    pub fn dispatch(mut self) -> Result<CreatedCandidate> {
        dispatch_with(
            &mut self.transport.backend,
            &mut self.transport.supplier,
            self.request,
        )
    }
}

fn dispatch_with<B: Backend, S: FnMut() -> Result<SecretValue>>(
    backend: &mut B,
    supplier: &mut S,
    request: LaunchRequest,
) -> Result<CreatedCandidate> {
    let body = serde_json::to_vec(&json!({
        "client_id":"me", "image":request.identity.image(),
        "label":request.identity.label(), "disk":request.disk_gb,
        "runtype":"args", "cancel_unavail":true,
        "env": {"M8S_PROVISION_SSH_KEY":request.ssh_public_key,
                "-p 2222/tcp":"1", "-p 40100/udp":"1"}
    }))
    .map_err(|_| Error::Invalid)?;
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
            method: reqwest::Method::PUT,
            url: format!("{ORIGIN}/api/v0/asks/{}/", request.offer),
            authorization,
            body: Some(body),
        })
        .map_err(|_| Error::Unavailable)?;
    let value = checked_json(reply, bytes, 8192)?;
    if value.get("success") != Some(&Value::Bool(true)) {
        return Err(Error::Unavailable);
    }
    let instance = value
        .get("new_contract")
        .and_then(Value::as_u64)
        .and_then(NonZeroU64::new)
        .ok_or(Error::Unavailable)?;
    Ok(CreatedCandidate { instance })
}

#[cfg(test)]
mod tests {
    use super::super::tests::{Fake, key};
    use super::*;
    fn request() -> LaunchRequest {
        LaunchRequest::new(
            NonZeroU64::new(42).unwrap(),
            RecoveryIntent::new(
                "synthetic-owner",
                "synthetic-node",
                "smoke",
                &format!("example.invalid/mesh@sha256:{}", "a".repeat(64)),
            )
            .unwrap(),
            format!(
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI{}",
                "A".repeat(68 - "AAAAC3NzaC1lZDI1NTE5AAAAI".len())
            ),
            10,
        )
        .unwrap()
    }
    #[test]
    fn fixed_one_shot_route_public_only_body_and_candidate() {
        let mut fake = Fake::json(br#"{"success":true,"new_contract":99}"#);
        assert_eq!(
            dispatch_with(&mut fake, &mut key, request())
                .unwrap()
                .instance_id()
                .get(),
            99
        );
        assert_eq!(fake.calls, 1);
        let sent = fake.request.unwrap();
        assert_eq!(sent.method, reqwest::Method::PUT);
        assert_eq!(sent.url, "https://console.vast.ai/api/v0/asks/42/");
        assert!(sent.authorization.is_sensitive());
        let body: Value = serde_json::from_slice(sent.body.as_ref().unwrap()).unwrap();
        assert_eq!(body["label"], "m8s:v1:synthetic-owner:synthetic-node:smoke");
        assert_eq!(body["runtype"], "args");
        assert_eq!(body["env"].as_object().unwrap().len(), 3);
        for field in [
            "onstart",
            "image_login",
            "template_id",
            "args",
            "nonce",
            "node_key",
            "owner_key",
        ] {
            assert!(body.get(field).is_none());
        }
    }
    #[test]
    fn ambiguous_or_malformed_response_never_retries() {
        for body in [
            br#"{}"#.as_slice(),
            br#"{"success":true}"#,
            br#"{"success":false,"new_contract":99}"#,
            br#"{"success":true,"new_contract":0}"#,
            br#"{"success":true,"new_contract":99,"new_contract":100}"#,
        ] {
            let mut fake = Fake::json(body);
            assert!(dispatch_with(&mut fake, &mut key, request()).is_err());
            assert_eq!(fake.calls, 1);
        }
    }
    #[test]
    fn invalid_credential_refuses_before_dispatch() {
        let mut fake = Fake::json(br#"{"success":true,"new_contract":99}"#);
        assert!(
            dispatch_with(
                &mut fake,
                &mut || Ok(SecretValue::new(b"bad\nheader".to_vec())),
                request()
            )
            .is_err()
        );
        assert_eq!(fake.calls, 0);
    }
}
