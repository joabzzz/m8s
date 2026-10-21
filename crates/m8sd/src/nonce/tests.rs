//! Actual SQLite policy tests with synthetic trusted provider/image observations.
//! No production authenticated-session constructor is forged; TLS/MP signing and
//! provider effects are NOT qualified by these SQL tests.
use super::*;
use m8s_provider::Mock;
use std::{
    path::PathBuf,
    sync::{
        Arc, Barrier,
        atomic::{AtomicU64, Ordering},
    },
};
static NEXT: AtomicU64 = AtomicU64::new(0);
const NONCE: [u8; 32] = [42; 32];
fn path() -> PathBuf {
    use std::os::unix::fs::DirBuilderExt;
    let path = std::env::temp_dir().join(format!(
        "m8sd-bootstrap-policy-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .unwrap();
    path.join("state.db")
}
fn fixture() -> (PathBuf, Store, Mock, String, LaunchBinding) {
    let path = path();
    let mut store = Store::open(&path, "owner").unwrap();
    let mut provider = Mock::default();
    store.scale("tier", 1).unwrap();
    store.reconcile(&mut provider, 0).unwrap();
    let standalone:bool=store.conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='bootstrap_nonce')",[],|r|r.get(0)).unwrap();
    assert!(!standalone);
    let node = store.nodes().unwrap().remove(0);
    let binding = LaunchBinding {
        owner: EndpointId([1; 32]),
        instance_id: node.instance_id.unwrap(),
        container_id: "synthetic-container".into(),
        provider_ip: "203.0.113.1".parse().unwrap(),
        image_sha256: [3; 32],
    };
    (path, store, provider, node.uid, binding)
}
fn issue(s: &Store, uid: &str, binding: &LaunchBinding) {
    s.issue_bootstrap_nonce(
        &NONCE,
        LaunchIdentity {
            observed_at: 100,
            status: ObservationStatus::Complete,
            owner: "owner",
            node_uid: uid,
            binding,
        },
        100,
    )
    .unwrap();
}
fn consume(s: &Store, nonce: &[u8; 32], binding: &LaunchBinding, now: i64) -> Result<()> {
    let hash: [u8; 32] = Sha256::digest(nonce).into();
    s.consume_record(&ConsumeRecord {
        expected_uid: None,
        hash: &hash,
        binding,
        node: EndpointId([2; 32]),
        now,
        cert_expires_at: 2000,
    })
}
fn certify(s: &Store, binding: &LaunchBinding, now: i64) -> Result<()> {
    let hash: [u8; 32] = Sha256::digest(NONCE).into();
    s.certify_record(
        &ConsumeRecord {
            expected_uid: None,
            hash: &hash,
            binding,
            node: EndpointId([2; 32]),
            now,
            cert_expires_at: 2000,
        },
        &NodeCertificate(b"synthetic-sql-fixture-not-a-cert".to_vec()),
    )
}
#[test]
fn exact_bindings_expiry_single_use_restart_and_one_nonce_per_launch() {
    let (path, s, _, uid, b) = fixture();
    issue(&s, &uid, &b);
    for now in [99, 1000, -1] {
        assert!(consume(&s, &NONCE, &b, now).is_err());
    }
    for field in 0..5 {
        let mut wrong = b.clone();
        match field {
            0 => wrong.owner = EndpointId([9; 32]),
            1 => wrong.instance_id.push('x'),
            2 => wrong.container_id.push('x'),
            3 => wrong.provider_ip = "203.0.113.2".parse().unwrap(),
            _ => wrong.image_sha256[0] ^= 1,
        };
        assert!(consume(&s, &NONCE, &wrong, 101).is_err());
    }
    assert!(consume(&s, &[43; 32], &b, 101).is_err());
    consume(&s, &NONCE, &b, 999).unwrap();
    assert!(s.conn.is_autocommit());
    drop(s);
    let s = Store::open(path, "owner").unwrap();
    assert!(consume(&s, &NONCE, &b, 999).is_err());
    let intent = s.bootstrap_attempts().unwrap().remove(0);
    assert!(intent.cleanup_required);
    assert_eq!(intent.stage, BootstrapStage::Consumed);
    assert!(
        s.issue_bootstrap_nonce(
            &[43; 32],
            LaunchIdentity {
                observed_at: 100,
                status: ObservationStatus::Complete,
                owner: "owner",
                node_uid: &uid,
                binding: &b
            },
            1000
        )
        .is_err()
    );
}
#[test]
fn wrong_owner_uid_and_rebound_instance_are_refused() {
    let (path, s, _, uid, b) = fixture();
    assert!(
        s.issue_bootstrap_nonce(
            &NONCE,
            LaunchIdentity {
                observed_at: 100,
                status: ObservationStatus::Complete,
                owner: "other",
                node_uid: &uid,
                binding: &b
            },
            100
        )
        .is_err()
    );
    assert!(
        s.issue_bootstrap_nonce(
            &NONCE,
            LaunchIdentity {
                observed_at: 100,
                status: ObservationStatus::Complete,
                owner: "owner",
                node_uid: "wrong-uid",
                binding: &b
            },
            100
        )
        .is_err()
    );
    assert!(Store::open(&path, "other").is_err());
    issue(&s, &uid, &b);
    let rogue = Store {
        origin: std::sync::Arc::new(crate::StoreOrigin),
        conn: Connection::open(&path).unwrap(),
        owner: "other".into(),
    };
    assert!(consume(&rogue, &NONCE, &b, 101).is_err());
    drop(rogue);
    s.conn
        .execute(
            "UPDATE gpu_node SET instance_id='different-instance' WHERE uid=?",
            [&uid],
        )
        .unwrap();
    assert!(consume(&s, &NONCE, &b, 101).is_err());
}
#[test]
fn two_connections_race_only_one_durable_consume() {
    let (path, s, _, uid, b) = fixture();
    issue(&s, &uid, &b);
    drop(s);
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let path = path.clone();
            let b = b.clone();
            let gate = barrier.clone();
            std::thread::spawn(move || {
                let s = Store::open(path, "owner").unwrap();
                gate.wait();
                consume(&s, &NONCE, &b, 101).is_ok()
            })
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .map(|t| usize::from(t.join().unwrap()))
            .sum::<usize>(),
        1
    );
    let s = Store::open(path, "owner").unwrap();
    assert!(s.bootstrap_attempts().unwrap()[0].cleanup_required);
}
#[test]
fn down_and_scale_zero_cannot_be_reversed_to_redeem_old_nonce() {
    for global in [false, true] {
        let (_, mut s, mut p, uid, b) = fixture();
        issue(&s, &uid, &b);
        if global {
            s.down().unwrap();
        } else {
            s.scale("tier", 0).unwrap();
        }
        s.scale("tier", 1).unwrap();
        assert!(consume(&s, &NONCE, &b, 101).is_err());
        let intent = s.bootstrap_attempts().unwrap().remove(0);
        assert!(intent.cancelled);
        assert_eq!(intent.stage, BootstrapStage::Cancelled);
        assert!(
            s.issue_bootstrap_nonce(
                &[43; 32],
                LaunchIdentity {
                    observed_at: 100,
                    status: ObservationStatus::Complete,
                    owner: "owner",
                    node_uid: &uid,
                    binding: &b
                },
                102
            )
            .is_err()
        );
        assert_eq!(s.nodes().unwrap().len(), 1);
        p.defer_destroy = true;
        s.reconcile(&mut p, 103).unwrap();
        assert_eq!(s.nodes().unwrap()[0].phase, m8s_core::Phase::Destroying);
    }
}
#[test]
fn down_during_signing_is_durable_and_does_not_hold_transaction() {
    let (path, s, _, uid, b) = fixture();
    issue(&s, &uid, &b);
    consume(&s, &NONCE, &b, 101).unwrap();
    // External signer callback window: another connection can commit down.
    let controller = Store::open(&path, "owner").unwrap();
    controller.down().unwrap();
    assert!(certify(&s, &b, 101).is_err());
    assert!(consume(&s, &NONCE, &b, 102).is_err());
    drop(controller);
    drop(s);
    let s = Store::open(path, "owner").unwrap();
    let intent = s.bootstrap_attempts().unwrap().remove(0);
    assert!(intent.cancelled && intent.cleanup_required);
    assert_eq!(intent.node_endpoint, Some(EndpointId([2; 32])));
    assert_eq!(s.nodes().unwrap().len(), 1);
}
#[test]
fn down_consume_race_and_late_delivery_remain_cleanup_safe() {
    let (path, s, _, uid, b) = fixture();
    issue(&s, &uid, &b);
    drop(s);
    let gate = Arc::new(Barrier::new(2));
    let path2 = path.clone();
    let g = gate.clone();
    let b2 = b.clone();
    let worker = std::thread::spawn(move || {
        let s = Store::open(path2, "owner").unwrap();
        g.wait();
        consume(&s, &NONCE, &b2, 101).is_ok()
    });
    let s = Store::open(&path, "owner").unwrap();
    gate.wait();
    s.down().unwrap();
    let burned = worker.join().unwrap();
    let intent = s.bootstrap_attempts().unwrap().remove(0);
    assert!(intent.cancelled);
    assert_eq!(intent.cleanup_required, burned);
    assert!(certify(&s, &b, 101).is_err());
    assert!(!s.confirm_bootstrap_delivery(&intent, 102).unwrap_or(false));
    assert_eq!(s.nodes().unwrap().len(), 1);
}
#[test]
fn simulated_sign_failure_and_unwind_preserve_burn_and_cleanup_on_restart() {
    for unwind in [false, true] {
        let (path, s, _, uid, b) = fixture();
        issue(&s, &uid, &b);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            consume(&s, &NONCE, &b, 101).unwrap();
            if unwind {
                panic!("synthetic interrupted signer");
            }
            Err::<NodeCertificate, _>(m8s_bootstrap::Error::Certificate)
        }));
        if unwind {
            assert!(result.is_err());
        } else {
            assert!(result.unwrap().is_err());
        }
        drop(s);
        let s = Store::open(path, "owner").unwrap();
        assert!(consume(&s, &NONCE, &b, 102).is_err());
        let attempt = s.bootstrap_attempts().unwrap().remove(0);
        assert!(attempt.cleanup_required);
        assert_eq!(attempt.stage, BootstrapStage::Consumed);
    }
}
#[test]
fn delivery_confirmation_then_down_requests_revocation_without_ready_or_row_delete() {
    let (_, mut s, mut p, uid, b) = fixture();
    issue(&s, &uid, &b);
    consume(&s, &NONCE, &b, 101).unwrap();
    certify(&s, &b, 101).unwrap();
    let attempt = s.bootstrap_attempts().unwrap().remove(0);
    assert!(attempt.cleanup_required);
    assert!(s.confirm_bootstrap_delivery(&attempt, 102).unwrap());
    assert!(!s.bootstrap_attempts().unwrap()[0].cleanup_required);
    assert_eq!(s.nodes().unwrap()[0].phase, m8s_core::Phase::Booting);
    s.down().unwrap();
    assert!(s.bootstrap_attempts().unwrap()[0].cleanup_required);
    p.defer_destroy = true;
    s.reconcile(&mut p, 103).unwrap();
    assert_eq!(s.nodes().unwrap().len(), 1);
    assert!(!s.confirm_bootstrap_delivery(&attempt, 104).unwrap());
    p.defer_destroy = false;
    s.reconcile(&mut p, 104).unwrap();
    assert_eq!(s.nodes().unwrap().len(), 1);
    s.reconcile(&mut p, 105).unwrap();
    assert!(s.nodes().unwrap().is_empty());
    let tombstone = s.bootstrap_attempts().unwrap().remove(0);
    assert!(tombstone.cleanup_required && tombstone.cancelled);
}
#[test]
fn old_unbound_database_rows_migrate_to_refusal() {
    let (path, s, _, uid, b) = fixture();
    s.conn.execute_batch("DROP TABLE nonce;CREATE TABLE nonce(hash BLOB PRIMARY KEY,instance_id TEXT NOT NULL,ip TEXT NOT NULL,digest TEXT NOT NULL,issued_at INTEGER NOT NULL,redeemed_at INTEGER,node_endpoint TEXT);").unwrap();
    s.conn
        .execute(
            "INSERT INTO nonce(hash,instance_id,ip,digest,issued_at,redeemed_at,node_endpoint) VALUES (?,?,?,?,100,101,'unverified-legacy-endpoint')",
            params![
                Sha256::digest(NONCE).as_slice(),
                b.instance_id,
                b.provider_ip.to_string(),
                digest_text(&b.image_sha256)
            ],
        )
        .unwrap();
    drop(s);
    let s = Store::open(&path, "owner").unwrap();
    assert!(consume(&s, &NONCE, &b, 101).is_err());
    let attempts = s.bootstrap_attempts().unwrap();
    assert_eq!(attempts[0].stage, BootstrapStage::LegacyQuarantined);
    assert!(attempts[0].node_endpoint.is_none());
    assert_eq!(
        attempts[0].legacy_endpoint.as_deref(),
        Some("unverified-legacy-endpoint")
    );
    assert!(attempts[0].cleanup_required && attempts[0].cancelled);
    assert_eq!(attempts[0].redeemed_at, Some(101));
    assert!(
        s.issue_bootstrap_nonce(
            &[43; 32],
            LaunchIdentity {
                observed_at: 100,
                status: ObservationStatus::Complete,
                owner: "owner",
                node_uid: &uid,
                binding: &b
            },
            102
        )
        .is_err()
    );
    assert_eq!(s.nodes().unwrap().len(), 1);
    // Synthetic persisted replacement intent reuses a provider ID; old unbound
    // tombstones must not quarantine this different UID on every future open.
    s.conn
        .execute(
            "UPDATE gpu_node SET instance_id='retired-provider-id' WHERE uid=?",
            [&uid],
        )
        .unwrap();
    s.conn.execute("INSERT INTO gpu_node(uid,tier,instance_id,phase,created_at) VALUES ('replacement-uid','tier',?,'\"Booting\"',200)",[&b.instance_id]).unwrap();
    drop(s);
    let s = Store::open(&path, "owner").unwrap();
    let mut replacement = b.clone();
    replacement.container_id = "replacement-container".into();
    s.issue_bootstrap_nonce(
        &[43; 32],
        LaunchIdentity {
            observed_at: 200,
            status: ObservationStatus::Complete,
            owner: "owner",
            node_uid: "replacement-uid",
            binding: &replacement,
        },
        200,
    )
    .unwrap();
    consume(&s, &[43; 32], &replacement, 201).unwrap();
    let attempts = s.bootstrap_attempts().unwrap();
    assert_eq!(attempts.len(), 2);
    assert!(attempts.iter().any(|a| a.stage == BootstrapStage::Consumed));
    assert!(
        attempts
            .iter()
            .any(|a| a.stage == BootstrapStage::LegacyQuarantined)
    );
    assert!(consume(&s, &NONCE, &replacement, 201).is_err());
}
#[test]
fn private_directory_db_and_sidecar_symlinks_are_refused() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let path = path();
    let parent = path.parent().unwrap();
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        Store::open(&path, "owner"),
        Err(Error::PrivateState)
    ));
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    let s = Store::open(&path, "owner").unwrap();
    for p in [
        path.clone(),
        parent.join("state.db-wal"),
        parent.join("state.db-shm"),
    ] {
        if let Ok(m) = std::fs::metadata(p) {
            assert_eq!(m.permissions().mode() & 0o077, 0);
        }
    }
    drop(s);
    let alias = parent.join("alias.db");
    symlink(&path, &alias).unwrap();
    assert!(matches!(
        Store::open(&alias, "owner"),
        Err(Error::PrivateState)
    ));
    let wal = parent.join("state.db-wal");
    if wal.exists() {
        std::fs::remove_file(&wal).unwrap();
    }
    symlink(&path, &wal).unwrap();
    assert!(matches!(
        Store::open(&path, "owner"),
        Err(Error::PrivateState)
    ));
}

