//! Obtain only the SSH host public key from this exact instance's provider logs.
//! Raw logs, result URLs and credentials are never exposed, Debug or Serialize.
use super::*;
use access::NodeAccess;
use std::num::NonZeroU64;

const PREFIX: &str = "M8S_PROVISION_HOST_KEY_V1 ";
pub struct ProvisioningLog<S> {
    transport: ReadonlyHttps<S>,
    instance: NonZeroU64,
}
pub struct HostPublicKey(String, NonZeroU64);
impl HostPublicKey {
    pub fn as_openssh(&self) -> &str {
        &self.0
    }
    pub fn matches_access(&self, access: &NodeAccess) -> bool {
        self.1 == access.instance_id()
    }
}
impl<S: FnMut() -> Result<SecretValue>> ProvisioningLog<S> {
    pub fn new(access: &NodeAccess, supplier: S) -> Result<Self> {
        Ok(Self {
            transport: ReadonlyHttps::new(supplier)?,
            instance: access.instance_id(),
        })
    }
    /// One exact log request, then bounded polling of that same object while
    /// generation is pending. Authorization failures and redirects are not retried.
    /// The coordinator additionally owns its fixed overall polling deadline.
    pub fn host_key(&mut self) -> Result<HostPublicKey> {
        let secret = (self.transport.supplier)().map_err(|_| Error::Unavailable)?;
        let bytes = secret.expose();
        if bytes.is_empty() || bytes.len() > 4096 || !bytes.iter().all(|b| (33..=126).contains(b)) {
            return Err(Error::Unavailable);
        }
        let bearer = SecretValue::new([b"Bearer ".as_slice(), bytes].concat());
        let mut authorization =
            HeaderValue::from_bytes(bearer.expose()).map_err(|_| Error::Unavailable)?;
        authorization.set_sensitive(true);
        let reply = self
            .transport
            .backend
            .send(WireRequest {
                method: reqwest::Method::PUT,
                url: format!("{ORIGIN}/api/v0/instances/request_logs/{}/", self.instance),
                authorization,
                body: Some(br#"{"tail":"100","filter":"M8S_PROVISION_HOST_KEY_V1"}"#.to_vec()),
            })
            .map_err(|_| Error::Unavailable)?;
        let value = checked_json(reply, bytes, 8192)?;
        let url = log_url(
            value
                .get("result_url")
                .and_then(Value::as_str)
                .ok_or(Error::Unavailable)?,
        )?;
        // The fixed TLS/no-proxy/no-redirect/no-retry client has NO default auth.
        // Never attach the provider account header to the object-storage request.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(9);
        let response = loop {
            let left = deadline
                .checked_duration_since(std::time::Instant::now())
                .ok_or(Error::Unavailable)?;
            let response = self
                .transport
                .backend
                .0
                .get(url.clone())
                .header(ACCEPT_ENCODING, "identity")
                .timeout(left.min(std::time::Duration::from_secs(3)))
                .send()
                .map_err(|_| Error::Unavailable)?;
            if !log_generation_pending(response.status()) {
                break response;
            }
            // The official provider CLI polls a newly requested log object.
            // Keep the original URL and never resend request_logs here.
            std::thread::sleep(std::time::Duration::from_millis(300).min(left));
        };
        if response.status() != reqwest::StatusCode::OK
            || unique_header(response.headers(), "content-encoding")?
                .is_some_and(|s| s != "identity")
            || response.content_length().is_some_and(|n| n > 65536)
        {
            return Err(Error::Unavailable);
        }
        let mut log = Vec::new();
        response
            .take(65537)
            .read_to_end(&mut log)
            .map_err(|_| Error::Unavailable)?;
        if log.len() > 65536 || log.windows(bytes.len()).any(|w| w == bytes) {
            return Err(Error::Unavailable);
        }
        host_key_from_log(&log, self.instance)
    }
}

fn log_generation_pending(status: reqwest::StatusCode) -> bool {
    matches!(
        status,
        reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::NOT_FOUND
    )
}

fn log_url(text: &str) -> Result<reqwest::Url> {
    if text.len() > 8192 {
        return Err(Error::Unavailable);
    }
    let url = reqwest::Url::parse(text).map_err(|_| Error::Unavailable)?;
    // Explicit object-store routes only, never a provider-controlled arbitrary
    // destination, IP literal, local network URL, redirect, or authenticated URI.
    let allowed = match url.host_str() {
        Some("s3.amazonaws.com") => {
            url.path().starts_with("/vast.ai/instance_logs/")
                || url.path().starts_with("/public.vast.ai/instance_logs/")
        }
        Some("vast.ai.s3.amazonaws.com") => url.path().starts_with("/instance_logs/"),
        _ => false,
    };
    if !allowed
        || url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Unavailable);
    }
    Ok(url)
}

