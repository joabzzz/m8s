//! Positive-only recovery of an uncertain creation by its exact owner intent.
//! Empty, partial and ambiguous results never establish provider absence.
use super::*;
use std::num::NonZeroU64;

/// Nonsecret immutable launch identity. Syntax is not image provenance; the
/// caller must supply its durable, approved image and never invent a retry intent.
#[derive(Clone)]
pub struct RecoveryIntent {
    label: String,
    image: String,
}
impl RecoveryIntent {
    pub fn new(owner: &str, node_uid: &str, tier: &str, image: &str) -> Result<Self> {
        if ![owner, node_uid, tier]
            .into_iter()
            .all(super::super::component)
            || !super::super::pinned_image(image)
        {
            return Err(Error::Invalid);
        }
        Ok(Self {
            label: format!("m8s:v1:{owner}:{node_uid}:{tier}"),
            image: image.into(),
        })
    }
    pub fn label(&self) -> &str {
        &self.label
    }
    pub fn image(&self) -> &str {
        &self.image
    }
}

/// Exactly one row matched the complete owner/node/tier label AND approved image.
/// Persist this binding before mutation. This is not admission, Ready, or absence.
pub struct RecoveryCandidate {
    id: NonZeroU64,
}
impl RecoveryCandidate {
    pub fn instance_id(&self) -> NonZeroU64 {
        self.id
    }
}

/// Read-only lookup scoped at construction. Never launches, destroys, retries,
/// accepts arbitrary filters/URLs, or searches unrelated resources for adoption.
pub struct RecoveryLookup<S> {
    transport: ReadonlyHttps<S>,
    intent: RecoveryIntent,
}
impl<S: FnMut() -> Result<SecretValue>> RecoveryLookup<S> {
    pub fn new(intent: RecoveryIntent, supplier: S) -> Result<Self> {
        Ok(Self {
            transport: ReadonlyHttps::new(supplier)?,
            intent,
        })
    }
    /// None means unresolved, including an empty successful filtered response.
    /// Keep the original liability and cleanup intent; never submit another create.
    pub fn find(&mut self) -> Result<Option<RecoveryCandidate>> {
        lookup_with(
            &mut self.transport.backend,
            &mut self.transport.supplier,
            &self.intent,
        )
    }
}

fn lookup_with<B: Backend, S: FnMut() -> Result<SecretValue>>(
    backend: &mut B,
    supplier: &mut S,
    intent: &RecoveryIntent,
) -> Result<Option<RecoveryCandidate>> {
    let mut url = reqwest::Url::parse(&format!("{ORIGIN}/api/v1/instances/"))
        .map_err(|_| Error::Unavailable)?;
    // Provider filtering narrows disclosure; local exact matching remains mandatory.
    url.query_pairs_mut()
        .append_pair("limit", "2")
        .append_pair("select_cols", r#"["id","label","image_uuid"]"#)
        .append_pair(
            "select_filters",
            &json!({"label":{"eq":intent.label}}).to_string(),
        );
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
            method: reqwest::Method::GET,
            url: url.to_string(),
            authorization,
            body: None,
        })
        .map_err(|_| Error::Unavailable)?;
    let value = checked_json(reply, bytes, 65536)?;
    let rows = value
        .get("instances")
        .and_then(Value::as_array)
        .ok_or(Error::Unavailable)?;
    if value.get("success") != Some(&Value::Bool(true))
        || rows.len() > 1
        || value.get("next_token") != Some(&Value::Null)
        || value.get("instances_found").and_then(Value::as_u64) != Some(rows.len() as u64)
        || value.get("total_instances").and_then(Value::as_u64) != Some(rows.len() as u64)
    {
        return Err(Error::Unavailable);
    }
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    if row.get("label").and_then(Value::as_str) != Some(intent.label.as_str())
        || row.get("image_uuid").and_then(Value::as_str) != Some(intent.image.as_str())
    {
        return Err(Error::Unavailable);
    }
    let id = row
        .get("id")
        .and_then(Value::as_u64)
        .and_then(NonZeroU64::new)
        .ok_or(Error::Unavailable)?;
    Ok(Some(RecoveryCandidate { id }))
}

#[cfg(test)]
mod tests {
    use super::super::tests::{Fake, key};
    use super::*;
    fn intent() -> RecoveryIntent {
        RecoveryIntent::new(
            "synthetic-owner",
            "synthetic-node",
            "smoke",
            &format!("example.invalid/mesh@sha256:{}", "a".repeat(64)),
        )
        .unwrap()
    }
    fn response(intent: &RecoveryIntent) -> Value {
        json!({"success":true,"next_token":null,"instances_found":1,"total_instances":1,
            "instances":[{"id":42,"label":intent.label(),"image_uuid":intent.image()}]})
    }
    fn lookup(value: &Value, intent: &RecoveryIntent) -> Result<Option<RecoveryCandidate>> {
        lookup_with(
            &mut Fake::json(&serde_json::to_vec(value).unwrap()),
            &mut key,
            intent,
        )
    }
    #[test]
    fn positive_recovery_is_exact_and_empty_is_unresolved() {
        let intent = intent();
        let mut backend = Fake::json(&serde_json::to_vec(&response(&intent)).unwrap());
        assert_eq!(
            lookup_with(&mut backend, &mut key, &intent)
                .unwrap()
                .unwrap()
                .instance_id()
                .get(),
            42
        );
        let sent = backend.request.unwrap();
        let url = reqwest::Url::parse(&sent.url).unwrap();
        assert_eq!(url.origin().ascii_serialization(), ORIGIN);
        assert_eq!(url.path(), "/api/v1/instances/");
        let fields: std::collections::BTreeMap<_, _> = url.query_pairs().collect();
        assert_eq!(fields["limit"], "2");
        assert_eq!(
            serde_json::from_str::<Value>(&fields["select_filters"]).unwrap(),
            json!({"label":{"eq":intent.label()}})
        );
        assert_eq!(sent.method, reqwest::Method::GET);
        assert!(sent.authorization.is_sensitive());
        assert!(sent.body.is_none());
        let empty = json!({"success":true,"next_token":null,"instances_found":0,"total_instances":0,"instances":[]});
        assert!(lookup(&empty, &intent).unwrap().is_none());
    }
    #[test]
    fn other_owners_images_partial_or_ambiguous_rows_cannot_be_adopted() {
        let intent = intent();
        for mutation in 0..8 {
            let mut value = response(&intent);
            match mutation {
                0 => {
                    value["instances"][0]["label"] =
                        json!("m8s:v1:another-owner:synthetic-node:smoke")
                }
                1 => value["instances"][0]["image_uuid"] = json!("example.invalid/mesh:latest"),
                2 => value["next_token"] = json!("next-page"),
                3 => value["total_instances"] = json!(2),
                4 => value["instances_found"] = json!(0),
                5 => value["success"] = json!(false),
                6 => value["instances"][0]["id"] = json!(0),
                _ => {
                    let row = value["instances"][0].clone();
                    value["instances"].as_array_mut().unwrap().push(row);
                }
            }
            assert!(lookup(&value, &intent).is_err());
        }
    }
    #[test]
    fn invalid_intents_fail_before_transport() {
        assert!(RecoveryIntent::new("owner:other", "node", "tier", intent().image()).is_err());
        assert!(RecoveryIntent::new("owner", "node", "tier", "image:latest").is_err());
    }
}