#[test]
fn observed_owner_mismatch_invalidates_grant_without_mutating_foreign_instance() {
    use m8s_provider::Provider;
    let (_, mut s, mut p, uid, b) = fixture();
    issue(&s, &uid, &b);
    let m8s_core::Observation::Present(mut instance) = p.describe(&b.instance_id).unwrap() else {
        panic!("synthetic fixture missing")
    };
    instance.owner = "other-owner".into();
    p.insert(instance).unwrap();
    s.reconcile(&mut p, 101).unwrap();
    assert!(consume(&s, &NONCE, &b, 102).is_err());
    assert!(s.bootstrap_attempts().unwrap()[0].cancelled);
    assert!(p.destroy_calls.is_empty());
    assert_eq!(s.nodes().unwrap().len(), 1);
}

#[test]
fn phase_and_uid_rebinding_cannot_inherit_launch_authority() {
    for change_uid in [false, true] {
        let (_, s, _, uid, b) = fixture();
        issue(&s, &uid, &b);
        if change_uid {
            s.conn
                .execute(
                    "UPDATE gpu_node SET uid='replacement-uid' WHERE uid=?",
                    [&uid],
                )
                .unwrap();
        } else {
            s.conn
                .execute(
                    "UPDATE gpu_node SET phase='\"Loading\"' WHERE uid=?",
                    [&uid],
                )
                .unwrap();
        }
        assert!(consume(&s, &NONCE, &b, 101).is_err());
        assert_eq!(s.nodes().unwrap().len(), 1);
        assert_eq!(
            s.bootstrap_attempts().unwrap()[0].stage,
            BootstrapStage::Issued
        );
    }
}
#[test]
fn nested_transaction_is_refused_before_any_external_effect() {
    let (_, s, _, uid, b) = fixture();
    issue(&s, &uid, &b);
    let tx = Transaction::new_unchecked(&s.conn, TransactionBehavior::Immediate).unwrap();
    assert!(consume(&s, &NONCE, &b, 101).is_err());
    assert_eq!(
        s.bootstrap_attempts().unwrap()[0].stage,
        BootstrapStage::Issued
    );
    tx.rollback().unwrap();
    consume(&s, &NONCE, &b, 101).unwrap();
    assert!(s.conn.is_autocommit());
}