fn host_key_from_log(bytes: &[u8], instance: NonZeroU64) -> Result<HostPublicKey> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error::Unavailable)?;
    let mut found = None;
    for line in text.lines() {
        let Some(key) = line.strip_prefix(PREFIX) else {
            continue;
        };
        let encoded = key.strip_prefix("ssh-ed25519 ").ok_or(Error::Unavailable)?;
        if found.is_some()
            || encoded.len() != 68
            || !encoded.starts_with("AAAAC3NzaC1lZDI1NTE5AAAAI")
            || !encoded
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+/".contains(&b))
        {
            return Err(Error::Unavailable);
        }
        found = Some(HostPublicKey(key.into(), instance));
    }
    found.ok_or(Error::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn host_key_from_log(bytes: &[u8]) -> Result<HostPublicKey> {
        super::host_key_from_log(bytes, NonZeroU64::new(42).unwrap())
    }
    #[test]
    fn log_polling_is_only_for_pending_generation() {
        assert!(log_generation_pending(reqwest::StatusCode::ACCEPTED));
        assert!(log_generation_pending(reqwest::StatusCode::NOT_FOUND));
        for code in [200, 301, 302, 307, 401, 403, 429, 500, 503] {
            assert!(!log_generation_pending(
                reqwest::StatusCode::from_u16(code).unwrap()
            ));
        }
    }
    #[test]
    fn only_fixed_https_object_storage_routes_are_accepted() {
        for url in [
            "https://s3.amazonaws.com/vast.ai/instance_logs/synthetic.log",
            "https://s3.amazonaws.com/public.vast.ai/instance_logs/synthetic.log",
            "https://vast.ai.s3.amazonaws.com/instance_logs/synthetic.log",
        ] {
            assert!(log_url(url).is_ok());
        }
        for url in [
            "http://s3.amazonaws.com/vast.ai/instance_logs/x",
            "https://127.0.0.1/vast.ai/instance_logs/x",
            "https://s3.amazonaws.com.evil.invalid/vast.ai/instance_logs/x",
            "https://s3.amazonaws.com/other-bucket/x",
            "https://s3.amazonaws.com/public.vast.ai/other-path/x",
            "https://s3.amazonaws.com/public.vast.ai.evil/instance_logs/x",
            "https://user@s3.amazonaws.com/vast.ai/instance_logs/x",
            "https://s3.amazonaws.com:444/vast.ai/instance_logs/x",
            "https://s3.amazonaws.com/vast.ai/instance_logs/x#fragment",
        ] {
            assert!(log_url(url).is_err());
        }
    }
    #[test]
    fn only_one_complete_public_key_line_is_returned() {
        let key = format!(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI{}",
            "A".repeat(68 - "AAAAC3NzaC1lZDI1NTE5AAAAI".len())
        );
        let line = format!("{PREFIX}{key}\n");
        assert_eq!(
            host_key_from_log(format!("unrelated synthetic output\n{line}").as_bytes())
                .unwrap()
                .as_openssh(),
            key
        );
        for value in [
            String::new(),
            format!("{line}{line}"),
            format!("{PREFIX}{key} comment\n"),
            format!("leading text {line}"),
            format!("{PREFIX}ssh-rsa synthetic\n"),
        ] {
            assert!(host_key_from_log(value.as_bytes()).is_err());
        }
    }
}
