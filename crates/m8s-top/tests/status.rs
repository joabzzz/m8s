#![cfg(unix)]
use m8s_top::{
    client::{ReadError, StatusClient, decode_status},
    view::{Dashboard, text_snapshot},
};
use std::io::{Read, Write};
use std::os::unix::{
    fs::PermissionsExt,
    net::{UnixListener, UnixStream},
};
use std::time::{Duration, Instant};

fn fixture() -> Vec<u8> {
    let mut bytes=serde_json::to_vec(&serde_json::json!({
        "ok":true,"mode":"mock-only","nodes":[
            {"uid":"node-a","tier":"synthetic","owner":"synthetic-owner","instance_id":"instance-a","phase":"Destroying","created_at":42},
            {"uid":"node-b","tier":"synthetic","owner":"synthetic-owner","instance_id":null,"phase":"Booting","created_at":43},
            {"uid":"node-c","tier":"synthetic","owner":"synthetic-owner","instance_id":"instance-c","phase":"Ready","created_at":44}
        ],"tiers":[["synthetic",3]],"events":[],"error":null,"secure_bootstrap":"unavailable; cannot become Ready"
    })).unwrap();
    bytes.push(b'\n');
    bytes
}
fn server<F: FnOnce(UnixStream) + Send + 'static>(
    timeout: Duration,
    send: F,
) -> Result<m8s_top::StatusSnapshot, ReadError> {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = dir.path().canonicalize().unwrap().join("status.sock");
    let listener = UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let worker = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    // BSD sockets can inherit listener O_NONBLOCK on accept.
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    let request = m8s_api::read_frame(&mut stream).unwrap();
                    assert!(matches!(
                        serde_json::from_slice::<m8s_api::Request>(&request).unwrap(),
                        m8s_api::Request::Status {}
                    ));
                    send(stream);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "client never connected to synthetic socket"
                    );
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("synthetic socket failed: {error}"),
            }
        }
    });
    let result = StatusClient::new(timeout).unwrap().status(&path);
    worker.join().unwrap();
    result
}
#[test]
fn status_request_is_read_only_and_missing_evidence_stays_unknown() {
    let raw = fixture();
    assert!(serde_json::from_slice::<m8s_api::Response>(&raw).is_ok());
    let snapshot = server(Duration::from_secs(1), move |mut stream| {
        stream.write_all(&raw).unwrap();
    })
    .unwrap();
    assert_eq!(snapshot.nodes.len(), 3);
    assert_eq!(snapshot.nodes[0].destroy_pending, Some(true));
    for node in &snapshot.nodes {
        assert!(node.last_observation.is_none());
        assert!(node.model_ready.is_none());
    }
    assert!(snapshot.bootstrap_verified.is_none());
    assert!(snapshot.hourly_micro_usd.is_none());
    let json = serde_json::to_value(snapshot).unwrap();
    assert_eq!(json["nodes"][2]["reported_phase"], "Ready");
    assert!(json["nodes"][2]["model_ready"].is_null());
    assert!(json["heartbeat_age_seconds"].is_null());
}
#[test]
fn malformed_wrong_schema_unknown_phase_duplicate_and_missing_fields_fail_closed() {
    let base: serde_json::Value = serde_json::from_slice(&fixture()).unwrap();
    let mut cases = vec![
        b"not-json\n".to_vec(),
        b"{}\n".to_vec(),
        b"{\"ok\":true,\"ok\":true}\n".to_vec(),
    ];
    cases.push(
        String::from_utf8(fixture())
            .unwrap()
            .replacen("\"ok\":true", "\"ok\":true,\"ok\":true", 1)
            .into_bytes(),
    );
    for edit in 0..8 {
        let mut value = base.clone();
        match edit {
            0 => {
                value["unexpected"] = true.into();
            }
            1 => {
                value["nodes"][0]["phase"] = "Running".into();
            }
            2 => {
                value["nodes"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("instance_id");
            }
            3 => {
                value.as_object_mut().unwrap().remove("error");
            }
            4 => {
                value["tiers"][0][1] = (-1).into();
            }
            5 => {
                value["nodes"][0]["unexpected"] = true.into();
            }
            6 => {
                value["nodes"][1]["uid"] = "node-a".into();
            }
            _ => {
                value["error"] = "synthetic-error-in-success".into();
            }
        }
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        cases.push(bytes);
    }
    for data in cases {
        assert!(matches!(
            decode_status(&data),
            Err(ReadError::InvalidResponse)
        ));
    }
    let mut missing_newline = fixture();
    missing_newline.pop();
    assert!(matches!(
        decode_status(&missing_newline),
        Err(ReadError::InvalidResponse)
    ));
}
#[test]
fn oversized_truncated_and_extra_frames_are_rejected() {
    for (index, bytes) in [
        vec![b'x'; m8s_api::MAX_FRAME + 1],
        b"{}".to_vec(),
        b"{}\n{}\n".to_vec(),
    ]
    .into_iter()
    .enumerate()
    {
        let result = server(Duration::from_secs(1), move |mut stream| {
            let _ = stream.write_all(&bytes);
            // Half-close truncated/multiple frames, then retain the socket
            // until rejection. macOS can reset pending bytes on full close.
            if index != 0 {
                stream.shutdown(std::net::Shutdown::Write).unwrap();
            }
            let mut scratch = [0u8; 1];
            let _ = stream.read(&mut scratch);
        });
        assert!(
            matches!(result, Err(ReadError::InvalidResponse)),
            "case {index}: {:?}",
            result.err()
        );
    }
}
#[test]
fn total_deadline_bounds_silent_and_slow_drip_servers() {
    let start = Instant::now();
    let result = server(Duration::from_millis(100), |_stream| {
        std::thread::sleep(Duration::from_millis(200))
    });
    assert!(matches!(result, Err(ReadError::Timeout)));
    assert!(start.elapsed() < Duration::from_secs(1));
    let start = Instant::now();
    let result = server(Duration::from_millis(100), |mut stream| {
        for _ in 0..50 {
            if stream.write_all(b" ").is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    assert!(matches!(result, Err(ReadError::Timeout)));
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(StatusClient::new(Duration::ZERO).is_err());
    assert!(StatusClient::new(Duration::from_secs(11)).is_err());
}
#[test]
fn rendering_snapshot_is_unqualified_and_stale_data_never_looks_live() {
    let mut dashboard = Dashboard::default();
    dashboard.update(decode_status(&fixture()));
    let text = text_snapshot(&dashboard, 150, 20).unwrap();
    assert!(text.contains("reported Ready (unverified)"));
    assert!(text.contains("PENDING / unconfirmed"));
    assert!(text.contains("heartbeat=unknown"));
    assert!(text.contains("readiness NOT verified"));
    assert!(!text.contains("last observation=42"));
    assert_eq!(text, include_str!("snapshots/unqualified.txt"));
    dashboard.update(Err(ReadError::Timeout));
    let stale = text_snapshot(&dashboard, 150, 20).unwrap();
    assert!(stale.contains("STALE / UNKNOWN"));
    assert!(stale.contains("node-a"));
    let unavailable = Dashboard {
        error: Some(ReadError::Unavailable),
        ..Default::default()
    };
    assert!(
        text_snapshot(&unavailable, 150, 20)
            .unwrap()
            .contains("not an empty fleet")
    );
    assert!(text_snapshot(&dashboard, 301, 20).is_err());
    assert!(text_snapshot(&dashboard, 150, 121).is_err());
    assert_eq!(
        m8s_top::view::bounded_area(u16::MAX, u16::MAX),
        ratatui::layout::Rect::new(0, 0, 300, 120)
    );
    for (width, height) in [(1, 1), (39, 11), (40, 12)] {
        assert!(text_snapshot(&dashboard, width, height).is_ok());
    }
}
#[test]
fn terminal_controls_are_escaped_and_raw_server_errors_events_never_exported() {
    let mut data: serde_json::Value = serde_json::from_slice(&fixture()).unwrap();
    data["nodes"][0]["uid"] = "node\u{1b}[2J\n\u{202e}".into();
    data["nodes"][0]["tier"] = "pool\u{1b}[H\t\u{9b}".into();
    data["nodes"][0]["owner"] = "owner-never-exported\u{1b}]52;c;synthetic\u{7}".into();
    data["nodes"][0]["instance_id"] = "instance\u{1b}]52;c;synthetic\u{7}\r".into();
    data["tiers"][0][0] = "pool\u{1b}[H\t\u{9b}".into();
    data["mode"] = "mock\u{1b}[2J\u{202e}".into();
    data["events"] = serde_json::json!(["synthetic-sensitive-message"]);
    let mut bytes = serde_json::to_vec(&data).unwrap();
    bytes.push(b'\n');
    let snapshot = decode_status(&bytes).unwrap();
    let json = serde_json::to_string(&snapshot).unwrap();
    assert!(!json.contains("synthetic-sensitive-message"));
    assert!(!json.contains("owner-never-exported"));
    assert!(!json.contains('\u{7}'));
    assert!(!json.contains('\u{9b}'));
    assert!(!json.contains('\u{1b}'));
    assert!(!json.contains('\u{202e}'));
    let text = text_snapshot(
        &Dashboard {
            snapshot: Some(snapshot),
            ..Default::default()
        },
        150,
        20,
    )
    .unwrap();
    assert!(!text.contains('\u{1b}'));
    assert!(!text.contains('\u{202e}'));
    data["ok"] = false.into();
    data["error"] = "synthetic-sensitive-message".into();
    let mut bytes = serde_json::to_vec(&data).unwrap();
    bytes.push(b'\n');
    let error = decode_status(&bytes).err().unwrap();
    assert_eq!(error, ReadError::Rejected);
    assert!(!error.to_string().contains("sensitive"));
}
#[test]
fn non_tty_text_json_and_explicit_one_shot_do_not_enter_alternate_screen() {
    for flag in [None, Some("--json"), Some("--once")] {
        let dir = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let path = dir.path().canonicalize().unwrap().join("status.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let request = m8s_api::read_frame(&mut stream).unwrap();
            assert!(matches!(
                serde_json::from_slice::<m8s_api::Request>(&request).unwrap(),
                m8s_api::Request::Status {}
            ));
            stream.write_all(&fixture()).unwrap();
        });
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_m8s-top"));
        command
            .arg("--socket")
            .arg(path)
            .stdin(std::process::Stdio::null());
        if let Some(flag) = flag {
            command.arg(flag);
        }
        let output = command.output().unwrap();
        worker.join().unwrap();
        assert!(output.status.success());
        assert!(!output.stdout.contains(&0x1b));
        assert!(output.stderr.is_empty());
        if flag == Some("--json") {
            let snapshot: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert!(snapshot["nodes"][0]["last_observation"].is_null());
            assert!(snapshot["nodes"][2]["model_ready"].is_null());
            assert_eq!(snapshot["reported_mode"], "mock-only");
        } else {
            assert!(
                String::from_utf8(output.stdout)
                    .unwrap()
                    .contains("Read-only status")
            );
        }
    }
}

#[test]
fn exact_api_phases_and_empty_lists_do_not_prove_readiness_or_absence() {
    use m8s_core::Phase;
    for phase in [
        Phase::Pending,
        Phase::Searching,
        Phase::Launching,
        Phase::Booting,
        Phase::Bootstrapped,
        Phase::Loading,
        Phase::Ready,
        Phase::Draining,
        Phase::Destroying,
        Phase::Gone,
        Phase::Failed,
        Phase::Orphaned,
        Phase::Paused,
    ] {
        let mut raw: serde_json::Value = serde_json::from_slice(&fixture()).unwrap();
        raw["nodes"][0]["phase"] = serde_json::to_value(phase).unwrap();
        raw["secure_bootstrap"] = "synthetic verified claim is not evidence".into();
        let mut bytes = serde_json::to_vec(&raw).unwrap();
        bytes.push(b'\n');
        let snapshot = decode_status(&bytes).unwrap();
        assert_eq!(snapshot.nodes[0].reported_phase, phase);
        assert!(snapshot.nodes[0].model_ready.is_none());
        assert!(snapshot.bootstrap_verified.is_none());
        let text = text_snapshot(
            &Dashboard {
                snapshot: Some(snapshot),
                ..Default::default()
            },
            150,
            20,
        )
        .unwrap();
        if phase == Phase::Gone {
            assert!(text.contains("Gone reported only"));
        }
        if phase == Phase::Orphaned {
            assert!(text.contains("orphan / unknown"));
        }
    }
    let mut raw: serde_json::Value = serde_json::from_slice(&fixture()).unwrap();
    raw["nodes"] = serde_json::json!([]);
    raw["tiers"] = serde_json::json!([]);
    let mut bytes = serde_json::to_vec(&raw).unwrap();
    bytes.push(b'\n');
    let text = text_snapshot(
        &Dashboard {
            snapshot: Some(decode_status(&bytes).unwrap()),
            ..Default::default()
        },
        150,
        20,
    )
    .unwrap();
    assert!(text.contains("No nodes reported; provider absence not independently verified"));
}

#[test]
fn unsafe_socket_permissions_types_and_symlinks_are_refused_before_connect() {
    use std::os::unix::fs::{DirBuilderExt, symlink};
    for case in 0..6 {
        let dir = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let base = dir.path().canonicalize().unwrap();
        let real = base.join("real");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&real)
            .unwrap();
        let path = real.join("status.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let target = match case {
            0 => {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
                path.clone()
            }
            1 => {
                std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
                path.clone()
            }
            2 => {
                let link = real.join("linked.sock");
                symlink(&path, &link).unwrap();
                link
            }
            3 => {
                let link = base.join("linked-dir");
                symlink(&real, &link).unwrap();
                link.join("status.sock")
            }
            4 => {
                std::fs::remove_file(&path).unwrap();
                std::fs::write(&path, b"synthetic-not-a-socket").unwrap();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
                path.clone()
            }
            _ => real.join("..").join("real").join("status.sock"),
        };
        let result = StatusClient::new(Duration::from_millis(100))
            .unwrap()
            .status(&target);
        assert!(
            matches!(result, Err(ReadError::UnsafeSocket)),
            "unsafe fixture {case} must fail closed"
        );
        assert!(
            matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock),
            "unsafe path must not be connected"
        );
    }
    // A symlink farther up the path is refused even when the immediate socket
    // parent is a genuine private directory.
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let base = dir.path().canonicalize().unwrap();
    let real = base.join("real");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&real)
        .unwrap();
    let inner = real.join("inner");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&inner)
        .unwrap();
    let path = inner.join("status.sock");
    let listener = UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let link = base.join("linked-ancestor");
    symlink(&real, &link).unwrap();
    let result = StatusClient::new(Duration::from_millis(100))
        .unwrap()
        .status(&link.join("inner/status.sock"));
    assert!(matches!(result, Err(ReadError::UnsafeSocket)));
    assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
}

#[test]
fn non_sticky_writable_ancestor_is_not_a_private_socket_boundary() {
    use std::os::unix::fs::DirBuilderExt;
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let base = dir.path().canonicalize().unwrap();
    let writable = base.join("writable");
    std::fs::create_dir(&writable).unwrap();
    std::fs::set_permissions(&writable, std::fs::Permissions::from_mode(0o777)).unwrap();
    let private = writable.join("private");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&private)
        .unwrap();
    let path = private.join("status.sock");
    let listener = UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let result = StatusClient::new(Duration::from_millis(100))
        .unwrap()
        .status(&path);
    assert!(matches!(result, Err(ReadError::UnsafeSocket)));
    assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
}