#[test]
fn observations_require_complete_exact_identity_and_local_freshness() {
    let (_, s, _, uid, b) = fixture();
    for (status, observed_at) in [
        (ObservationStatus::Unknown, 100),
        (ObservationStatus::Error, 100),
        (ObservationStatus::Partial, 100),
        (ObservationStatus::Complete, 101),
        (ObservationStatus::Complete, 69),
        (ObservationStatus::Complete, -1),
    ] {
        let observation = LaunchIdentity {
            owner: "owner",
            node_uid: &uid,
            binding: &b,
            observed_at,
            status,
        };
        assert!(observation.validate("owner", &b, 100).is_err());
        assert!(s.issue_bootstrap_nonce(&NONCE, observation, 100).is_err());
    }
    let observation = LaunchIdentity {
        owner: "owner",
        node_uid: &uid,
        binding: &b,
        observed_at: 100,
        status: ObservationStatus::Complete,
    };
    assert!(observation.validate("owner", &b, 130).is_ok());
    assert!(observation.validate("owner", &b, 131).is_err());
    let mut wrong = b.clone();
    wrong.container_id.push('x');
    assert!(observation.validate("owner", &wrong, 100).is_err());
    assert!(observation.validate("foreign-owner", &b, 100).is_err());
    s.issue_bootstrap_nonce(&NONCE, observation, 100).unwrap();
}

