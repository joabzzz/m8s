#![cfg(unix)]
//! Public fresh-workspace creation, only own private temporary parents.
mod support;
use serde_json::{Value, json};
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::{
    ffi::OsString,
    fs::{self, File},
    os::unix::ffi::OsStringExt,
    path::PathBuf,
    process::Output,
};
use support::Client;
struct Workspace {
    _dir: tempfile::TempDir,
    path: PathBuf,
}
impl Workspace {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap().join("SECRET-workspace");
        Self { _dir: dir, path }
    }
    fn args(&self) -> Vec<OsString> {
        vec![
            "init".into(),
            "--mock".into(),
            "--directory".into(),
            self.path.clone().into(),
            "--json".into(),
        ]
    }
    fn run(&self) -> Output {
        Client::spawn(&self.args(), false).output()
    }
}
fn refused(out: Output, code: &str) {
    assert!(!out.status.success());
    assert_eq!(out.stderr, format!("m8s: {code}\n").as_bytes());
    assert!(out.stdout.is_empty());
}
fn inventory(path: &std::path::Path) -> Vec<(String, u32, Vec<u8>)> {
    let mut out = Vec::new();
    for entry in fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        let m = fs::symlink_metadata(entry.path()).unwrap();
        out.push((
            entry.file_name().into_string().unwrap(),
            m.mode(),
            if m.is_file() {
                fs::read(entry.path()).unwrap()
            } else {
                vec![]
            },
        ));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}
