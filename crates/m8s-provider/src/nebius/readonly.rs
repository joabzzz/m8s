//! Optional REST gateway client: fixed origin, GET only, no retries or redirects.
//! No implicit credentials/environment/CLI execution, token exchange or mutations.
use super::{Transport, component, wire};
use crate::{Error, Result};
use m8s_secrets::SecretValue;
use reqwest::header::{ACCEPT, ACCEPT_ENCODING, AUTHORIZATION, HeaderValue};
use serde::de::DeserializeOwned;
use std::{io::Read, sync::Arc, time::Duration};

const ORIGIN: &str = "https://api.nebius.cloud";
const MAX_BODY: u64 = 1_048_576;
const TIMEOUT: Duration = Duration::from_secs(10);

/// Token supplier is called only at an allowed read boundary. No Debug/Serialize
/// or secret accessors. Refresh/expiry is the caller's explicit responsibility.
/// Construct/use from synchronous code, outside a Tokio runtime.
/// ```compile_fail
/// use m8s_provider::nebius::readonly::ReadonlyHttps;
/// let client = ReadonlyHttps::new(|| Err(m8s_provider::Error::Unavailable)).unwrap();
/// println!("{client:?}");
/// ```
/// ```compile_fail
/// use m8s_provider::nebius::readonly::ReadonlyHttps;
/// let client = ReadonlyHttps::new(|| Err(m8s_provider::Error::Unavailable)).unwrap();
/// let _ = serde_json::to_string(&client);
/// ```
pub struct ReadonlyHttps<S> {
    client: reqwest::blocking::Client,
    supplier: S,
}
impl<S: FnMut() -> Result<SecretValue>> ReadonlyHttps<S> {
    /// Construction does not fetch tokens or contact Nebius.
    pub fn new(supplier: S) -> Result<Self> {
        let roots =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|_| Error::Unavailable)?
        .with_root_certificates(roots)
        .with_no_client_auth();
        let client = reqwest::blocking::Client::builder()
            .tls_backend_preconfigured(tls)
            .tls_sslkeylogfile(false)
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .referer(false)
            .connection_verbose(false)
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .connect_timeout(TIMEOUT)
            .timeout(TIMEOUT)
            .build()
            .map_err(|_| Error::Unavailable)?;
        Ok(Self { client, supplier })
    }
    fn read<T: DeserializeOwned>(
        &mut self,
        resource: Resource,
        project: &str,
        token: &str,
    ) -> Result<wire::Page<T>> {
        let url = read_url(resource, project, token)?;
        // Validate the route before obtaining/exposing a credential.
        let secret = (self.supplier)()?;
        let authorization = authorization(&secret)?;
        let response = self
            .client
            .get(url)
            .header(AUTHORIZATION, authorization)
            .header(ACCEPT, "application/json")
            .header(ACCEPT_ENCODING, "identity")
            .send()
            .map_err(|_| Error::Unavailable)?;
        let status = response.status().as_u16();
        // Do not inspect error bodies/Location, never turn 404 into absence.
        if status != 200 {
            return Err(Error::Unavailable);
        }
        let header = |name| -> Result<Option<&str>> {
            let values = response.headers().get_all(name).iter().collect::<Vec<_>>();
            match values.as_slice() {
                [] => Ok(None),
                [value] => value.to_str().map(Some).map_err(|_| Error::Unavailable),
                _ => Err(Error::Unavailable),
            }
        };
        validate_headers(
            header("content-type")?,
            header("content-encoding")?,
            header("content-length")?,
        )?;
        decode(response)
    }
}
impl<S: FnMut() -> Result<SecretValue>> Transport for ReadonlyHttps<S> {
    fn instances(&mut self, project: &str, token: &str) -> Result<wire::Page<wire::Vm>> {
        self.read(Resource::Instances, project, token)
    }
    fn platforms(&mut self, project: &str, token: &str) -> Result<wire::Page<wire::Platform>> {
        self.read(Resource::Platforms, project, token)
    }
    fn quotas(&mut self, project: &str, token: &str) -> Result<wire::Page<wire::Quota>> {
        self.read(Resource::Quotas, project, token)
    }
    // Defaults refuse fixture create/delete, before supplier or network access.
}
#[derive(Clone, Copy)]
enum Resource {
    Instances,
    Platforms,
    Quotas,
}
fn read_url(resource: Resource, project: &str, token: &str) -> Result<reqwest::Url> {
    if !component(project) || token.len() > 4096 {
        return Err(Error::Invalid);
    }
    let path = match resource {
        Resource::Instances => "/compute/v1/instances",
        Resource::Platforms => "/compute/v1/platforms",
        Resource::Quotas => "/quotas/v1/quota-allowances",
    };
    let mut url = reqwest::Url::parse(ORIGIN).map_err(|_| Error::Invalid)?;
    url.set_path(path);
    url.query_pairs_mut()
        .append_pair("parentId", project)
        .append_pair("pageSize", "1000")
        .append_pair("pageToken", token);
    Ok(url)
}
fn authorization(secret: &SecretValue) -> Result<HeaderValue> {
    let token = secret.expose();
    if token.is_empty()
        || token.len() > 16_384
        || !token
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(b))
    {
        return Err(Error::Invalid);
    }
    let mut bytes = b"Bearer ".to_vec();
    bytes.extend_from_slice(token);
    let mut header = HeaderValue::from_bytes(&bytes).map_err(|_| Error::Invalid)?;
    bytes.fill(0);
    header.set_sensitive(true);
    Ok(header)
}
fn validate_headers(
    content_type: Option<&str>,
    encoding: Option<&str>,
    length: Option<&str>,
) -> Result<()> {
    if !content_type.is_some_and(|s| {
        s.split(';')
            .next()
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"))
    }) || encoding.is_some_and(|s| !s.eq_ignore_ascii_case("identity"))
        || length.is_some_and(|s| s.parse::<u64>().map_or(true, |v| v > MAX_BODY))
    {
        return Err(Error::Unavailable);
    }
    Ok(())
}
fn decode<T: DeserializeOwned>(body: impl Read) -> Result<wire::Page<T>> {
    let mut bytes = Vec::new();
    body.take(MAX_BODY + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Unavailable)?;
    if bytes.len() as u64 > MAX_BODY {
        return Err(Error::Unavailable);
    }
    serde_json::from_slice(&bytes).map_err(|_| Error::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    #[test]
    fn constructor_and_mutations_never_obtain_credentials() {
        let calls = Cell::new(0);
        let mut client = ReadonlyHttps::new(|| {
            calls.set(calls.get() + 1);
            Err(Error::Unavailable)
        })
        .unwrap();
        assert_eq!(calls.get(), 0);
        assert!(client.delete_fixture("computeinstance-fixture").is_err());
        assert!(
            client
                .create_fixture(super::super::FixtureLaunch {
                    project: "project-fixture".into(),
                    platform: "fixture".into(),
                    preset: "fixture".into(),
                    labels: Default::default(),
                })
                .is_err()
        );
        assert!(client.instances("../other", "").is_err());
        assert_eq!(calls.get(), 0);
    }
    #[test]
    fn fixed_routes_and_escaped_pagination() {
        for (resource, path) in [
            (Resource::Instances, "/compute/v1/instances"),
            (Resource::Platforms, "/compute/v1/platforms"),
            (Resource::Quotas, "/quotas/v1/quota-allowances"),
        ] {
            let url = read_url(resource, "project-fixture", "a&parentId=foreign#/").unwrap();
            assert_eq!(url.origin().ascii_serialization(), ORIGIN);
            assert_eq!(url.path(), path);
            let pairs = url.query_pairs().collect::<Vec<_>>();
            assert_eq!(pairs.len(), 3);
            assert_eq!(pairs[0].1, "project-fixture");
            assert_eq!(pairs[2].1, "a&parentId=foreign#/");
        }
    }
    #[test]
    fn authentication_is_sensitive_and_rejects_header_injection() {
        let value = authorization(&SecretValue::new(b"synthetic-token".to_vec())).unwrap();
        assert!(value.is_sensitive());
        assert!(!format!("{value:?}").contains("synthetic-token"));
        for value in ["", "token\r\nx-secret: injected", "token space"] {
            assert!(authorization(&SecretValue::new(value.as_bytes().to_vec())).is_err());
        }
    }
    #[test]
    fn bounded_json_refuses_ambiguity_errors_and_encoding() {
        assert!(
            validate_headers(Some("application/json; charset=utf-8"), None, Some("10")).is_ok()
        );
        for (ct, enc, len) in [
            (None, None, None),
            (Some("text/html"), None, None),
            (Some("application/json"), Some("gzip"), None),
            (Some("application/json"), None, Some("1048577")),
            (Some("application/json"), None, Some("invalid")),
        ] {
            assert!(validate_headers(ct, enc, len).is_err());
        }
        for bytes in [
            br#"{"error":"synthetic-secret"}"#.as_slice(),
            br#"{"items":[],"items":[]}"#,
            br#"{"nextPageToken":null}"#,
            b"not json",
        ] {
            assert!(decode::<wire::Vm>(bytes).is_err());
        }
        assert!(decode::<wire::Vm>(vec![b' '; MAX_BODY as usize + 1].as_slice()).is_err());
        // Protobuf default omission is valid for a genuinely empty page.
        assert!(
            decode::<wire::Vm>(b"{}".as_slice())
                .unwrap()
                .items
                .is_empty()
        );
    }
}