#[test]
fn consumption_rechecks_fresh_complete_observation_and_atomic_uid_binding() {
    let (_, s, _, uid, b) = fixture();
    issue(&s, &uid, &b);
    let hash: [u8; 32] = Sha256::digest(NONCE).into();
    let record = ConsumeRecord {
        expected_uid: None,
        hash: &hash,
        binding: &b,
        node: EndpointId([2; 32]),
        now: 140,
        cert_expires_at: 2000,
    };
    for (status, observed_at, node_uid) in [
        (ObservationStatus::Complete, 100, uid.as_str()),
        (ObservationStatus::Complete, 141, uid.as_str()),
        (ObservationStatus::Partial, 140, uid.as_str()),
        (ObservationStatus::Unknown, 140, uid.as_str()),
        (ObservationStatus::Error, 140, uid.as_str()),
        (ObservationStatus::Complete, 140, "wrong-uid"),
    ] {
        let adapter = ObservedConsumer {
            store: &s,
            observation: LaunchIdentity {
                owner: "owner",
                node_uid,
                binding: &b,
                observed_at,
                status,
            },
        };
        assert!(adapter.consume_observed(&record).is_err());
        assert_eq!(
            s.bootstrap_attempts().unwrap()[0].stage,
            BootstrapStage::Issued
        );
    }
    let adapter = ObservedConsumer {
        store: &s,
        observation: LaunchIdentity {
            owner: "owner",
            node_uid: &uid,
            binding: &b,
            observed_at: 140,
            status: ObservationStatus::Complete,
        },
    };
    adapter.consume_observed(&record).unwrap();
    assert!(adapter.consume_observed(&record).is_err());
}
