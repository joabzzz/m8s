use super::*;
use std::{cell::Cell, io::Cursor};
const SYNTHETIC: &str = "SYNTHETIC-ONLY-FAKE-ACCOUNT-KEY";
pub(super) struct Fake {
    pub(super) calls: usize,
    pub(super) request: Option<WireRequest>,
    pub(super) reply: Option<WireReply>,
}
impl Fake {
    pub(super) fn json(body: &[u8]) -> Self {
        Self {
            calls: 0,
            request: None,
            reply: Some(WireReply {
                status: 200,
                content_type: Some("application/json; charset=utf-8".into()),
                encoding: None,
                declared_length: None,
                body: Box::new(Cursor::new(body.to_vec())),
            }),
        }
    }
}
impl Backend for Fake {
    fn send(&mut self, request: WireRequest) -> Result<WireReply> {
        self.calls += 1;
        self.request = Some(request);
        self.reply.take().ok_or(Error::Unavailable)
    }
}
pub(super) fn key() -> Result<SecretValue> {
    Ok(SecretValue::new(SYNTHETIC.as_bytes().to_vec()))
}
fn request(method: Method, path: &str, body: Option<Value>) -> Request {
    Request {
        method,
        path: path.into(),
        body,
    }
}
fn account() -> Request {
    request(Method::Get, "/api/v0/users/current", None)
}
fn failed(result: Result<Reply>) {
    let error = match result {
        Ok(_) => panic!("expected opaque failure"),
        Err(e) => e,
    };
    assert!(matches!(error, Error::Unavailable));
    assert!(!format!("{error:?} {error}").contains(SYNTHETIC));
    assert!(std::error::Error::source(&error).is_none());
}
#[test]
fn construction_does_not_consume_supplier_or_connect() {
    let called = Cell::new(false);
    let _transport = ReadonlyHttps::new(|| {
        called.set(true);
        key()
    })
    .unwrap();
    assert!(!called.get());
}
#[test]
fn fixed_origin_sensitive_header_and_sanitized_balance() {
    let mut fake = Fake::json(
        br#"{"balance":6.0000009,"email":"secret@example.invalid","ssh_key":"not-public"}"#,
    );
    let reply = exchange_with(&mut fake, &mut key, account()).unwrap();
    assert!(!reply.complete_instance_snapshot);
    assert_eq!(
        serde_json::from_slice::<Value>(&reply.body).unwrap(),
        json!({"balance_micro_usd":6000000,"credit_micro_usd":null,"auto_topup":"unknown"})
    );
    let sent = fake.request.as_ref().unwrap();
    assert_eq!(sent.url, "https://console.vast.ai/api/v0/users/current/");
    assert_eq!(sent.method, reqwest::Method::GET);
    assert!(sent.authorization.is_sensitive());
    assert!(!format!("{:?}", sent.authorization).contains(SYNTHETIC));
    assert_eq!(fake.calls, 1);
}
#[test]
fn account_credit_is_distinct_from_balance_and_rounded_down() {
    let mut fake = Fake::json(br#"{"balance":0,"credit":6.1250009}"#);
    let reply = exchange_with(&mut fake, &mut key, account()).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&reply.body).unwrap(),
        json!({"balance_micro_usd":0,"credit_micro_usd":6125000,"auto_topup":"unknown"})
    );
    for credit in [json!(-1), json!("6"), json!({}), json!(true)] {
        let mut fake =
            Fake::json(&serde_json::to_vec(&json!({"balance":0,"credit":credit})).unwrap());
        assert!(exchange_with(&mut fake, &mut key, account()).is_err());
    }
}
#[test]
fn mutations_bad_paths_queries_bodies_never_consume_secret_or_wire() {
    let requests = vec![
        request(Method::Delete, "/api/v0/instances/1", None),
        request(Method::Put, "/api/v0/asks/1/", Some(json!({}))),
        request(
            Method::Get,
            "https://evil.invalid/api/v0/users/current",
            None,
        ),
        request(Method::Get, "/api/v0/users/current?api_key=synthetic", None),
        request(Method::Get, "//evil.invalid/", None),
        request(Method::Get, "/api/v0/instances/01", None),
        request(Method::Get, "/api/v0/instances/1/", None),
        request(Method::Get, "/api/v0/instances/", None), // historical snapshot unqualified
        request(Method::Get, "/api/v0/users/current", Some(json!({}))),
        request(Method::Post, "/api/v0/bundles", Some(json!({"type":"bid"}))),
        request(
            Method::Post,
            "/api/v0/bundles",
            Some(json!({"onstart":"bad"})),
        ),
    ];
    for req in requests {
        let mut fake = Fake::json(b"{}");
        let calls = Cell::new(0);
        failed(exchange_with(
            &mut fake,
            &mut || {
                calls.set(calls.get() + 1);
                key()
            },
            req,
        ));
        assert_eq!(calls.get(), 0);
        assert_eq!(fake.calls, 0);
    }
}
#[test]
fn offer_query_exact_and_object_schema_refused() {
    let mut fake = Fake::json(br#"{"offers":[]}"#);
    let reply = exchange_with(
        &mut fake,
        &mut key,
        request(Method::Post, "/api/v0/bundles", Some(offer_query())),
    )
    .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&reply.body).unwrap(),
        json!({"offers":[],"pricing_qualified":false})
    );
    let wire = fake.request.unwrap();
    assert_eq!(wire.url, "https://console.vast.ai/api/v0/bundles/");
    assert_eq!(
        serde_json::from_slice::<Value>(wire.body.as_ref().unwrap()).unwrap(),
        offer_query()
    );
    let mut fake = Fake::json(br#"{"offers":{"id":123}}"#);
    failed(exchange_with(
        &mut fake,
        &mut key,
        request(Method::Post, "/api/v0/bundles", Some(offer_query())),
    ));
}
#[test]
fn partial_or_empty_list_never_complete_and_foreign_text_dropped() {
    for body in [br#"{"instances":[],"next_token":"opaque"}"#.as_slice(),
                 br#"{"instances":[],"total_instances":0,"instances_found":0}"#,
                 br#"{"instances":[{"id":1,"label":"private arbitrary text","actual_status":"stopped","extra_env":["private"]}]}"#] {
        let mut fake = Fake::json(body);
        let reply = exchange_with(&mut fake,&mut key,request(Method::Get,"/api/v1/instances",None)).unwrap();
        assert!(!reply.complete_instance_snapshot);
        let text = String::from_utf8(reply.body).unwrap();
        assert!(!text.contains("private"));
        assert!(!text.contains("opaque"));
    }
}
#[test]
fn describe_returns_only_label_status_id_not_absence() {
    let mut fake = Fake::json(br#"{"instances":{"id":1,"label":"m8s:v1:owner:node:tier","actual_status":"running","onstart":"private"}}"#);
    let reply = exchange_with(
        &mut fake,
        &mut key,
        request(Method::Get, "/api/v0/instances/1", None),
    )
    .unwrap();
    assert!(!reply.complete_instance_snapshot);
    assert!(!String::from_utf8(reply.body).unwrap().contains("private"));
    let mut fake = Fake::json(b"{}");
    fake.reply.as_mut().unwrap().status = 404;
    failed(exchange_with(
        &mut fake,
        &mut key,
        request(Method::Get, "/api/v0/instances/1", None),
    ));
}
#[test]
fn errors_redirects_duplicates_and_invalid_json_are_opaque_no_retry() {
    for status in [301, 302, 307, 308, 401, 403, 404, 429, 500] {
        let mut fake = Fake::json(SYNTHETIC.as_bytes());
        fake.reply.as_mut().unwrap().status = status;
        failed(exchange_with(&mut fake, &mut key, account()));
        assert_eq!(fake.calls, 1);
    }
    for body in [
        br#"{"balance":1,"balance":6}"#.as_slice(),
        br#"{"success":false}"#,
        br#"{"balance":NaN}"#,
        b"[]",
        b"not-json",
    ] {
        let mut fake = Fake::json(body);
        failed(exchange_with(&mut fake, &mut key, account()));
    }
}
#[test]
fn invalid_secret_refused_before_backend() {
    for bytes in [b"".as_slice(), b"a\nb", b"a b", b"a\rb", &[0xff]] {
        let mut fake = Fake::json(b"{}");
        failed(exchange_with(
            &mut fake,
            &mut || Ok(SecretValue::new(bytes.to_vec())),
            account(),
        ));
        assert_eq!(fake.calls, 0);
    }
}
#[test]
fn body_content_length_encoding_limits_and_io_timeout() {
    struct Fails;
    impl Read for Fails {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(std::io::ErrorKind::TimedOut, SYNTHETIC))
        }
    }
    let mut replies = vec![];
    let mut fake = Fake::json(b"{}");
    let mut r = fake.reply.take().unwrap();
    r.body = Box::new(Fails);
    replies.push(r);
    for mode in 0..5 {
        let mut r = Fake::json(b"{}").reply.unwrap();
        match mode {
            0 => r.body = Box::new(Cursor::new(vec![b'x'; MAX_BODY + 1])),
            1 => r.declared_length = Some(MAX_BODY as u64 + 1),
            2 => r.declared_length = Some(100),
            3 => r.encoding = Some("gzip".into()),
            _ => r.content_type = Some("text/html".into()),
        }
        replies.push(r);
    }
    for reply in replies {
        let mut fake = Fake {
            calls: 0,
            request: None,
            reply: Some(reply),
        };
        failed(exchange_with(&mut fake, &mut key, account()));
        assert_eq!(fake.calls, 1);
    }
}
#[test]
fn response_reflection_including_json_escape_is_refused() {
    let mut fake = Fake::json(
        serde_json::to_vec(&json!({"balance":6,"email":SYNTHETIC}))
            .unwrap()
            .as_slice(),
    );
    failed(exchange_with(&mut fake, &mut key, account()));
    let escaped = format!(
        "{{\"balance\":6,\"email\":\"{}\"}}",
        SYNTHETIC
            .chars()
            .map(|c| format!("\\u{:04x}", c as u32))
            .collect::<String>()
    );
    let mut fake = Fake::json(escaped.as_bytes());
    failed(exchange_with(&mut fake, &mut key, account()));
}
#[test]
fn duplicate_security_headers_refused() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.append(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.append(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    assert!(unique_header(&headers, "content-type").is_err());
}

#[test]
fn wrong_describe_id_and_duplicate_rows_refused() {
    let mut fake = Fake::json(br#"{"instances":{"id":2,"label":null,"actual_status":"running"}}"#);
    failed(exchange_with(
        &mut fake,
        &mut key,
        request(Method::Get, "/api/v0/instances/1", None),
    ));
    let mut fake=Fake::json(br#"{"instances":[{"id":1,"label":null,"actual_status":"running"},{"id":1,"label":null,"actual_status":"running"}]}"#);
    failed(exchange_with(
        &mut fake,
        &mut key,
        request(Method::Get, "/api/v1/instances", None),
    ));
}

#[test]
fn tls_uses_explicit_aws_lc_and_no_secret_key_logging() {
    let config = tls_config().unwrap();
    let expected = rustls::crypto::aws_lc_rs::default_provider();
    assert_eq!(
        config.crypto_provider().cipher_suites,
        expected.cipher_suites
    );
    assert!(!config.key_log.will_log("CLIENT_RANDOM"));
    assert!(!config.key_log.will_log("CLIENT_TRAFFIC_SECRET_0"));
    assert!(!webpki_roots::TLS_SERVER_ROOTS.is_empty());
}

#[test]
fn numeric_secret_reflection_and_backend_failure_are_opaque() {
    let mut fake = Fake::json(br#"{"balance":123456}"#);
    failed(exchange_with(
        &mut fake,
        &mut || Ok(SecretValue::new(b"123456".to_vec())),
        account(),
    ));
    let mut fake = Fake {
        calls: 0,
        request: None,
        reply: None,
    };
    failed(exchange_with(&mut fake, &mut key, account()));
    assert_eq!(fake.calls, 1);
}
