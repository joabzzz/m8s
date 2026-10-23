//! Control semantics with a scripted port; no network, credential or live clock.
use super::*;
use crate::operation::vast::tests::{executor, setup};
use std::{cell::RefCell, rc::Rc};

#[derive(Default)]
struct Calls(Rc<RefCell<Vec<&'static str>>>);
struct Port {
    calls: Rc<RefCell<Vec<&'static str>>>,
    healthy: bool,
}
impl RestartPort for Port {
    fn lookup(&mut self, _: &VastRecovery) -> m8s_provider::Result<Option<NonZeroU64>> {
        self.calls.borrow_mut().push("lookup");
        Ok(None) // an empty lookup is never absence
    }
    fn destroy(&mut self, _: &VastRecovery, _: NonZeroU64) -> m8s_provider::Result<()> {
        self.calls.borrow_mut().push("destroy");
        Ok(())
    }
    fn confirm_absent(
        &mut self,
        _: &VastRecovery,
        instance: NonZeroU64,
    ) -> m8s_provider::Result<Option<Absence>> {
        self.calls.borrow_mut().push("confirm");
        Ok(self
            .healthy
            .then(|| Absence::synthetic(instance, 150, true)))
    }
}

fn bound_store() -> (Store, crate::operation::vast::tests::Directory) {
    let (store, directory) = setup(6_000_000, 1);
    let executor = executor(42);
    let intent = executor.intent("run", "request", "operation").unwrap();
    executor.prepare_at(&store, &intent, || Ok(100)).unwrap();
    let claim = store.claim_create("operation", 100).unwrap();
    assert!(
        start_bound_invocation(
            &store,
            &claim,
            InvocationClock::Explicit(100),
            "smoke",
            false
        )
        .unwrap()
    );
    store
        .record_create_result(
            "operation",
            &CreateObservation::Bound(ResourceBinding {
                owner: "owner".into(),
                run_id: "run".into(),
                node_uid: "node".into(),
                instance_id: "42".into(),
                volumes: VolumeBinding::NoneAllocated,
                provenance: "synthetic-exact-observation".into(),
                observed_at: 101,
                status: EvidenceStatus::Complete,
            }),
            101,
        )
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE gpu_node SET instance_id='42',phase='\"Booting\"' WHERE uid='node'",
            [],
        )
        .unwrap();
    (store, directory)
}

#[test]
fn refuses_to_start_on_empty_or_mock_state() {
    let path = std::env::temp_dir().join(format!("m8sd-daemon-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir(&path).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    let store = Store::open(path.join("state.db"), "owner").unwrap();
    store.scale("smoke", 1).unwrap();
    let port = Port {
        calls: Calls::default().0,
        healthy: true,
    };
    assert!(VastDaemon::new(store, port, CleanupTrigger::AtDeadline).is_err());
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn launch_scale_reload_and_shutdown_requests_are_refused_without_effect() {
    let (store, _directory) = bound_store();
    let calls = Calls::default();
    let mut daemon = VastDaemon::new(
        store,
        Port {
            calls: calls.0.clone(),
            healthy: true,
        },
        CleanupTrigger::AtDeadline,
    )
    .unwrap();
    let before = daemon.store().conn.total_changes();
    for request in [
        Request::Up {
            tier: "smoke".into(),
        },
        Request::Scale {
            tier: "smoke".into(),
            replicas: 1,
        },
        Request::Scale {
            tier: "other".into(),
            replicas: 2,
        },
        Request::ScaleBatch { targets: vec![] },
        Request::ReloadConfig {},
        Request::Shutdown {},
    ] {
        let response = daemon.handle(request);
        assert!(!response.ok);
        assert_eq!(response.mode, m8s_api::VAST_DEMO_MODE);
        assert_eq!(response.secure_bootstrap, m8s_api::VAST_DEMO_SECURITY);
        assert!(
            response
                .nodes
                .iter()
                .all(|n| n.phase != m8s_core::Phase::Ready)
        );
    }
    assert_eq!(daemon.store().conn.total_changes(), before);
    assert!(calls.0.borrow().is_empty());
    assert_eq!(daemon.store().tiers().unwrap(), [("smoke".into(), 1)]);
    let status = daemon.handle(Request::Status {});
    assert!(status.ok && status.nodes.len() == 1);
    assert_eq!(
        daemon.store().conn.total_changes(),
        before,
        "status is read-only"
    );
}

#[test]
fn down_brings_cleanup_forward_but_never_renews_or_claims() {
    let (store, _directory) = bound_store();
    let calls = Calls::default();
    let mut daemon = VastDaemon::new(
        store,
        Port {
            calls: calls.0.clone(),
            healthy: true,
        },
        CleanupTrigger::AtDeadline,
    )
    .unwrap();
    // Before the original serving deadline (160) and with desired=1: wait.
    let report = daemon.tick(|| Ok(150)).unwrap();
    assert_eq!(report[0].outcome, Outcome::AwaitingCleanup);
    assert!(calls.0.borrow().is_empty());
    assert!(daemon.handle(Request::Down {}).ok);
    let report = daemon.tick(|| Ok(150)).unwrap();
    assert_eq!(report[0].outcome, Outcome::Removed);
    assert_eq!(*calls.0.borrow(), ["destroy", "confirm"]);
    let response = daemon.handle(Request::Status {});
    assert!(response.ok && response.nodes.is_empty());
    assert_eq!(response.tiers, [("smoke".into(), 0)]);
    assert!(daemon.store().claim_create("operation", 300).is_err());
}

#[test]
fn unconfirmed_absence_keeps_the_node_visible_in_status() {
    let (store, _directory) = bound_store();
    let calls = Calls::default();
    let mut daemon = VastDaemon::new(
        store,
        Port {
            calls: calls.0.clone(),
            healthy: false,
        },
        CleanupTrigger::Immediate,
    )
    .unwrap();
    for _ in 0..3 {
        let report = daemon.tick(|| Ok(200)).unwrap();
        assert_eq!(report[0].outcome, Outcome::AbsenceUnconfirmed);
        let status = daemon.handle(Request::Status {});
        assert!(status.ok);
        assert_eq!(status.nodes.len(), 1, "retained, not absent");
        assert_eq!(status.nodes[0].phase, m8s_core::Phase::Destroying);
    }
}
