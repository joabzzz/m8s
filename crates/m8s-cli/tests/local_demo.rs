// Only our synthetic fresh state is inspected. Never operator state or credentials.
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Root(tempfile::TempDir);
impl Root {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(root.path().join("sentinel"), b"synthetic-before-after").unwrap();
        Self(root)
    }
    fn parent(&self) -> PathBuf {
        self.0.path().canonicalize().unwrap()
    }
    fn state(&self) -> PathBuf {
        self.parent().join("fresh-state")
    }
    fn unchanged(&self) {
        assert_eq!(
            fs::read(self.parent().join("sentinel")).unwrap(),
            b"synthetic-before-after"
        );
    }
}
fn args(path: &Path, cap: &str, scenario: &str) -> Vec<OsString> {
    vec![
        "--synthetic-accounting".into(),
        "--state-dir".into(),
        path.into(),
        "--synthetic-cap-micro-usd".into(),
        cap.into(),
        "--scenario".into(),
        scenario.into(),
    ]
}
fn invoke(args: &[OsString]) -> Output {
    // No shell, nested cargo, generic child command or PID-based cleanup.
    // The shared runner retains its owned-child cleanup on every error path.
    Command::new(env!("CARGO_BIN_EXE_m8s"))
        .arg("local-demo")
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn help_explains_explicit_opt_in_without_state() {
    let root = Root::new();
    let out = invoke(&["--help".into()]);
    assert!(out.status.success());
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .contains("--features local-development")
    );
    assert!(!root.state().exists());
    root.unchanged();
}
#[cfg(not(all(unix, feature = "local-development")))]
#[test]
fn unavailable_fails_before_state_and_redacts_all_arguments() {
    let root = Root::new();
    let out = invoke(&args(&root.state(), "10000", "cancel"));
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    #[cfg(not(feature = "local-development"))]
    assert_eq!(
        out.stderr,
        b"m8s: local-demo unavailable: build with --features local-development\n"
    );
    #[cfg(all(feature = "local-development", not(unix)))]
    assert_eq!(
        out.stderr,
        b"m8s: local-demo unavailable on this platform\n"
    );
    assert!(!root.state().exists());
    root.unchanged();
}

#[cfg(all(unix, feature = "local-development"))]
mod enabled {
    use super::*;
    use serde_json::Value;

