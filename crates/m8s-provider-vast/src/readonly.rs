//! Explicit optional read-only HTTPS. No environment/key-file/default provider.
//! Construction performs no provider I/O. All mutation is refused before supplier.
use super::{MAX_BODY, Method, Reply, Request, Transport, UniqueKeys, number, numeric_path_id};
use m8s_provider::{Error, Result};
use m8s_secrets::SecretValue;
use reqwest::header::{ACCEPT, ACCEPT_ENCODING, AUTHORIZATION, CONTENT_TYPE, HeaderValue};
use serde_json::{Value, json};
use std::{collections::BTreeSet, io::Read, sync::Arc, time::Duration};

#[cfg(feature = "launch-https")]
pub mod access;
#[cfg(feature = "scoped-destroy-https")]
pub mod destroy;
#[cfg(feature = "launch-https")]
pub mod launch;
#[cfg(feature = "launch-https")]
pub mod preflight;
#[cfg(feature = "launch-https")]
pub mod provisioning_log;
#[cfg(feature = "scoped-destroy-https")]
pub mod recovery;

const ORIGIN: &str = "https://console.vast.ai";
const TIMEOUT: Duration = Duration::from_secs(10);

/// No Debug/Serialize/Clone or key accessors. Supplier is invoked per allowed read
/// only, never during construction. Use from a synchronous context, not Tokio.
/// Replies contain WHITELISTED JSON facts, not raw account/provider responses.
/// Live API/schema/TLS qualification is outstanding. No live authorization implied.
/// ```compile_fail
/// use m8s_provider_vast::readonly::ReadonlyHttps;
/// use m8s_secrets::SecretValue;
/// let value = ReadonlyHttps::new(|| Ok(SecretValue::new(vec![]))).unwrap();
/// println!("{value:?}");
/// ```
/// ```compile_fail
/// use m8s_provider_vast::readonly::ReadonlyHttps;
/// use m8s_secrets::SecretValue;
/// let value = ReadonlyHttps::new(|| Ok(SecretValue::new(vec![]))).unwrap();
/// let _ = serde_json::to_string(&value);
/// ```
pub struct ReadonlyHttps<S> {
    backend: ReqwestBackend,
    supplier: S,
}
impl<S: FnMut() -> Result<SecretValue>> ReadonlyHttps<S> {
    pub fn new(supplier: S) -> Result<Self> {
        let config = tls_config()?;
        let client = reqwest::blocking::Client::builder()
            .tls_backend_preconfigured(config)
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
        Ok(Self {
            backend: ReqwestBackend(client, None),
            supplier,
        })
    }
}
impl<S: FnMut() -> Result<SecretValue>> Transport for ReadonlyHttps<S> {
    fn exchange(&mut self, request: Request) -> Result<Reply> {
        exchange_with(&mut self.backend, &mut self.supplier, request)
    }
}

fn tls_config() -> Result<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let roots = rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| Error::Unavailable)
        .map(|builder| builder.with_root_certificates(roots).with_no_client_auth())
}

