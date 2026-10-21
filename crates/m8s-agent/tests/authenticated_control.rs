#![cfg(feature = "authenticated-control")]

use m8s_agent::{
    DeadManSwitch, Effect, Outcome,
    control::{ControlError, EndpointId, VerifiedHeartbeat},
    supervisor::{ChildSpec, RetryTeardown, Supervisor, TeardownDriver, UnavailableBoot},
};

struct NeverCalled;
impl TeardownDriver for NeverCalled {
    fn perform(&mut self, _: Effect) -> Outcome {
        panic!("a refused start must not perform effects")
    }
}

#[test]
fn trusted_node_configuration_does_not_open_boot_gate() {
    let result = Supervisor::start_with_control_node(
        ChildSpec {
            program: "/synthetic/must-not-spawn".into(),
            args: vec![],
        },
        &mut UnavailableBoot,
        DeadManSwitch::new("owner".into(), vec![], 0, 10, 100).unwrap(),
        NeverCalled,
        RetryTeardown::new(4).unwrap(),
        EndpointId([7; 32]),
    );
    assert!(result.is_err());
    assert_eq!(
        result.err().unwrap().to_string(),
        "secure bootstrap unavailable or rejected"
    );
}

// Compile the actual public boundary without manufacturing an event or starting
// iroh. There is no per-event target or raw identity argument.
#[test]
fn verified_dispatch_signature_is_move_only_and_target_free() {
    type Dispatch =
        fn(&mut Supervisor<NeverCalled>, VerifiedHeartbeat<'_>, i64) -> Result<bool, ControlError>;
    let _: Dispatch = Supervisor::verified_heartbeat;
}