    fn lines(out: &Output) -> Vec<Value> {
        std::str::from_utf8(&out.stdout)
            .unwrap()
            .lines()
            .map(|line| {
                assert!(line.len() <= 1024);
                serde_json::from_str(line).unwrap()
            })
            .collect()
    }
    fn refusal(out: &Output, error: &str) {
        assert!(!out.status.success());
        let values = lines(out);
        let last = values.last().unwrap();
        assert_eq!(last["phase"], "failure");
        assert_eq!(last["error"], error);
        assert_eq!(last["no_reset"], true);
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(!text.contains("local_child_spawned"));
        assert!(!text.contains("Ready"));
    }
    fn journal(path: &Path) -> Value {
        serde_json::from_slice(&fs::read(path.join("local-journal/state.json")).unwrap()).unwrap()
    }
    fn lifecycle(scenario: &str) {
        let root = Root::new();
        let path = root.state();
        let out = invoke(&args(&path, "10000", scenario));
        assert!(out.status.success(), "{out:?}");
        assert!(out.stderr.is_empty());
        let values = lines(&out);
        assert!(values.iter().any(|v| v["phase"] == "local_child_spawned"));
        assert!(values.iter().any(|v| v["phase"]
            == if scenario == "cancel" {
                "stop_requested"
            } else {
                "deadline_stop_observed"
            }));
        let last = values.last().unwrap();
        assert_eq!(last["phase"], "summary");
        assert_eq!(last["journal_cleanup"], "LocalOnlyAbsent");
        assert_eq!(last["billing"], "unknown");
        assert_eq!(last["budget_state"], "Uncertain");
        assert_eq!(last["reserved_hold_retained"], true);
        let reserved = values
            .iter()
            .find(|v| v["phase"] == "reservation_persisted")
            .unwrap();
        assert!(last["simulated_hold_micro_usd"].as_u64().unwrap() > 0);
        assert_eq!(
            last["simulated_hold_micro_usd"],
            reserved["simulated_hold_micro_usd"]
        );
        let state: m8sd::budget::BudgetState =
            serde_json::from_value(last["budget_state"].clone()).unwrap();
        assert_eq!(state, m8sd::budget::BudgetState::Uncertain);
        assert_eq!(last["claim_state"], "consumed");
        assert_eq!(last["cleanup_required"], true);
        for forbidden in ["Ready", "budget_refunded", "spent", "proof"] {
            assert!(!String::from_utf8_lossy(&out.stdout).contains(forbidden));
        }
        // Shared runner verifies typed DB reservations/operations before summary.
        // Public CLI test reads only its own journal and snapshots its synthetic DB.
        let records = journal(&path)["records"].as_object().unwrap().clone();
        assert_eq!(records.len(), 1);
        assert_eq!(records.values().next().unwrap()["status"], "Absent");
        let db = fs::read(path.join("state.db")).unwrap();
        let journal_before = fs::read(path.join("local-journal/state.json")).unwrap();
        refusal(
            &invoke(&args(&path, "10000", scenario)),
            "state-path-must-be-new-no-reset",
        );
        assert_eq!(fs::read(path.join("state.db")).unwrap(), db);
        assert_eq!(
            fs::read(path.join("local-journal/state.json")).unwrap(),
            journal_before
        );
        root.unchanged();
    }
    #[test]
    fn cancel_real_binary_retains_unknown_full_hold() {
        lifecycle("cancel");
    }
    #[test]
    fn deadline_real_binary_retains_unknown_full_hold() {
        lifecycle("deadline");
    }
    #[test]
    fn malformed_arguments_refuse_before_any_state_or_child() {
        let root = Root::new();
        let path = root.state();
        let good = args(&path, "10000", "cancel");
        let mut cases = vec![vec![], args(&path, "10000", "unknown")];
        for index in [0, 1, 3, 5] {
            let mut bad = good.clone();
            if index == 0 {
                bad.remove(0);
            } else {
                bad.drain(index..index + 2);
            }
            cases.push(bad);
        }
        for cap in [
            "0",
            "-1",
            "+1",
            "1.0",
            "1000001",
            "18446744073709551616",
            " 1",
            "",
        ] {
            cases.push(args(&path, cap, "cancel"));
        }
        for extra in [
            vec!["--synthetic-accounting"],
            vec!["--state-dir", "opaque"],
            vec!["--synthetic-cap-micro-usd", "10"],
            vec!["--scenario", "cancel"],
            vec!["--json"],
            vec!["--socket", "opaque"],
            vec!["--force"],
            vec!["opaque"],
            vec!["opaque"; 18],
        ] {
            let mut bad = good.clone();
            bad.extend(extra.into_iter().map(OsString::from));
            cases.push(bad);
        }
        cases.push(args(Path::new("relative"), "10000", "cancel"));
        cases.push(args(&root.parent().join("a/../b"), "10000", "cancel"));
        for bad in cases {
            refusal(&invoke(&bad), "invalid-arguments");
            assert!(!path.exists());
            root.unchanged();
        }
    }
    #[test]
    fn tiny_cap_keeps_empty_journal_and_nonzero_failure() {
        let root = Root::new();
        refusal(
            &invoke(&args(&root.state(), "1", "cancel")),
            "synthetic-cap-refused-before-child",
        );
        assert!(root.state().join("state.db").is_file());
        assert!(
            journal(&root.state())["records"]
                .as_object()
                .unwrap()
                .is_empty()
        );
        root.unchanged();
    }
    #[test]
    fn existing_and_symlinked_state_and_parent_never_mutate_targets() {
        use std::os::unix::fs::{DirBuilderExt, symlink};
        let root = Root::new();
        let target = root.parent().join("existing");
        fs::DirBuilder::new().mode(0o700).create(&target).unwrap();
        fs::write(target.join("sentinel"), b"synthetic-target").unwrap();
        for path in [target.clone(), root.state()] {
            if path != target {
                symlink(&target, &path).unwrap();
            }
            refusal(
                &invoke(&args(&path, "10000", "cancel")),
                "state-path-must-be-new-no-reset",
            );
            assert!(!target.join("state.db").exists());
            assert_eq!(
                fs::read(target.join("sentinel")).unwrap(),
                b"synthetic-target"
            );
        }
        let alias = root.parent().join("alias");
        symlink(&target, &alias).unwrap();
        refusal(
            &invoke(&args(&alias.join("new"), "10000", "cancel")),
            "private-state-unavailable",
        );
        assert!(!target.join("new").exists());
        root.unchanged();
    }
}

#[cfg(unix)]
#[test]
fn whole_argv_utf8_contract_still_refuses_without_effects() {
    use std::os::unix::ffi::OsStringExt;
    let root = Root::new();
    let mut bad = args(&root.state(), "10000", "cancel");
    bad.push(OsString::from_vec(vec![0xff]));
    let out = invoke(&bad);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert_eq!(out.stderr, b"m8s: invalid arguments\n");
    assert!(!root.state().exists());
    root.unchanged();
}
