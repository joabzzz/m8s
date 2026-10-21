#![cfg(unix)]
use m8s_agent::supervisor::LocalTerminationState;
use m8s_core::{Instance, InstanceState, Observation};
use m8s_provider::{Error, LaunchSpec, Provider};
use m8s_provider_local::{DevelopmentCommand, Local, LocalDeadlines, LocalLaunch};
use std::{
    ffi::OsString,
    fs,
    os::unix::{ffi::OsStringExt, fs::PermissionsExt},
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "m8s-timed-integration-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
    fn open(&self) -> Local {
        let end = Instant::now() + Duration::from_secs(3);
        loop {
            match Local::open_development(&self.0, "owner".into(), command()) {
                Ok(p) => return p,
                Err(Error::Unavailable) if Instant::now() < end => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(e) => panic!("open failed: {e:?}"),
            }
        }
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn command() -> DevelopmentCommand {
    DevelopmentCommand::new("/bin/sleep".into(), vec!["60".into()]).unwrap()
}
fn spec(uid: &str) -> LaunchSpec {
    LaunchSpec {
        owner: "owner".into(),
        node_uid: uid.into(),
        tier: "dev".into(),
    }
}
fn deadlines(after: Duration) -> LocalDeadlines {
    let stop = SystemTime::now() + after;
    LocalDeadlines::new(stop, stop + Duration::from_secs(2)).unwrap()
}
fn wait_reaped(launch: &LocalLaunch) {
    let end = Instant::now() + Duration::from_secs(4);
    loop {
        if launch.termination_state() == LocalTerminationState::DirectChildReaped {
            return;
        }
        assert!(Instant::now() < end, "direct child was not reaped");
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn deadline_progresses_while_caller_mutex_is_held_and_requires_explicit_destroy() {
    let dir = Directory::new();
    let mut local = dir.open();
    let launch = local
        .launch_development_until(&spec("a"), deadlines(Duration::from_millis(250)))
        .unwrap();
    let id = launch.instance().id.clone();
    let local = Mutex::new(local);
    let mut guard = local.lock().unwrap();
    wait_reaped(&launch); // worker cannot acquire this mutex, nor call any Local method
    assert!(!launch.cleanup_deadline_missed());
    assert!(matches!(
        guard.describe(&id).unwrap(),
        Observation::Present(Instance {
            state: InstanceState::Stopped,
            ..
        })
    ));
    guard.destroy(&id).unwrap();
    assert_eq!(guard.describe(&id).unwrap(), Observation::Absent);
}
#[test]
fn cloned_stop_is_latched_and_cannot_renew_or_launch() {
    let dir = Directory::new();
    let mut local = dir.open();
    let launch = local
        .launch_development_until(&spec("a"), deadlines(Duration::from_secs(20)))
        .unwrap();
    let stop = launch.stop_handle();
    let other = stop.clone();
    stop.request_stop();
    for _ in 0..1000 {
        other.request_stop();
    }
    wait_reaped(&launch);
    assert!(!launch.cleanup_deadline_missed());
    local.destroy(&launch.instance().id).unwrap();
    assert_eq!(
        local.describe(&launch.instance().id).unwrap(),
        Observation::Absent
    );
}
#[test]
fn drop_launch_and_local_cleanup_but_restart_stays_uncertain() {
    let dir = Directory::new();
    let mut local = dir.open();
    let launch = local
        .launch_development_until(&spec("a"), deadlines(Duration::from_secs(20)))
        .unwrap();
    let id = launch.instance().id.clone();
    drop(local); // asynchronous stop, worker retains store until reap
    wait_reaped(&launch);
    drop(launch);
    let mut reopened = dir.open();
    assert!(reopened.describe(&id).is_err());
    assert!(reopened.destroy(&id).is_err());
    assert!(
        reopened
            .launch_development_until(&spec("b"), deadlines(Duration::from_secs(1)))
            .is_err()
    );
    let bytes = fs::read_to_string(dir.0.join("state.json")).unwrap();
    assert!(!bytes.contains("pid"));
    assert!(!bytes.contains("/bin/sleep"));
}
#[test]
fn drop_launch_alone_stops_and_one_active_limit_includes_untimed_launches() {
    let dir = Directory::new();
    let mut local = dir.open();
    let launch = local
        .launch_development_until(&spec("a"), deadlines(Duration::from_secs(20)))
        .unwrap();
    let id = launch.instance().id.clone();
    assert!(
        local
            .launch_development_until(&spec("b"), deadlines(Duration::from_secs(1)))
            .is_err()
    );
    let offer = local.search().unwrap().remove(0);
    assert!(local.launch(&offer, &spec("c")).is_err());
    drop(launch);
    let end = Instant::now() + Duration::from_secs(4);
    loop {
        if matches!(
            local.describe(&id).unwrap(),
            Observation::Present(Instance {
                state: InstanceState::Stopped,
                ..
            })
        ) {
            break;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(5));
    }
    local.destroy(&id).unwrap();
    assert_eq!(local.describe(&id).unwrap(), Observation::Absent);
}
#[test]
fn natural_exit_is_stopped_and_storage_failure_is_not_absence() {
    let dir = Directory::new();
    // Keep a controlled synthetic group member alive through leader exit: this
    // sandbox can deny kill on an already-empty group (not absence evidence).
    let mut local = Local::open_development(
        &dir.0,
        "owner".into(),
        DevelopmentCommand::new(
            "/bin/sh".into(),
            vec!["-c".into(), "/bin/sleep 60 & exit 0".into()],
        )
        .unwrap(),
    )
    .unwrap();
    let launch = local
        .launch_development_until(&spec("a"), deadlines(Duration::from_secs(2)))
        .unwrap();
    wait_reaped(&launch);
    assert!(matches!(
        local.describe(&launch.instance().id).unwrap(),
        Observation::Present(Instance {
            state: InstanceState::Stopped,
            ..
        })
    ));
    // Journal uncertainty does not erase retained child evidence, nor authorize absence.
    fs::rename(dir.0.join("state.json"), dir.0.join("displaced")).unwrap();
    assert!(local.destroy(&launch.instance().id).is_err());
    assert!(local.describe(&launch.instance().id).is_err());
    assert_eq!(
        launch.termination_state(),
        LocalTerminationState::DirectChildReaped
    );
}
#[test]
fn canonical_raw_bytes_boundaries_order_and_limits() {
    let make = |args: Vec<OsString>| {
        DevelopmentCommand::new("/bin/sleep".into(), args)
            .unwrap()
            .fingerprint()
    };
    assert_eq!(make(vec!["1".into()]), make(vec!["1".into()]));
    assert_eq!(
        make(vec!["1".into()])
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        "c4e228648d6bb5b8eaca00dcbd064a778046c8b1935d3257d46812578c14487d"
    );
    assert_ne!(
        make(vec!["ab".into(), "c".into()]),
        make(vec!["a".into(), "bc".into()])
    );
    assert_ne!(make(vec![]), make(vec!["".into()]));
    assert_ne!(
        make(vec!["a".into(), "b".into()]),
        make(vec!["b".into(), "a".into()])
    );
    assert_ne!(
        make(vec![OsString::from_vec(vec![0xff])]),
        make(vec![OsString::from_vec(vec![0xfe])])
    );
    assert_ne!(
        make(vec![]),
        DevelopmentCommand::new("/bin/true".into(), vec![])
            .unwrap()
            .fingerprint()
    );
    assert!(DevelopmentCommand::new("relative".into(), vec![]).is_err());
    assert!(DevelopmentCommand::new("/bin/sleep".into(), vec!["".into(); 257]).is_err());
    assert!(DevelopmentCommand::new("/bin/sleep".into(), vec!["".into(); 256]).is_ok());
    let budget = 64 * 1024 - b"/bin/sleep".len();
    assert!(DevelopmentCommand::new("/bin/sleep".into(), vec!["x".repeat(budget).into()]).is_ok());
    assert!(
        DevelopmentCommand::new("/bin/sleep".into(), vec!["x".repeat(budget + 1).into()]).is_err()
    );
    assert!(
        DevelopmentCommand::new("/bin/sleep".into(), vec![OsString::from_vec(vec![0])]).is_err()
    );
    assert!(
        DevelopmentCommand::new(
            PathBuf::from(OsString::from_vec(b"/bin/\0sleep".to_vec())),
            vec![]
        )
        .is_err()
    );
}

#[test]
fn live_destroy_requests_stop_even_when_storage_validation_fails() {
    let dir = Directory::new();
    let mut local = dir.open();
    let launch = local
        .launch_development_until(&spec("a"), deadlines(Duration::from_secs(20)))
        .unwrap();
    fs::rename(dir.0.join("state.json"), dir.0.join("displaced")).unwrap();
    assert!(local.destroy(&launch.instance().id).is_err());
    wait_reaped(&launch); // no valid journal or subsequent Local call required
    assert!(local.describe(&launch.instance().id).is_err());
    assert!(
        !fs::read_to_string(dir.0.join("displaced"))
            .unwrap()
            .contains("Absent")
    );
}
