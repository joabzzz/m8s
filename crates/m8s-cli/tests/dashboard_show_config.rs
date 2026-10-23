#![cfg(unix)]
use m8s_cli::dashboard::{
    ConfigObservation,
    client::{ReadError, StatusClient, decode_status},
    view::{Dashboard, text_snapshot},
};
use std::{
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    time::{Duration, Instant},
};
fn status() -> Vec<u8> {
    frame(
        serde_json::json!({"ok":true,"mode":"mock-only","nodes":[],"tiers":[["tier",1]],"events":[],"error":null,"secure_bootstrap":"unavailable"}),
    )
}
fn config() -> Vec<u8> {
    frame(
        serde_json::json!({"ok":true,"mode":"mock-only","scope":"mock-control-admission-only","configured":true,"error":null,"tiers":[{"id":"tier","min":1,"max":2}]}),
    )
}
fn frame(v: serde_json::Value) -> Vec<u8> {
    let mut b = serde_json::to_vec(&v).unwrap();
    b.push(b'\n');
    b
}
fn observe(
    show: bool,
    timeout: Duration,
    status_delay: Duration,
    config_delay: Duration,
    reply: Option<Vec<u8>>,
    unsafe_second: bool,
) -> Result<m8s_cli::dashboard::StatusSnapshot, ReadError> {
    let dir = tempfile::Builder::new()
        .prefix("m8tc-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap();
    let path = dir.path().canonicalize().unwrap().join("status.sock");
    let listener = UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let server_path = path.clone();
    let worker = std::thread::spawn(move || {
        let count = if show && !unsafe_second && reply.is_some() && status_delay < timeout {
            2
        } else {
            1
        };
        for index in 0..count {
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => panic!("{e}"),
                };
                assert!(
                    Instant::now() < deadline,
                    "expected synthetic connection missing"
                );
                std::thread::sleep(Duration::from_millis(1));
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let request = m8s_api::read_frame(&mut stream).unwrap();
            let v: serde_json::Value = serde_json::from_slice(&request).unwrap();
            assert_eq!(
                v,
                serde_json::json!({"command":if index==0{"status"}else{"config_status"}})
            );
            if index == 0 {
                if unsafe_second {
                    std::fs::set_permissions(&server_path, std::fs::Permissions::from_mode(0o666))
                        .unwrap();
                }
                std::thread::sleep(status_delay);
                if stream.write_all(&status()).is_err() {
                    break;
                }
            } else {
                let reply = reply.as_ref().unwrap();
                // Dribble rather than reset the per-read timeout budget.
                for chunk in reply.chunks(reply.len().div_ceil(6)) {
                    std::thread::sleep(config_delay / 6);
                    if stream.write_all(chunk).is_err() {
                        break;
                    }
                }
            }
            stream.shutdown(std::net::Shutdown::Write).unwrap_or(());
            let mut byte = [0];
            assert_eq!(
                stream.read(&mut byte).unwrap(),
                0,
                "client must close each bounded observation"
            );
        }
        if !show || unsafe_second {
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
    });
    let client = StatusClient::new(timeout).unwrap();
    let result = if show {
        client.status_with_config(&path)
    } else {
        client.status(&path)
    };
    worker.join().unwrap();
    result
}
#[test]
fn default_is_one_request_optin_is_checked_and_failures_are_unknown_not_empty_policy() {
    let default = observe(
        false,
        Duration::from_secs(1),
        Duration::ZERO,
        Duration::ZERO,
        None,
        false,
    )
    .unwrap();
    assert!(default.config_observation.is_none());
    assert!(
        serde_json::to_value(default)
            .unwrap()
            .get("config_observation")
            .is_none()
    );
    let available = observe(
        true,
        Duration::from_secs(1),
        Duration::ZERO,
        Duration::ZERO,
        Some(config()),
        false,
    )
    .unwrap();
    assert!(matches!(
        available.config_observation,
        Some(ConfigObservation::Available {
            configured: true,
            atomic_with_status: false,
            ..
        })
    ));
    for reply in [
        b"{}\n".to_vec(),
        b"sensitive-invalid\n".to_vec(),
        frame(
            serde_json::json!({"ok":false,"mode":"mock-only","scope":"mock-control-admission-only","error":"sensitive","configured":false,"tiers":[]}),
        ),
        {
            let mut v: serde_json::Value = serde_json::from_slice(&config()).unwrap();
            v["tiers"][0]["max"] = 17.into();
            frame(v)
        },
    ] {
        let snapshot = observe(
            true,
            Duration::from_secs(1),
            Duration::ZERO,
            Duration::ZERO,
            Some(reply),
            false,
        )
        .unwrap();
        assert_eq!(snapshot.pools, vec![("tier".into(), 1)]);
        assert!(matches!(
            snapshot.config_observation,
            Some(ConfigObservation::Unknown { .. })
        ));
    }
    let unavailable = observe(
        true,
        Duration::from_secs(1),
        Duration::ZERO,
        Duration::ZERO,
        None,
        false,
    )
    .unwrap();
    assert!(matches!(
        unavailable.config_observation,
        Some(ConfigObservation::Unknown { .. })
    ));
    let unsupported = observe(
        true,
        Duration::from_secs(1),
        Duration::ZERO,
        Duration::ZERO,
        Some(status()),
        false,
    )
    .unwrap();
    assert!(matches!(
        unsupported.config_observation,
        Some(ConfigObservation::Unknown { .. })
    ));
    let failed_status = observe(
        true,
        Duration::from_millis(40),
        Duration::from_millis(100),
        Duration::ZERO,
        Some(config()),
        false,
    );
    assert!(matches!(failed_status, Err(ReadError::Timeout)));
    let unsafe_path = observe(
        true,
        Duration::from_secs(1),
        Duration::ZERO,
        Duration::ZERO,
        None,
        true,
    )
    .unwrap();
    assert!(matches!(
        unsafe_path.config_observation,
        Some(ConfigObservation::Unknown { .. })
    ));
}
#[test]
fn second_dribbling_observation_uses_original_deadline_with_positive_control() {
    let result = observe(
        true,
        Duration::from_millis(180),
        Duration::from_millis(100),
        Duration::from_millis(120),
        Some(config()),
        false,
    )
    .unwrap();
    assert!(matches!(
        result.config_observation,
        Some(ConfigObservation::Unknown { .. })
    ));
    let positive = observe(
        true,
        Duration::from_millis(600),
        Duration::from_millis(100),
        Duration::from_millis(120),
        Some(config()),
        false,
    )
    .unwrap();
    assert!(matches!(
        positive.config_observation,
        Some(ConfigObservation::Available { .. })
    ));
}
#[test]
fn rendering_available_legacy_unknown_stale_and_tiny_is_pure_and_bounded() {
    let mut snapshot = decode_status(&status()).unwrap();
    snapshot.config_observation = Some(ConfigObservation::Available {
        configured: true,
        tiers: vec![m8s_api::ConfigTierBounds {
            id: "tier\u{1b}[31m".into(),
            min: 1,
            max: 2,
        }],
        atomic_with_status: false,
    });
    let mut dashboard = Dashboard {
        snapshot: Some(snapshot.clone()),
        ..Default::default()
    };
    let text = text_snapshot(&dashboard, 150, 24).unwrap();
    assert!(
        text.contains("CONFIGURED")
            && text.contains("NOT atomic")
            && text.contains("min=1 max=2")
            && text.contains("desired=1")
    );
    assert!(!text.contains('\u{1b}'));
    dashboard.update(Err(ReadError::Timeout));
    let stale = text_snapshot(&dashboard, 150, 24).unwrap();
    assert!(stale.contains("STALE / UNKNOWN") && stale.contains("Config: UNKNOWN"));
    assert!(!stale.contains("min=1 max=2"));
    snapshot.config_observation = Some(ConfigObservation::Available {
        configured: false,
        tiers: vec![],
        atomic_with_status: false,
    });
    dashboard.update(Ok(snapshot));
    assert!(
        text_snapshot(&dashboard, 150, 24)
            .unwrap()
            .contains("UNCONFIGURED")
    );
    for (w, h) in [(1, 1), (80, 10), (100, 14), (300, 120)] {
        let text = text_snapshot(&dashboard, w, h).unwrap();
        assert!(text.len() <= usize::from(w) * usize::from(h) * 8 + usize::from(h));
    }
}

// No half-close/EOF wait: the fixture drops each peer immediately after write.
fn immediate_close(
    first: Vec<u8>,
    second: Option<Vec<u8>>,
) -> Result<m8s_cli::dashboard::StatusSnapshot, ReadError> {
    let dir = tempfile::Builder::new()
        .prefix("m8close-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap();
    let path = dir.path().canonicalize().unwrap().join("status.sock");
    let listener = UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let show = second.is_some();
    let worker = std::thread::spawn(move || {
        for (index, reply) in std::iter::once(first).chain(second).enumerate() {
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => panic!("fixture accept failed"),
                };
                assert!(Instant::now() < deadline, "expected connection missing");
                std::thread::sleep(Duration::from_millis(1));
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let request = m8s_api::read_frame(&mut stream).unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&request).unwrap(),
                serde_json::json!({"command":if index==0{"status"}else{"config_status"}})
            );
            let _ = stream.write_all(&reply);
            drop(stream); // genuine immediate full close, no client-EOF masking
        }
    });
    let client = StatusClient::new(Duration::from_secs(1)).unwrap();
    let result = if show {
        client.status_with_config(&path)
    } else {
        client.status(&path)
    };
    worker.join().unwrap();
    result
}
#[test]
fn complete_buffered_frames_survive_immediate_full_close_but_partial_eof_never_succeeds() {
    assert!(
        immediate_close(status(), None)
            .unwrap()
            .config_observation
            .is_none()
    );
    let mut large: serde_json::Value = serde_json::from_slice(&status()).unwrap();
    large["events"] = serde_json::json!(vec!["x".repeat(4096); 12]);
    let large = frame(large);
    assert!(large.len() < m8s_api::MAX_FRAME);
    assert!(immediate_close(large, None).is_ok()); // multiple buffered reads
    let snapshot = immediate_close(status(), Some(config())).unwrap();
    assert!(matches!(
        snapshot.config_observation,
        Some(ConfigObservation::Available {
            configured: true,
            ..
        })
    ));
    for reply in [
        b"{\"ok\":".to_vec(),
        b"{}\n".to_vec(),
        vec![b'x'; m8s_api::MAX_FRAME + 1],
    ] {
        assert!(matches!(
            immediate_close(reply, None),
            Err(ReadError::InvalidResponse)
        ));
    }
    let partial = immediate_close(status(), Some(b"{\"ok\":".to_vec())).unwrap();
    assert!(matches!(
        partial.config_observation,
        Some(ConfigObservation::Unknown { .. })
    ));
}
