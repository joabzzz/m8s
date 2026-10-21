use m8s_core::{Instance, InstanceState, Observation, Phase};
use m8s_provider::{Fault, Mock, Provider};
use m8sd::Store;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn path() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "m8s-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&p).unwrap();
    p.join("state.db")
}
#[test]
fn restart_persistence_and_owner_bound_database() {
    let path = path();
    let mut p = Mock::default();
    {
        let mut s = Store::open(&path, "owner").unwrap();
        s.scale("qwen27", 1).unwrap();
        s.reconcile(&mut p, 0).unwrap();
    }
    let mut s = Store::open(&path, "owner").unwrap();
    assert_eq!(s.tiers().unwrap(), vec![("qwen27".into(), 1)]);
    s.reconcile(&mut p, 1).unwrap();
    assert_eq!(s.nodes().unwrap().len(), 1);
    assert_eq!(s.nodes().unwrap()[0].phase, Phase::Booting);
    assert!(Store::open(&path, "another-owner").is_err());
}
#[test]
fn only_explicit_absence_releases_finalizer() {
    let mut s = Store::open(path(), "owner").unwrap();
    let mut p = Mock::default();
    s.scale("tier", 1).unwrap();
    s.reconcile(&mut p, 0).unwrap();
    s.down().unwrap();
    p.faults.push(Fault::Describe);
    s.reconcile(&mut p, 1).unwrap();
    assert_eq!(s.nodes().unwrap().len(), 1);
    p.defer_destroy = true;
    s.reconcile(&mut p, 2).unwrap();
    assert_eq!(s.nodes().unwrap()[0].phase, Phase::Destroying);
    s.reconcile(&mut p, 3).unwrap();
    assert_eq!(s.nodes().unwrap().len(), 1);
    assert_eq!(p.destroy_calls.len(), 2);
    p.defer_destroy = false;
    s.reconcile(&mut p, 4).unwrap();
    assert_eq!(s.nodes().unwrap().len(), 1);
    s.reconcile(&mut p, 5).unwrap();
    assert!(s.nodes().unwrap().is_empty());
}
#[test]
fn stopped_orphans_retry_and_other_owners_are_untouched() {
    let mut s = Store::open(path(), "owner").unwrap();
    let mut p = Mock::default();
    for (id, owner) in [("ours", "owner"), ("theirs", "other")] {
        p.insert(Instance {
            id: id.into(),
            owner: owner.into(),
            tier: "tier".into(),
            node_uid: id.into(),
            state: InstanceState::Stopped,
        })
        .unwrap();
    }
    p.faults.push(Fault::Destroy);
    s.reconcile(&mut p, 0).unwrap();
    assert_eq!(s.nodes().unwrap().len(), 1);
    s.reconcile(&mut p, 1).unwrap();
    assert_eq!(p.destroy_calls, vec!["ours", "ours"]);
    s.reconcile(&mut p, 2).unwrap();
    assert!(s.nodes().unwrap().is_empty());
    assert!(matches!(
        p.describe("theirs").unwrap(),
        Observation::Present(_)
    ));
}
#[test]
fn launch_rate_limit_survives_row_deletion_and_restart() {
    let path = path();
    let mut p = Mock::default();
    for now in 0..3 {
        let mut s = Store::open(&path, "owner").unwrap();
        s.scale("tier", 1).unwrap();
        s.reconcile(&mut p, now).unwrap();
        let id = s.nodes().unwrap()[0].instance_id.clone().unwrap();
        p.remove(&id).unwrap();
    }
    let mut s = Store::open(&path, "owner").unwrap();
    s.reconcile(&mut p, 3).unwrap();
    assert!(s.nodes().unwrap().is_empty());
    assert_eq!(s.launch_count("tier", 3).unwrap(), 3);
    assert!(s.logs().unwrap().iter().any(|e| e.starts_with("Paused")));
    s.reconcile(&mut p, 3600).unwrap();
    assert_eq!(s.nodes().unwrap().len(), 1);
}
#[test]
fn ambiguous_launch_adopted_after_restart_not_duplicated() {
    let path = path();
    let mut p = Mock::default();
    p.faults.push(Fault::LaunchAfter);
    {
        let mut s = Store::open(&path, "owner").unwrap();
        s.scale("tier", 1).unwrap();
        s.reconcile(&mut p, 0).unwrap();
        assert!(s.nodes().unwrap()[0].instance_id.is_none());
    }
    let mut s = Store::open(&path, "owner").unwrap();
    s.reconcile(&mut p, 1).unwrap();
    assert!(s.nodes().unwrap()[0].instance_id.is_some());
    assert_eq!(s.launch_count("tier", 1).unwrap(), 1);
    assert_eq!(p.list_owned("owner").unwrap().len(), 1);
}
#[test]
fn list_error_is_not_empty_and_unknown_launch_is_never_retried() {
    let mut s = Store::open(path(), "owner").unwrap();
    let mut p = Mock::default();
    s.scale("tier", 1).unwrap();
    p.faults.push(Fault::LaunchBefore);
    s.reconcile(&mut p, 0).unwrap();
    p.faults.push(Fault::List);
    assert!(s.reconcile(&mut p, 1).is_err());
    for now in 2..10 {
        s.reconcile(&mut p, now).unwrap();
    }
    assert_eq!(s.launch_count("tier", 9).unwrap(), 1);
    assert!(p.list_owned("owner").unwrap().is_empty());
    s.down().unwrap();
    s.reconcile(&mut p, 10).unwrap();
    assert_eq!(s.nodes().unwrap().len(), 1);
}
#[test]
fn mock_provider_state_itself_survives_restart() {
    let path = path().with_extension("json");
    let id = {
        let mut p = Mock::persistent(path.clone()).unwrap();
        let offer = p.search().unwrap().remove(0);
        p.launch(
            &offer,
            &m8s_provider::LaunchSpec {
                owner: "o".into(),
                tier: "t".into(),
                node_uid: "u".into(),
            },
        )
        .unwrap()
        .id
    };
    let mut p = Mock::persistent(path).unwrap();
    assert!(matches!(p.describe(&id).unwrap(), Observation::Present(_)));
}
#[test]
fn nonce_binding_ttl_single_use_and_restart() {
    use m8sd::nonce::Binding;
    let path = path();
    let mut p = Mock::default();
    let mut s = Store::open(&path, "owner").unwrap();
    s.scale("tier", 1).unwrap();
    s.reconcile(&mut p, 0).unwrap();
    let id = s.nodes().unwrap()[0].instance_id.clone().unwrap();
    let nonce = [42u8; 32];
    let digest = format!("sha256:{}", "a".repeat(64));
    let binding = |ip| Binding {
        instance_id: &id,
        source_ip: ip,
        node_endpoint: "verified-endpoint",
        image_digest: &digest,
        self_destruct_present: true,
    };
    s.issue_nonce(&nonce, &id, "203.0.113.1", &digest, 0)
        .unwrap();
    drop(s);
    let s = Store::open(&path, "owner").unwrap();
    assert!(!s.redeem_nonce(&nonce, binding("203.0.113.2"), 1).unwrap());
    assert!(
        !s.redeem_nonce(
            &nonce,
            Binding {
                self_destruct_present: false,
                ..binding("203.0.113.1")
            },
            1
        )
        .unwrap()
    );
    assert!(!s.redeem_nonce(&nonce, binding("203.0.113.1"), -1).unwrap());
    assert!(s.redeem_nonce(&nonce, binding("203.0.113.1"), 899).unwrap());
    assert!(!s.redeem_nonce(&nonce, binding("203.0.113.1"), 899).unwrap());
    let expired = [43u8; 32];
    s.issue_nonce(&expired, &id, "203.0.113.1", &digest, 0)
        .unwrap();
    assert!(
        !s.redeem_nonce(&expired, binding("203.0.113.1"), 900)
            .unwrap()
    );
}

