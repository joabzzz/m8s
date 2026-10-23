use m8s_core::{InstanceState, Observation, Offer};
use m8s_provider::nebius::{Config, FixtureLaunch, Nebius, Transport, wire};
use m8s_provider::{Error, LaunchSpec, Provider, Result};
use serde_json::{Value, json};
use std::collections::VecDeque;

const INSTANCES: &str = include_str!("fixtures/nebius/instances.json");
const PLATFORMS: &str = include_str!("fixtures/nebius/platforms.json");
const QUOTAS: &str = include_str!("fixtures/nebius/quotas.json");
#[derive(Default)]
struct Fixture {
    pages: VecDeque<(&'static str, Result<wire::Page<wire::Vm>>)>,
    platforms: Option<wire::Page<wire::Platform>>,
    quotas: Option<wire::Page<wire::Quota>>,
    creates: usize,
    deletes: Vec<String>,
    accept_create: bool,
}
fn page<T: serde::de::DeserializeOwned>(text: &str) -> wire::Page<T> {
    serde_json::from_str(text).unwrap()
}
fn operation(id: &str) -> wire::Operation {
    serde_json::from_value(json!({"id":"operation-fixture","resourceId":id})).unwrap()
}
impl Transport for Fixture {
    fn instances(&mut self, project: &str, token: &str) -> Result<wire::Page<wire::Vm>> {
        assert_eq!(project, "project-fixture");
        let (expected, reply) = self
            .pages
            .pop_front()
            .expect("unexpected inventory request");
        assert_eq!(token, expected);
        reply
    }
    fn platforms(&mut self, project: &str, token: &str) -> Result<wire::Page<wire::Platform>> {
        assert_eq!(project, "project-fixture");
        assert_eq!(token, "");
        self.platforms.take().ok_or(Error::Unavailable)
    }
    fn quotas(&mut self, project: &str, token: &str) -> Result<wire::Page<wire::Quota>> {
        assert_eq!(project, "project-fixture");
        assert_eq!(token, "");
        self.quotas.take().ok_or(Error::Unavailable)
    }
    fn create_fixture(&mut self, request: FixtureLaunch) -> Result<wire::Operation> {
        self.creates += 1;
        assert_eq!(request.project, "project-fixture");
        assert_eq!(request.platform, "gpu-h100-sxm");
        assert_eq!(request.preset, "1gpu-16vcpu-200gb");
        assert_eq!(request.labels.len(), 4);
        assert_eq!(request.labels["m8s-owner"], "owner-fixture");
        assert_eq!(request.labels["m8s-node-uid"], "node-one");
        assert_eq!(request.labels["m8s-tier"], "interactive");
        assert_eq!(request.labels["m8s-version"], "1");
        if self.accept_create {
            Ok(operation("computeinstance-one"))
        } else {
            Err(Error::Unavailable)
        }
    }
    fn delete_fixture(&mut self, id: &str) -> Result<wire::Operation> {
        self.deletes.push(id.into());
        Ok(operation(id))
    }
}
fn config() -> Config {
    Config {
        project: "project-fixture".into(),
        region: "eu-north1".into(),
        owner: "owner-fixture".into(),
    }
}
fn spec() -> LaunchSpec {
    LaunchSpec {
        owner: "owner-fixture".into(),
        node_uid: "node-one".into(),
        tier: "interactive".into(),
    }
}
fn provider(pages: &[&str]) -> Nebius<Fixture> {
    let fixture = Fixture {
        pages: pages.iter().map(|text| ("", Ok(page(text)))).collect(),
        ..Default::default()
    };
    Nebius::fixture_only(config(), fixture).unwrap()
}
fn catalog() -> Fixture {
    Fixture {
        platforms: Some(page(PLATFORMS)),
        quotas: Some(page(QUOTAS)),
        ..Default::default()
    }
}
fn offer() -> Offer {
    Nebius::read_only(config(), catalog())
        .unwrap()
        .search()
        .unwrap()
        .remove(0)
}

#[test]
fn catalog_uses_current_usd_price_byte_capacities_region_and_quota() {
    let mut p = Nebius::read_only(config(), catalog()).unwrap();
    assert_eq!(p.id(), "nebius");
    let offers = p.search().unwrap();
    assert_eq!(offers.len(), 2);
    assert_eq!(offers[0].hourly_micro_usd, 4_500_000);
    assert_eq!(offers[1].hourly_micro_usd, 36_000_000);
    assert_eq!(offers[0].vram_bytes_per_gpu, 80_000_000_000);
    assert_eq!(offers[0].country.as_deref(), Some("FI"));
    assert_eq!(offers[0].region.as_deref(), Some("eu-north1"));
    assert_eq!(offers[0].inet_down_mbps, 0);
    assert_eq!(offers[0].reliability_bps, 0);
    assert!(!offers[0].interruptible);
    assert!(m8s_core::policy::check_location(&offers[0]).is_ok());
}
#[test]
fn exhausted_frozen_missing_and_unknown_quota_fail_closed() {
    for (limit, usage, state, usage_state, count) in [
        ("8", "1", "STATE_ACTIVE", "USAGE_STATE_USED", 1),
        ("0", "1", "STATE_ACTIVE", "USAGE_STATE_USED", 0),
        ("8", "0", "STATE_FROZEN", "USAGE_STATE_NOT_USED", 0),
        ("8", "0", "STATE_ACTIVE", "USAGE_STATE_UNKNOWN", 0),
    ] {
        let mut quotas: Value = serde_json::from_str(QUOTAS).unwrap();
        quotas["items"][0]["spec"]["limit"] = json!(limit);
        quotas["items"][0]["status"] =
            json!({"state":state,"usageState":usage_state,"usage":usage});
        let fixture = Fixture {
            quotas: Some(serde_json::from_value(quotas).unwrap()),
            ..catalog()
        };
        assert_eq!(
            Nebius::read_only(config(), fixture)
                .unwrap()
                .search()
                .unwrap()
                .len(),
            count
        );
    }
    let fixture = Fixture {
        quotas: Some(page("{}")),
        ..catalog()
    };
    assert!(
        Nebius::read_only(config(), fixture)
            .unwrap()
            .search()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn owner_snapshot_is_complete_across_pages_and_exactly_scoped() {
    let value: Value = serde_json::from_str(INSTANCES).unwrap();
    let first = json!({"items":[value["items"][0]],"nextPageToken":"next"});
    let last = json!({"items":[value["items"][1],value["items"][2]]});
    let fixture = Fixture {
        pages: VecDeque::from([
            ("", Ok(serde_json::from_value(first).unwrap())),
            ("next", Ok(serde_json::from_value(last).unwrap())),
        ]),
        ..Default::default()
    };
    let mut p = Nebius::read_only(config(), fixture).unwrap();
    assert!(p.list_owned("owner-other").is_err());
    let owned = p.list_owned("owner-fixture").unwrap();
    assert_eq!(owned.len(), 2);
    assert_eq!(owned[1].state, InstanceState::Stopped);
}
#[test]
fn partial_failed_repeating_or_duplicate_inventory_never_confirms_absence() {
    let fixture = Fixture {
        pages: VecDeque::from([
            ("", Ok(page(r#"{"nextPageToken":"next"}"#))),
            ("next", Err(Error::Unavailable)),
        ]),
        ..Default::default()
    };
    assert!(
        Nebius::read_only(config(), fixture)
            .unwrap()
            .describe("computeinstance-missing")
            .is_err()
    );
    let fixture = Fixture {
        pages: VecDeque::from([
            ("", Ok(page(r#"{"nextPageToken":"next"}"#))),
            ("next", Ok(page(r#"{"nextPageToken":"next"}"#))),
        ]),
        ..Default::default()
    };
    assert!(
        Nebius::read_only(config(), fixture)
            .unwrap()
            .describe("computeinstance-missing")
            .is_err()
    );
    let mut value: Value = serde_json::from_str(INSTANCES).unwrap();
    value["items"][1] = value["items"][0].clone();
    assert!(
        provider(&[&value.to_string()])
            .list_owned("owner-fixture")
            .is_err()
    );
}
#[test]
fn malformed_owned_labels_wrong_project_and_unknown_states_fail_closed() {
    for (pointer, value) in [
        ("/items/0/metadata/labels/m8s-tier", json!("")),
        ("/items/0/metadata/labels/m8s-version", json!("2")),
        ("/items/0/metadata/parentId", json!("project-other")),
        ("/items/0/status/state", json!("DELETING")),
        ("/items/0/status/state", json!("ERROR")),
    ] {
        let mut rows: Value = serde_json::from_str(INSTANCES).unwrap();
        *rows.pointer_mut(pointer).unwrap() = value;
        assert!(
            provider(&[&rows.to_string()])
                .list_owned("owner-fixture")
                .is_err()
        );
    }
}
#[test]
fn uncertain_launch_is_adoptable_idempotent_and_never_retried() {
    let mut p = provider(&["{}", "{}", INSTANCES, INSTANCES]);
    assert!(p.launch(&offer(), &spec()).is_err());
    assert!(p.launch(&offer(), &spec()).is_err());
    let adopted = p.launch(&offer(), &spec()).unwrap();
    assert_eq!(adopted.id, "computeinstance-one");
    assert_eq!(p.launch(&offer(), &spec()).unwrap(), adopted);
    assert_eq!(p.into_transport().creates, 1);
}
#[test]
fn adoption_after_restart_does_not_create_and_rejects_collisions() {
    let mut p = provider(&[INSTANCES]);
    assert_eq!(
        p.launch(&offer(), &spec()).unwrap().id,
        "computeinstance-one"
    );
    assert_eq!(p.into_transport().creates, 0);
    for pointer in [
        "/items/0/metadata/labels/m8s-tier",
        "/items/0/spec/resources/preset",
    ] {
        let mut value: Value = serde_json::from_str(INSTANCES).unwrap();
        *value.pointer_mut(pointer).unwrap() = json!("different");
        assert!(
            provider(&[&value.to_string()])
                .launch(&offer(), &spec())
                .is_err()
        );
    }
    let mut value: Value = serde_json::from_str(INSTANCES).unwrap();
    let mut duplicate = value["items"][0].clone();
    duplicate["metadata"]["id"] = json!("computeinstance-duplicate");
    value["items"].as_array_mut().unwrap().push(duplicate);
    assert!(
        provider(&[&value.to_string()])
            .launch(&offer(), &spec())
            .is_err()
    );
}
#[test]
fn acceptance_without_observed_vm_is_uncertain_and_changed_intent_is_invalid() {
    let mut fixture = provider(&["{}", "{}", "{}"]).into_transport();
    fixture.accept_create = true;
    let mut p = Nebius::fixture_only(config(), fixture).unwrap();
    assert!(p.launch(&offer(), &spec()).is_err());
    let mut changed = spec();
    changed.tier = "different".into();
    assert!(matches!(p.launch(&offer(), &changed), Err(Error::Invalid)));
    assert!(p.launch(&offer(), &spec()).is_err());
    assert_eq!(p.into_transport().creates, 1);
}
#[test]
fn destroy_acknowledgement_is_not_absence_and_foreign_ids_are_refused() {
    let mut p = provider(&[INSTANCES, INSTANCES, "{}", INSTANCES]);
    p.destroy("computeinstance-one").unwrap();
    assert!(matches!(
        p.describe("computeinstance-one").unwrap(),
        Observation::Present(_)
    ));
    assert_eq!(
        p.describe("computeinstance-one").unwrap(),
        Observation::Absent
    );
    assert!(p.destroy("computeinstance-other").is_err());
    assert_eq!(p.into_transport().deletes, ["computeinstance-one"]);
}
#[test]
fn read_only_provider_refuses_all_mutations_before_transport() {
    let mut p = Nebius::read_only(config(), Fixture::default()).unwrap();
    assert!(p.launch(&offer(), &spec()).is_err());
    assert!(p.destroy("computeinstance-one").is_err());
    let fixture = p.into_transport();
    assert_eq!(fixture.creates, 0);
    assert!(fixture.deletes.is_empty());
}
#[test]
fn wire_rejects_duplicate_labels_and_known_fields() {
    for text in [
        INSTANCES.replace(
            "\"m8s-owner\": \"owner-fixture\"",
            "\"m8s-owner\": \"owner-other\", \"m8s-owner\": \"owner-fixture\"",
        ),
        INSTANCES.replace(
            "\"state\": \"RUNNING\"",
            "\"state\": \"STOPPED\", \"state\": \"RUNNING\"",
        ),
    ] {
        assert!(serde_json::from_str::<wire::Page<wire::Vm>>(&text).is_err());
    }
}
#[test]
fn invalid_location_or_offer_never_reaches_effects() {
    let mut invalid = config();
    invalid.region = "unknown".into();
    assert!(Nebius::fixture_only(invalid, Fixture::default()).is_err());
    let mut p = provider(&[]);
    let mut invalid = offer();
    invalid.hourly_micro_usd = 1;
    assert!(p.launch(&invalid, &spec()).is_err());
    let mut foreign = spec();
    foreign.owner = "owner-other".into();
    assert!(p.launch(&offer(), &foreign).is_err());
}

#[test]
fn adopted_intent_is_consumed_even_after_vm_disappears() {
    let mut p = provider(&[INSTANCES, "{}"]);
    assert!(p.launch(&offer(), &spec()).is_ok());
    assert!(p.launch(&offer(), &spec()).is_err());
    assert_eq!(p.into_transport().creates, 0);
}

#[test]
fn omitted_protojson_zero_usage_is_a_valid_known_zero() {
    let mut quotas: Value = serde_json::from_str(QUOTAS).unwrap();
    quotas["items"][0]["status"]
        .as_object_mut()
        .unwrap()
        .remove("usage");
    quotas["items"][0]["status"]["usageState"] = json!("USAGE_STATE_NOT_USED");
    let fixture = Fixture {
        quotas: Some(serde_json::from_value(quotas).unwrap()),
        ..catalog()
    };
    assert_eq!(
        Nebius::read_only(config(), fixture)
            .unwrap()
            .search()
            .unwrap()
            .len(),
        2
    );
}
