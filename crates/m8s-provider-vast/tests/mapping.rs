use m8s_core::provider::{Error, LaunchSpec, Provider};
use m8s_core::{InstanceState, Observation, Offer};
use m8s_provider_vast::{FixtureConfig, FixtureTransport, Market, Method, Reply, Request, Vast};
use serde_json::{Value, json};
use std::collections::VecDeque;

struct Tape {
    replies: VecDeque<Result<Reply, Error>>,
    requests: Vec<Request>,
}
impl FixtureTransport for Tape {
    fn exchange(&mut self, request: Request) -> Result<Reply, Error> {
        self.requests.push(request);
        self.replies
            .pop_front()
            .expect("unexpected transport request")
    }
}
fn reply(body: &str, complete: bool) -> Result<Reply, Error> {
    Ok(Reply {
        status: 200,
        body: body.as_bytes().to_vec(),
        complete_instance_snapshot: complete,
    })
}
fn adapter(market: Market, replies: Vec<Result<Reply, Error>>) -> Vast<Tape> {
    Vast::fixture_only(
        market,
        "owner".into(),
        FixtureConfig {
            image: format!("fixture.invalid/m8s-node@sha256:{}", "a".repeat(64)),
            disk_gb: 64,
        },
        Tape {
            replies: replies.into(),
            requests: vec![],
        },
    )
    .ok()
    .unwrap()
}
fn spec() -> LaunchSpec {
    LaunchSpec {
        owner: "owner".into(),
        node_uid: "uid".into(),
        tier: "tier".into(),
    }
}
fn offer() -> Offer {
    Offer {
        id: "101".into(),
        country: Some("US".into()),
        region: None,
        inet_down_mbps: 1000,
        gpu_count: 1,
        vram_bytes_per_gpu: 24_000_000_000,
        hourly_micro_usd: 100_000,
        interruptible: false,
        reliability_bps: 9900,
    }
}
fn snapshot(id: u64, owner: &str, state: &str) -> String {
    json!({"instances":[{"id":id,"label":format!("m8s:v1:{owner}:uid:tier"),"actual_status":state}]}).to_string()
}
#[test]
fn community_and_secure_explicit_filters_and_precise_mapping() {
    let body = include_str!("fixtures/offers.json");
    for (market, expected, id) in [
        (Market::Community, false, "101"),
        (Market::Secure, true, "102"),
    ] {
        let mut a = adapter(market, vec![reply(body, false)]);
        let offers = a.search().unwrap();
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].id, id);
        if market == Market::Community {
            assert_eq!(offers[0].hourly_micro_usd, 123457);
            assert_eq!(offers[0].inet_down_mbps, 1000);
            assert_eq!(offers[0].vram_bytes_per_gpu, 24_576_000_000);
            assert_eq!(offers[0].gpu_count, 2);
            assert_eq!(offers[0].reliability_bps, 9912);
            assert_eq!(a.id(), "vast-community");
        } else {
            assert_eq!(a.id(), "vast-secure");
        }
        let tape = a.into_transport();
        let req = &tape.requests[0];
        assert_eq!(req.method, Method::Post);
        assert_eq!(req.path, "/api/v0/bundles/");
        let q = req.body.as_ref().unwrap();
        assert_eq!(q["datacenter"]["eq"], expected);
        assert_eq!(q["verified"]["eq"], true);
        assert_eq!(q["external"]["eq"], false);
        assert_eq!(q["type"]["eq"], "ondemand");
    }
}
#[test]
fn response_trust_filters_and_location_fail_closed() {
    let original: Value = serde_json::from_str(include_str!("fixtures/offers.json")).unwrap();
    for (key, value) in [
        ("geolocation", json!("CN")),
        ("geolocation", json!("UA")),
        ("geolocation", json!("City, US")),
        ("geolocation", Value::Null),
        ("verified", json!(false)),
        ("external", json!(true)),
        ("rentable", json!(false)),
        ("type", json!("bid")),
        ("inet_down", json!(999.999)),
    ] {
        let mut row = original["offers"][0].clone();
        row[key] = value;
        let body = json!({"offers":[row]}).to_string();
        assert!(
            adapter(Market::Community, vec![reply(&body, false)])
                .search()
                .unwrap()
                .is_empty(),
            "{key}"
        );
    }
}
#[test]
fn numeric_overflow_negative_and_missing_data_are_errors() {
    let original: Value = serde_json::from_str(include_str!("fixtures/offers.json")).unwrap();
    for (key, value) in [
        ("dph_total", json!(-1)),
        ("dph_total", json!("0.1")),
        ("gpu_ram", json!(-1)),
        ("num_gpus", json!(0)),
        ("num_gpus", json!(1.9)),
        ("reliability", json!(1.00001)),
        ("num_gpus", Value::Null),
    ] {
        let mut row = original["offers"][0].clone();
        row[key] = value;
        let body = json!({"offers":[row]}).to_string();
        assert!(
            adapter(Market::Community, vec![reply(&body, false)])
                .search()
                .is_err(),
            "{key}"
        );
    }
    for body in [
        "{}",
        "[]",
        r#"{"offers":[{"id":1}]}"#,
        r#"{"offers":[{"id":1e99}]}"#,
    ] {
        assert!(
            adapter(Market::Community, vec![reply(body, false)])
                .search()
                .is_err()
        );
    }
}
#[test]
fn launches_use_new_contract_and_no_env_templates_or_secrets() {
    let mut a = adapter(
        Market::Community,
        vec![reply(r#"{"success":true,"new_contract":501}"#, false)],
    );
    let instance = a.launch(&offer(), &spec()).unwrap();
    assert_eq!(instance.id, "501");
    assert_eq!(instance.state, InstanceState::Starting);
    assert_eq!(a.launch(&offer(), &spec()).unwrap(), instance);
    let tape = a.into_transport();
    assert_eq!(tape.requests.len(), 1);
    let r = &tape.requests[0];
    assert_eq!(r.path, "/api/v0/asks/101/");
    assert_eq!(r.method, Method::Put);
    let b = r.body.as_ref().unwrap();
    assert_eq!(b["label"], "m8s:v1:owner:uid:tier");
    assert_eq!(b["runtype"], "args");
    for forbidden in [
        "env",
        "extra_env",
        "onstart",
        "image_login",
        "template_hash_id",
        "price",
    ] {
        assert!(b.get(forbidden).is_none());
    }
}
#[test]
fn ambiguous_or_malformed_launch_is_never_automatically_reissued() {
    for result in [
        Err(Error::Unavailable),
        reply(r#"{"success":false,"new_contract":501}"#, false),
        reply(r#"{"success":true,"id":501}"#, false),
    ] {
        let mut a = adapter(Market::Community, vec![result]);
        assert!(a.launch(&offer(), &spec()).is_err());
        assert!(a.launch(&offer(), &spec()).is_err());
        assert_eq!(a.into_transport().requests.len(), 1);
    }
}
#[test]
fn list_owned_exact_owner_and_describe_unknown_labels_never_gone() {
    let body = include_str!("fixtures/instances.json");
    let mut a = adapter(
        Market::Community,
        vec![reply(body, true), reply(body, true), reply(body, true)],
    );
    let ours = a.list_owned("owner").unwrap();
    assert_eq!(ours.len(), 1);
    assert_eq!(ours[0].id, "501");
    assert!(a.describe("502").is_err());
    assert!(a.describe("503").is_err());
    assert_eq!(a.into_transport().requests.len(), 3);
}
#[test]
fn incomplete_http_or_error_envelope_is_never_absence() {
    for status in [401, 403, 404, 408, 429, 500, 503] {
        let mut a = adapter(
            Market::Community,
            vec![Ok(Reply {
                status,
                body: br#"{"instances":[]}"#.to_vec(),
                complete_instance_snapshot: true,
            })],
        );
        assert!(a.describe("501").is_err(), "status {status}");
    }
    for (body, complete) in [
        (r#"{"instances":[]}"#, false),
        (r#"{"success":false,"instances":[]}"#, true),
        (
            r#"{"error":"synthetic-sensitive-error","instances":[]}"#,
            true,
        ),
        (r#"{"instances":[],"has_more":true}"#, true),
        (r#"{"instances":[],"total":1}"#, true),
        (r#"{"instances":[],"next":"page2"}"#, true),
        (r#"{"instances":[{"id":1},{"id":1}]}"#, true),
        ("{}", true),
        ("not json", true),
    ] {
        let mut a = adapter(Market::Community, vec![reply(body, complete)]);
        let error = a.describe("501").expect_err("must not confirm absent");
        assert!(!error.to_string().contains("synthetic-sensitive"));
    }
}
#[test]
fn only_explicit_complete_successful_snapshot_can_confirm_absence() {
    let mut a = adapter(Market::Community, vec![reply(r#"{"instances":[]}"#, true)]);
    assert_eq!(a.describe("501").unwrap(), Observation::Absent);
}
#[test]
fn destroy_checks_ownership_and_acknowledgement_does_not_become_absence() {
    let stopped = snapshot(501, "owner", "stopped");
    let mut a = adapter(
        Market::Community,
        vec![
            reply(&stopped, true),
            reply(r#"{"success":true}"#, false),
            reply(&stopped, true),
        ],
    );
    a.destroy("501").unwrap();
    assert!(matches!(
        a.describe("501").unwrap(),
        Observation::Present(_)
    ));
    let t = a.into_transport();
    assert_eq!(t.requests[1].method, Method::Delete);
    assert_eq!(t.requests[1].path, "/api/v0/instances/501/");
    let foreign = snapshot(501, "other", "running");
    let mut a = adapter(Market::Community, vec![reply(&foreign, true)]);
    assert!(a.destroy("501").is_err());
    assert_eq!(a.into_transport().requests.len(), 1);
}
#[test]
fn loading_stopped_exited_and_unknown_states_stay_present_or_uncertain() {
    for (status, expected) in [
        ("loading", InstanceState::Starting),
        ("created", InstanceState::Starting),
        ("stopped", InstanceState::Stopped),
        ("exited", InstanceState::Exited),
    ] {
        let mut a = adapter(
            Market::Secure,
            vec![reply(&snapshot(501, "owner", status), true)],
        );
        assert!(matches!(a.describe("501").unwrap(),Observation::Present(i) if i.state==expected));
    }
    for status in ["destroyed", "offline", "new-unrecognized-status"] {
        let mut a = adapter(
            Market::Secure,
            vec![reply(&snapshot(501, "owner", status), true)],
        );
        assert!(a.describe("501").is_err());
    }
}
#[test]
fn invalid_paths_and_label_components_never_reach_transport() {
    let mut a = adapter(Market::Community, vec![]);
    for id in ["0", "../1", "01", "1/", "1?api_key=synthetic"] {
        assert!(a.describe(id).is_err());
    }
    let mut s = spec();
    s.owner = "owner-other".into();
    assert!(a.launch(&offer(), &s).is_err());
    s = spec();
    s.node_uid = "uid:injection".into();
    assert!(a.launch(&offer(), &s).is_err());
    assert!(a.list_owned("owner-other").is_err());
    assert!(a.into_transport().requests.is_empty());
}

#[test]
fn intent_uid_is_not_reused_for_another_tier_or_offer() {
    let mut a = adapter(
        Market::Community,
        vec![reply(r#"{"success":true,"new_contract":501}"#, false)],
    );
    a.launch(&offer(), &spec()).unwrap();
    let mut changed = spec();
    changed.tier = "changed-tier".into();
    assert!(a.launch(&offer(), &changed).is_err());
    let mut changed_offer = offer();
    changed_offer.id = "102".into();
    assert!(a.launch(&changed_offer, &spec()).is_err());
    assert_eq!(a.into_transport().requests.len(), 1);
}

#[test]
fn duplicate_json_keys_cannot_hide_present_instances_or_override_owner() {
    for body in [
        r#"{"instances":[{"id":501}],"instances":[]}"#,
        r#"{"instances":[{"id":501,"id":502,"label":"m8s:v1:owner:uid:tier","actual_status":"running"}]}"#,
        r#"{"instances":[{"id":501,"label":"m8s:v1:other:uid:tier","label":"m8s:v1:owner:uid:tier","actual_status":"running"}]}"#,
    ] {
        let mut a = adapter(Market::Community, vec![reply(body, true)]);
        assert!(a.describe("501").is_err());
    }
}
#[test]
fn unlabelled_foreign_instances_are_not_owned() {
    let mut a = adapter(
        Market::Community,
        vec![reply(
            r#"{"instances":[{"id":501,"label":null,"actual_status":"running"}]}"#,
            true,
        )],
    );
    assert!(a.list_owned("owner").unwrap().is_empty());
}