#[test]
fn clock_rollback_does_not_open_new_launch_slots() {
    let mut s = Store::open(path(), "owner").unwrap();
    let mut p = Mock::default();
    s.scale("tier", 1).unwrap();
    s.reconcile(&mut p, 10000).unwrap();
    let id = s.nodes().unwrap()[0].instance_id.clone().unwrap();
    p.remove(&id).unwrap();
    s.reconcile(&mut p, 0).unwrap();
    assert!(s.nodes().unwrap().is_empty());
    assert_eq!(s.launch_count("tier", 0).unwrap(), 1);
    s.reconcile(&mut p, 10000).unwrap();
    assert_eq!(s.nodes().unwrap().len(), 1);
}
#[test]
fn nonce_mismatches_and_concurrent_replay_are_refused() {
    use m8sd::nonce::Binding;
    let path = path();
    let mut p = Mock::default();
    let mut s = Store::open(&path, "owner").unwrap();
    s.scale("tier", 1).unwrap();
    s.reconcile(&mut p, 0).unwrap();
    let id = s.nodes().unwrap()[0].instance_id.clone().unwrap();
    let digest = format!("sha256:{}", "b".repeat(64));
    let nonce = [44u8; 32];
    s.issue_nonce(&nonce, &id, "203.0.113.1", &digest, 0)
        .unwrap();
    let binding = || Binding {
        instance_id: &id,
        source_ip: "203.0.113.1",
        node_endpoint: "verified-endpoint",
        image_digest: &digest,
        self_destruct_present: true,
    };
    assert!(
        !s.redeem_nonce(
            &nonce,
            Binding {
                instance_id: "wrong",
                ..binding()
            },
            1
        )
        .unwrap()
    );
    assert!(
        !s.redeem_nonce(
            &nonce,
            Binding {
                image_digest: "wrong",
                ..binding()
            },
            1
        )
        .unwrap()
    );
    assert!(!s.redeem_nonce(&[45u8; 32], binding(), 1).unwrap());
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let path = path.clone();
            let id = id.clone();
            let digest = digest.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let s = Store::open(path, "owner").unwrap();
                barrier.wait();
                s.redeem_nonce(
                    &nonce,
                    Binding {
                        instance_id: &id,
                        source_ip: "203.0.113.1",
                        node_endpoint: "verified-endpoint",
                        image_digest: &digest,
                        self_destruct_present: true,
                    },
                    1,
                )
                .unwrap()
            })
        })
        .collect();
    let successes = threads
        .into_iter()
        .map(|t| usize::from(t.join().unwrap()))
        .sum::<usize>();
    assert_eq!(successes, 1);
}

#[test]
fn starting_instances_retain_desired_capacity_but_down_still_destroys() {
    let mut s = Store::open(path(), "owner").unwrap();
    let mut p = Mock::default();
    s.scale("tier", 1).unwrap();
    s.reconcile(&mut p, 0).unwrap();
    let node = s.nodes().unwrap().remove(0);
    p.insert(Instance {
        id: node.instance_id.unwrap(),
        owner: "owner".into(),
        node_uid: node.uid,
        tier: "tier".into(),
        state: InstanceState::Starting,
    })
    .unwrap();
    s.reconcile(&mut p, 1).unwrap();
    assert!(p.destroy_calls.is_empty());
    assert_eq!(s.nodes().unwrap().len(), 1);
    assert_eq!(s.launch_count("tier", 1).unwrap(), 1);
    s.down().unwrap();
    s.reconcile(&mut p, 2).unwrap();
    assert_eq!(p.destroy_calls.len(), 1);
    s.reconcile(&mut p, 3).unwrap();
    assert!(s.nodes().unwrap().is_empty());
}