#[test]
fn fresh_exact_private_examples_validate_and_output_is_relative_deterministic() {
    let w = Workspace::new();
    let out = w.run();
    assert!(out.status.success(), "{out:?}");
    assert!(out.stderr.is_empty());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["daemon_started"], false);
    assert_eq!(v["files"], json!(["m8s.toml", "catalog.toml", "README.md"]));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("SECRET"));
    assert!(out.stdout.len() < 1024);
    assert_eq!(fs::metadata(&w.path).unwrap().mode() & 0o7777, 0o700);
    let files = inventory(&w.path);
    assert_eq!(
        files.iter().map(|e| e.0.as_str()).collect::<Vec<_>>(),
        ["README.md", "catalog.toml", "m8s.toml", "state"]
    );
    for name in ["m8s.toml", "catalog.toml", "README.md"] {
        let m = fs::metadata(w.path.join(name)).unwrap();
        assert_eq!(m.mode() & 0o7777, 0o600);
        assert_eq!(m.nlink(), 1);
        assert_eq!(m.uid(), nix::unistd::geteuid().as_raw());
    }
    assert_eq!(
        fs::read(w.path.join("m8s.toml")).unwrap(),
        m8s_config::EXAMPLE.as_bytes()
    );
    assert_eq!(
        fs::read(w.path.join("catalog.toml")).unwrap(),
        m8s_models::EXAMPLE_CATALOG.as_bytes()
    );
    let readme = fs::read_to_string(w.path.join("README.md")).unwrap();
    assert!(readme.contains("\"$PWD/state\""));
    assert!(readme.contains("Mock-only"));
    assert!(readme.contains("reload-config --check"));
    assert!(readme.contains("logs --follow"));
    assert!(!readme.contains("SECRET"));
    let catalog =
        toml::from_str(&fs::read_to_string(w.path.join("catalog.toml")).unwrap()).unwrap();
    m8s_config::Config::from_path(
        &w.path.join("m8s.toml"),
        &catalog,
        &m8s_config::ValidationLimits::default(),
    )
    .unwrap();
    assert_eq!(
        fs::metadata(w.path.join("state")).unwrap().mode() & 0o7777,
        0o700
    );
    assert_eq!(fs::read_dir(w.path.join("state")).unwrap().count(), 0);
    refused(w.run(), "init-failed-partial-artifacts-may-remain");
    assert_eq!(inventory(&w.path), files);
    let other = Workspace::new();
    let mut args = other.args();
    args.pop();
    let second = Client::spawn(&args, false).output();
    assert!(second.status.success());
    assert_eq!(second.stdout, out.stdout);
    assert_eq!(inventory(&other.path), files);
}
#[test]
fn existing_entries_never_adopt_overwrite_or_change_permissions() {
    for kind in 0..3 {
        let w = Workspace::new();
        if kind == 0 {
            fs::create_dir(&w.path).unwrap();
            fs::set_permissions(&w.path, fs::Permissions::from_mode(0o755)).unwrap();
        } else if kind == 1 {
            fs::write(&w.path, b"sentinel").unwrap();
        } else {
            let target = w.path.with_file_name("target");
            fs::write(&target, b"sentinel").unwrap();
            symlink(target, &w.path).unwrap();
        }
        let before = fs::symlink_metadata(&w.path).unwrap();
        refused(w.run(), "init-failed-partial-artifacts-may-remain");
        let after = fs::symlink_metadata(&w.path).unwrap();
        assert_eq!(
            (before.mode(), before.ino(), before.len()),
            (after.mode(), after.ino(), after.len())
        );
        if kind != 0 {
            assert_eq!(fs::read(&w.path).unwrap(), b"sentinel");
        }
        if kind == 0 {
            assert_eq!(fs::read_dir(&w.path).unwrap().count(), 0);
        }
    }
}
#[test]
fn unsafe_or_noncanonical_parents_refuse_without_children_or_repairs() {
    let w = Workspace::new();
    let parent = w.path.parent().unwrap();
    let unsafe_parent = parent.join("unsafe");
    fs::create_dir(&unsafe_parent).unwrap();
    fs::set_permissions(&unsafe_parent, fs::Permissions::from_mode(0o777)).unwrap();
    let alias = parent.join("alias");
    symlink(parent, &alias).unwrap();
    for path in [
        unsafe_parent.join("child"),
        alias.join("child"),
        parent.join("missing").join("child"),
        PathBuf::from(format!("{}/./child", parent.display())),
        PathBuf::from(format!("{}//child", parent.display())),
        PathBuf::from(format!("{}/child/", parent.display())),
        parent.join("..").join("child"),
    ] {
        let mut args = w.args();
        args[3] = path.into();
        refused(
            Client::spawn(&args, false).output(),
            "init-failed-partial-artifacts-may-remain",
        );
    }
    assert_eq!(fs::metadata(&unsafe_parent).unwrap().mode() & 0o7777, 0o777);
    assert_eq!(fs::read_dir(&unsafe_parent).unwrap().count(), 0);
    assert!(!parent.join("child").exists());
    assert!(!parent.join("missing").exists());
}
#[test]
fn strict_explicit_mock_utf8_and_effectfree_help() {
    let w = Workspace::new();
    for extra in [
        vec!["--mock"],
        vec!["--json"],
        vec!["--directory", "SECRET"],
        vec!["--force"],
        vec!["--help"],
        vec!["SECRET"],
    ] {
        let mut args = w.args();
        args.extend(extra.into_iter().map(OsString::from));
        refused(
            Client::spawn(&args, false).output(),
            "invalid-init-arguments",
        );
        assert!(!w.path.exists());
    }
    let mut args = w.args();
    args.remove(1);
    refused(
        Client::spawn(&args, false).output(),
        "invalid-init-arguments",
    );
    let mut args = w.args();
    args[3] = "SECRET-relative".into();
    refused(
        Client::spawn(&args, false).output(),
        "invalid-init-arguments",
    );
    let mut args = w.args();
    args[3] = OsString::from_vec(vec![255]);
    refused(Client::spawn(&args, false).output(), "invalid arguments");
    let out = Client::spawn(&["init".into(), "--help".into()], false).output();
    assert!(out.status.success());
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .contains("Partial files may remain")
    );
    assert!(!w.path.exists());
}
#[test]
fn broken_stdout_does_not_undo_or_retry_completed_workspace() {
    let w = Workspace::new();
    refused(
        Client::spawn(&w.args(), true).output(),
        "init-output-unavailable",
    );
    let before = inventory(&w.path);
    assert_eq!(before.len(), 4);
    refused(w.run(), "init-failed-partial-artifacts-may-remain");
    assert_eq!(inventory(&w.path), before);
}

#[test]
fn generated_no_acl_descriptors_pass_borrowed_acl_metadata_check_without_mutation() {
    let w = Workspace::new();
    let out = w.run();
    assert!(out.status.success());
    let before = inventory(&w.path);
    for name in ["m8s.toml", "catalog.toml", "README.md", "state"] {
        let fd = File::open(w.path.join(name)).unwrap();
        m8s_secrets::check_no_extended_acl(&fd).unwrap();
    }
    assert_eq!(inventory(&w.path), before);
}