// Private trusted backend seam; cannot inject arbitrary URLs/headers at public API.
struct WireRequest {
    method: reqwest::Method,
    url: String,
    authorization: HeaderValue,
    body: Option<Vec<u8>>,
}
struct WireReply {
    status: u16,
    content_type: Option<String>,
    encoding: Option<String>,
    declared_length: Option<u64>,
    body: Box<dyn Read>,
}
trait Backend {
    fn send(&mut self, request: WireRequest) -> Result<WireReply>;
}
struct ReqwestBackend(reqwest::blocking::Client, Option<&'static str>);
impl Backend for ReqwestBackend {
    fn send(&mut self, request: WireRequest) -> Result<WireReply> {
        let mut builder = self
            .0
            .request(request.method, &request.url)
            .header(AUTHORIZATION, request.authorization)
            .header(ACCEPT, "application/json")
            .header(ACCEPT_ENCODING, "identity");
        if let Some(body) = request.body {
            builder = builder.header(CONTENT_TYPE, "application/json").body(body);
        }
        self.1 = None;
        let response = builder.send().map_err(|error| {
            self.1 = Some(if error.is_connect() {
                "vast-connection-failed"
            } else if error.is_timeout() {
                "vast-request-timeout"
            } else {
                "vast-transport-failed"
            });
            Error::Unavailable
        })?;
        let header = |name| unique_header(response.headers(), name);
        let status = response.status().as_u16();
        // Never inspect error/redirect bodies or Location headers.
        if status != 200 {
            self.1 = Some(match status {
                401 => "vast-http-unauthorized",
                403 => "vast-http-forbidden",
                404 => "vast-http-not-found",
                301 | 302 | 307 | 308 => "vast-http-redirect-refused",
                _ => "vast-http-status-refused",
            });
            return Err(Error::Unavailable);
        }
        let content_type = header("content-type")?;
        let encoding = header("content-encoding")?;
        let length = header("content-length")?;
        let declared_length = length
            .map(|v| v.parse::<u64>().map_err(|_| Error::Unavailable))
            .transpose()?;
        Ok(WireReply {
            status,
            content_type,
            encoding,
            declared_length,
            body: Box::new(response),
        })
    }
}

#[derive(Clone, Copy)]
enum ReadKind {
    Account,
    Offers,
    Describe(u64),
    List,
}
fn allowed(request: &Request) -> Result<ReadKind> {
    match (&request.method, request.path.as_str(), &request.body) {
        (Method::Get, "/api/v0/users/current" | "/api/v0/users/current/", None) => {
            Ok(ReadKind::Account)
        }
        (Method::Get, "/api/v1/instances", None) => Ok(ReadKind::List),
        (Method::Get, path, None) if path.starts_with("/api/v0/instances/") => {
            numeric_path_id(&path["/api/v0/instances/".len()..]).map_err(|_| Error::Unavailable)?;
            Ok(ReadKind::Describe(
                path["/api/v0/instances/".len()..]
                    .parse()
                    .map_err(|_| Error::Unavailable)?,
            ))
        }
        (Method::Post, "/api/v0/bundles", Some(body)) if *body == offer_query() => {
            Ok(ReadKind::Offers)
        }
        _ => Err(Error::Unavailable),
    }
}
/// Exact official query; no presets, credentials, onstart or user-supplied options.
pub fn offer_query() -> Value {
    json!({"limit":10,"type":"on-demand","verified":{"eq":true},
           "rentable":{"eq":true},"rented":{"eq":false}})
}
fn exchange_with<B: Backend, S: FnMut() -> Result<SecretValue>>(
    backend: &mut B,
    supplier: &mut S,
    request: Request,
) -> Result<Reply> {
    let kind = allowed(&request)?; // Reject paths/queries/mutation BEFORE secret/network.
    let secret = supplier().map_err(|_| Error::Unavailable)?;
    let bytes = secret.expose();
    if bytes.is_empty() || bytes.len() > 4096 || !bytes.iter().all(|b| (33..=126).contains(b)) {
        return Err(Error::Unavailable);
    }
    let bearer = SecretValue::new([b"Bearer ".as_slice(), bytes].concat());
    let mut authorization =
        HeaderValue::from_bytes(bearer.expose()).map_err(|_| Error::Unavailable)?;
    authorization.set_sensitive(true);
    let body = request
        .body
        .map(|v| serde_json::to_vec(&v).map_err(|_| Error::Unavailable))
        .transpose()?;
    let reply = backend
        .send(WireRequest {
            method: if matches!(request.method, Method::Post) {
                reqwest::Method::POST
            } else {
                reqwest::Method::GET
            },
            url: match kind {
                // Live canonical route; do not follow the slashless 301 with auth.
                ReadKind::Account => format!("{ORIGIN}/api/v0/users/current/"),
                ReadKind::List => format!("{ORIGIN}/api/v1/instances/"),
                ReadKind::Offers => format!("{ORIGIN}/api/v0/bundles/"),
                _ => format!("{ORIGIN}{}", request.path),
            },
            authorization,
            body,
        })
        .map_err(|_| Error::Unavailable)?;
    let value = checked_json(reply, bytes, MAX_BODY)?;
    let facts = sanitize(kind, &value).map_err(|_| Error::Unavailable)?;
    let body = serde_json::to_vec(&facts).map_err(|_| Error::Unavailable)?;
    // Never infer full-account completeness from HTTP status, empty rows, counts,
    // absent cursor, 404, or provider success. No pagination qualification yet.
    Ok(Reply {
        status: 200,
        body,
        complete_instance_snapshot: false,
    })
}
fn checked_json(reply: WireReply, credential: &[u8], limit: usize) -> Result<Value> {
    if limit == 0 || limit > MAX_BODY {
        return Err(Error::Unavailable);
    }
    if reply.status != 200
        || reply
            .content_type
            .as_deref()
            .map(|s| s.split(';').next().unwrap_or("").trim())
            != Some("application/json")
        || !matches!(reply.encoding.as_deref(), None | Some("identity"))
        || reply.declared_length.is_some_and(|n| n > limit as u64)
    {
        return Err(Error::Unavailable);
    }
    let mut raw = Vec::new();
    reply
        .body
        .take((limit + 1) as u64)
        .read_to_end(&mut raw)
        .map_err(|_| Error::Unavailable)?;
    if raw.len() > limit || reply.declared_length.is_some_and(|n| n != raw.len() as u64) {
        return Err(Error::Unavailable);
    }
    serde_json::from_slice::<UniqueKeys>(&raw).map_err(|_| Error::Unavailable)?;
    let value: Value = serde_json::from_slice(&raw).map_err(|_| Error::Unavailable)?;
    if reflects_secret(
        &value,
        std::str::from_utf8(credential).map_err(|_| Error::Unavailable)?,
    ) {
        return Err(Error::Unavailable);
    }
    let object = value.as_object().ok_or(Error::Unavailable)?;
    if object.contains_key("error")
        || object.contains_key("errors")
        || object
            .get("success")
            .is_some_and(|v| v != &Value::Bool(true))
    {
        return Err(Error::Unavailable);
    }
    Ok(value)
}
fn instance(row: &Value) -> Result<Value> {
    let id = row
        .get("id")
        .and_then(Value::as_u64)
        .filter(|n| *n > 0)
        .ok_or(Error::Invalid)?;
    let label = match row.get("label") {
        Some(Value::Null) => Value::Null,
        Some(Value::String(label)) if label.len() <= 256 => {
            if super::parse_label(label).is_some() {
                json!(label)
            } else {
                Value::Null
            }
        }
        _ => return Err(Error::Invalid),
    };
    let state = row
        .get("actual_status")
        .and_then(Value::as_str)
        .ok_or(Error::Invalid)?;
    if !matches!(
        state,
        "running" | "loading" | "created" | "stopped" | "exited" | "offline" | "unknown"
    ) {
        return Err(Error::Invalid);
    }
    Ok(json!({"id":id,"label":label,"actual_status":state}))
}
fn sanitize(kind: ReadKind, value: &Value) -> Result<Value> {
    match kind {
        ReadKind::Account => {
            // Credit is a separate provider field. A zero balance does not mean
            // the account has no rental credit. Missing credit stays unknown;
            // never substitute balance or round available funds upwards.
            let credit = value
                .get("credit")
                .filter(|v| !v.is_null())
                .map(|_| number(value, "credit", 1_000_000, false))
                .transpose()?;
            Ok(
                json!({"balance_micro_usd":number(value,"balance",1_000_000,false)?,
                "credit_micro_usd":credit,"auto_topup":"unknown"}),
            )
        }
        ReadKind::Describe(expected) => {
            let row = instance(value.get("instances").ok_or(Error::Invalid)?)?;
            if row["id"].as_u64() != Some(expected) {
                return Err(Error::Invalid);
            }
            Ok(json!({"instances":row}))
        }
        ReadKind::List => {
            let rows = value
                .get("instances")
                .and_then(Value::as_array)
                .filter(|r| r.len() <= 1024)
                .ok_or(Error::Invalid)?;
            let mut seen = BTreeSet::new();
            let rows: Vec<_> = rows
                .iter()
                .map(|r| {
                    let fact = instance(r)?;
                    if !seen.insert(fact["id"].as_u64().ok_or(Error::Invalid)?) {
                        return Err(Error::Invalid);
                    }
                    Ok(fact)
                })
                .collect::<Result<_>>()?;
            Ok(json!({"instances":rows,"pagination_qualified":false}))
        }
        ReadKind::Offers => {
            // Official example currently shows an object, historical fixture a list.
            // Support only bounded explicit list schema, never silently wrap objects.
            let rows = value
                .get("offers")
                .and_then(Value::as_array)
                .filter(|r| r.len() <= 10)
                .ok_or(Error::Invalid)?;
            let mut seen = BTreeSet::new();
            let mut facts = Vec::new();
            for row in rows {
                let id = row
                    .get("id")
                    .and_then(Value::as_u64)
                    .filter(|n| *n > 0)
                    .ok_or(Error::Invalid)?;
                if !seen.insert(id) {
                    return Err(Error::Invalid);
                }
                let mut fact = json!({"id":id,"pricing_qualified":false});
                for key in [
                    "dph_total",
                    "storage_cost",
                    "inet_down_cost",
                    "inet_up_cost",
                    "inet_down",
                ] {
                    number(row, key, 1_000_000, true)?;
                    fact[key] = row[key].clone();
                }
                // Raw rates deliberately unqualified: official units conflict.
                facts.push(fact);
            }
            Ok(json!({"offers":facts,"pricing_qualified":false}))
        }
    }
}

fn unique_header(headers: &reqwest::header::HeaderMap, name: &str) -> Result<Option<String>> {
    let mut values = headers.get_all(name).iter();
    let value = values
        .next()
        .map(|v| {
            v.to_str()
                .map(str::to_owned)
                .map_err(|_| Error::Unavailable)
        })
        .transpose()?;
    if values.next().is_some() {
        return Err(Error::Unavailable);
    }
    Ok(value)
}
fn reflects_secret(value: &Value, secret: &str) -> bool {
    match value {
        Value::String(v) => v.contains(secret),
        Value::Number(v) => v.to_string().contains(secret),
        Value::Array(v) => v.iter().any(|v| reflects_secret(v, secret)),
        Value::Object(v) => v
            .iter()
            .any(|(k, v)| k.contains(secret) || reflects_secret(v, secret)),
        _ => false,
    }
}

#[cfg(test)]
mod tests;
