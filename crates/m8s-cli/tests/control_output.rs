#![cfg(unix)]
//! Legacy rendering against only an owned synthetic peer.
mod support;
use serde_json::{Value, json};
use std::{
    io::Write,
    os::unix::net::UnixListener,
    process::Output,
    time::{Duration, Instant},
};
use support::Client;
fn reply() -> Value {
    json!({"ok":true,"mode":"mock-only","nodes":[{"uid":"node-1","tier":"a","owner":"owner","instance_id":null,"phase":"Pending","created_at":1}],"tiers":[["a",1]],"events":["launch queued"],"error":null,"secure_bootstrap":"unavailable"})
}
fn serve(value: &Value, command: &[&str], json: bool, broken: bool) -> Output {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("control.sock");
    let listener = UnixListener::bind(&path).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut args = vec!["--socket".into(), path.into_os_string()];
    args.extend(command.iter().map(Into::into));
    if json {
        args.push("--json".into());
    }
    let child = Client::spawn(&args, broken);
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut stream = loop {
        match listener.accept() {
            Ok((s, _)) => break s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => panic!("own synthetic accept"),
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    };
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let request: Value =
        serde_json::from_slice(&m8s_api::read_frame(&mut stream).unwrap()).unwrap();
    let expected = match command {
        ["up", tier] => json!({"command":"up","tier":tier}),
        ["scale", tier, n] => {
            json!({"command":"scale","tier":tier,"replicas":n.parse::<u32>().unwrap()})
        }
        [name] => json!({"command":name.replace('-',"_")}),
        _ => panic!("test command"),
    };
    assert_eq!(request, expected);
    let mut bytes = serde_json::to_vec(value).unwrap();
    bytes.push(b'\n');
    assert!(bytes.len() <= m8s_api::MAX_FRAME, "own reply wire bound");
    stream.write_all(&bytes).unwrap();
    drop(stream);
    let result = child.output();
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    result
}
#[track_caller]
fn refused(out: Output, code: &str) {
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert_eq!(out.stderr, format!("m8s: {code}\n").as_bytes());
}
#[test]
fn clean_legacy_text_and_json_are_exact_and_requests_once() {
    let r = reply();
    for command in [
        vec!["status"],
        vec!["up", "a"],
        vec!["scale", "a", "1"],
        vec!["down"],
        vec!["shutdown"],
        vec!["reload-config"],
        vec!["logs"],
    ] {
        // Successful stop acknowledgements report zero desired; shutdown also
        // reports no retained node rows. Down itself need not mean absent nodes.
        let mut r = r.clone();
        if command == ["down"] || command == ["shutdown"] {
            r["tiers"] = json!([["a", 0]]);
        }
        if command == ["shutdown"] {
            r["nodes"] = json!([]);
        }
        let o = serve(&r, &command, true, false);
        assert!(o.status.success());
        assert!(o.stderr.is_empty());
        let typed: m8s_api::Response = serde_json::from_value(r.clone()).unwrap();
        assert_eq!(
            o.stdout,
            format!("{}\n", serde_json::to_string(&typed).unwrap()).as_bytes()
        );
        let o = serve(&r, &command, false, false);
        assert!(o.status.success());
        let mut expected = format!(
            "mode: mock-only · secure bootstrap: unavailable\na: desired {}\n",
            r["tiers"][0][1]
        );
        if command != ["shutdown"] {
            expected.push_str("a node-1 Pending unresolved\n");
        }
        if command == ["logs"] {
            expected.push_str("launch queued\n");
        }
        assert_eq!(o.stdout, expected.as_bytes());
    }
    let mut r = r;
    r["secure_bootstrap"] = json!("unavailable; nodes cannot become Ready");
    assert!(serve(&r, &["status"], true, false).status.success());
}
#[test]
fn free_controls_and_unicode_are_escaped_without_raw_terminal_data() {
    let mut r = reply();
    let hostile = "x\u{1b}\n\r\t\u{7f}é\u{202e}";
    r["nodes"][0]["uid"] = json!(hostile);
    r["nodes"][0]["owner"] = json!(hostile);
    r["nodes"][0]["instance_id"] = json!(hostile);
    r["events"] = json!([hostile]);
    for json in [true, false] {
        let o = serve(&r, &["logs"], json, false);
        assert!(o.status.success());
        assert!(!o.stdout.contains(&27));
        assert!(!o.stdout.contains(&127));
        let text = String::from_utf8(o.stdout).unwrap();
        assert!(text.contains("\\u007f\\u00e9\\u202e"));
        if json {
            assert!(text.is_ascii());
            let v: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(v["events"][0], hostile);
        } else {
            assert!(text.contains("x\\u001b\\u000a\\u000d\\u0009"));
        }
    }
}
#[test]
fn unsafe_claims_incoherence_duplicates_and_bounds_refuse_statically() {
    let base = reply();
    let mut cases = vec![];
    for (key, value) in [
        ("mode", json!("approved\u{1b}")),
        ("secure_bootstrap", json!("Ready")),
        ("error", json!("sensitive\u{202e}")),
        ("tiers", json!([["a", 1], ["a", 1]])),
        ("tiers", json!([["a", 17]])),
        ("tiers", json!([["unsafe\n", 1]])),
        ("events", json!(["x".repeat(4097)])),
        ("events", json!(vec!["x"; 201])),
    ] {
        let mut r = base.clone();
        r[key] = value;
        cases.push(r);
    }
    let mut r = base.clone();
    r["nodes"][0]["phase"] = json!("Ready");
    cases.push(r);
    let mut r = base.clone();
    r["nodes"] = json!([r["nodes"][0], r["nodes"][0]]);
    cases.push(r);
    let mut r = base.clone();
    r["nodes"][0]["uid"] = json!("x".repeat(257));
    cases.push(r);
    for r in cases {
        for json in [true, false] {
            refused(
                serve(&r, &["status"], json, false),
                "control-outcome-unknown",
            );
        }
    }
    let mut r = base;
    r["ok"] = json!(false);
    r["error"] = json!("sensitive\u{1b}");
    for json in [true, false] {
        refused(
            serve(&r, &["down"], json, false),
            "control-refused-or-outcome-unknown",
        );
    }
}
#[test]
fn expansion_cap_malformed_reply_and_broken_output_are_uncertain() {
    let mut r = reply();
    r["events"] = json!(vec!["é".repeat(1000); 20]);
    for json in [true, false] {
        refused(
            serve(&r, &["logs"], json, false),
            "control-output-unavailable-outcome-unknown",
        );
    }
    refused(
        serve(&json!({"secret":"private"}), &["shutdown"], true, false),
        "control-outcome-unknown",
    );
    refused(
        serve(&reply(), &["up", "a"], false, true),
        "control-output-unavailable-outcome-unknown",
    );
}

#[test]
fn manual_acknowledgements_must_match_requested_stored_intent() {
    for json in [true, false] {
        for command in [
            vec!["scale", "a", "2"],
            vec!["scale", "missing", "0"],
            vec!["up", "missing"],
            vec!["down"],
            vec!["shutdown"],
        ] {
            refused(
                serve(&reply(), &command, json, false),
                "control-outcome-unknown",
            );
        }
        let mut zero = reply();
        zero["tiers"] = json!([["a", 0]]);
        refused(
            serve(&zero, &["up", "a"], json, false),
            "control-outcome-unknown",
        );
        // A destroy acknowledgement/zero desired is not shutdown completion.
        refused(
            serve(&zero, &["shutdown"], json, false),
            "control-outcome-unknown",
        );
        assert!(serve(&zero, &["down"], json, false).status.success());
        assert!(
            serve(&zero, &["scale", "a", "0"], json, false)
                .status
                .success()
        );
        // Unmentioned tiers and retained nodes do not constrain positive scale.
        zero["tiers"] = json!([["other", 4], ["a", 2]]);
        assert!(
            serve(&zero, &["scale", "a", "2"], json, false)
                .status
                .success()
        );
        assert!(serve(&zero, &["up", "a"], json, false).status.success());
        // Empty stopped snapshots are legitimate, not invented missing intents.
        zero["tiers"] = json!([]);
        zero["nodes"] = json!([]);
        assert!(serve(&zero, &["down"], json, false).status.success());
        assert!(serve(&zero, &["shutdown"], json, false).status.success());
        refused(
            serve(&zero, &["scale", "a", "0"], json, false),
            "control-outcome-unknown",
        );
    }
}
