use super::*;
use serde_json::json;
fn config() -> Value {
    json!({"schema":"m8s.vast-node/v1","instance_id":42,"container_id":"42",
        "image_sha256":"a".repeat(64),"owner_endpoint":"b".repeat(64),
        "owner_signing_key":"c".repeat(64),"mesh_owner_id":"d".repeat(64),
        "owner_address":"127.0.0.1:12345","bind_address":"0.0.0.0:40100",
        "trust":{},"program":"/opt/mesh/mesh-llm",
        "gguf":"/models/qwen2.5-0.5b-instruct-q4_k_m.gguf",
        "material_root":"/var/lib/m8s/material","model":"qwen2.5-0.5b-instruct-q4_k_m",
        "heartbeat_ttl_seconds":30,"max_lifetime_seconds":600,"port":9337,"console_port":9338})
}
fn packet(config: &Value) -> Result<ProvisionPacket> {
    ProvisionPacket::new(
        config,
        SecretValue::new(vec![0x97; 32]),
        SecretValue::new(vec![0xe8; 32]),
    )
}
#[test]
fn packet_matches_receiver_format_without_json_secrets() {
    let config = config();
    let packet = packet(&config).unwrap();
    let frame = packet.frame.expose();
    assert_eq!(&frame[..MAGIC.len()], MAGIC);
    let size = u32::from_be_bytes(frame[MAGIC.len()..MAGIC.len() + 4].try_into().unwrap()) as usize;
    let start = MAGIC.len() + 4;
    assert_eq!(
        serde_json::from_slice::<Value>(&frame[start..start + size]).unwrap(),
        config
    );
    assert_eq!(&frame[start + size..start + size + 32], &[0x97; 32]);
    assert_eq!(&frame[start + size + 32..], "e8".repeat(32).as_bytes());
    assert_eq!(frame.len(), start + size + 96);
}
#[test]
fn malformed_public_config_and_secret_lengths_refuse() {
    for name in FIELDS {
        let mut value = config();
        value.as_object_mut().unwrap().remove(*name);
        assert!(packet(&value).is_err());
    }
    for (name, value) in [
        ("secret", json!("synthetic")),
        ("container_id", json!("43")),
        ("program", json!("/bin/sh")),
        ("image_sha256", json!("bad")),
        ("owner_endpoint", json!("G".repeat(64))),
    ] {
        let mut config = config();
        config[name] = value;
        assert!(packet(&config).is_err());
    }
    for size in [0, 31, 33] {
        assert!(
            ProvisionPacket::new(
                &config(),
                SecretValue::new(vec![1; size]),
                SecretValue::new(vec![2; 32])
            )
            .is_err()
        );
        assert!(
            ProvisionPacket::new(
                &config(),
                SecretValue::new(vec![1; 32]),
                SecretValue::new(vec![2; size])
            )
            .is_err()
        );
    }
    let mut large = config();
    large["trust"] = json!({"padding":"x".repeat(8192)});
    assert!(packet(&large).is_err());
}
#[test]
fn ssh_command_is_fixed_pinned_noninteractive_and_has_no_secret_arguments() {
    for bad in [
        "/tmp/key%h",
        "/tmp/key name",
        "/tmp/key\nother",
        "/tmp/~key",
    ] {
        assert!(!literal_path(Path::new(bad)));
    }
    let command = ssh_command(
        Path::new("/tmp/synthetic"),
        Path::new("/tmp/synthetic/key"),
        Path::new("/tmp/synthetic/known"),
        "8.8.8.8:12345".parse().unwrap(),
    );
    let command = command.as_std();
    assert_eq!(command.get_program(), "/usr/bin/ssh");
    let args: Vec<_> = command.get_args().map(|a| a.to_str().unwrap()).collect();
    assert!(args.windows(2).any(|a| a == ["-F", "/dev/null"]));
    for option in [
        "StrictHostKeyChecking=yes",
        "IdentityAgent=none",
        "BatchMode=yes",
        "IdentitiesOnly=yes",
        "ConnectionAttempts=1",
        "ControlMaster=no",
        "ClearAllForwardings=yes",
        "UserKnownHostsFile=/tmp/synthetic/known",
    ] {
        assert!(args.contains(&option));
    }
    assert_eq!(&args[args.len() - 2..], ["m8s@8.8.8.8", "provision-v1"]);
    let environment: Vec<_> = command
        .get_envs()
        .map(|(k, _)| k.to_str().unwrap())
        .collect();
    assert_eq!(environment, ["HOME", "PATH"]);
}

#[test]
fn flint_provisioning_requires_pinned_native_selection_without_a_local_file() {
    let mut value = config();
    value["model"] = json!(FLINT_MODEL_REF);
    assert!(packet(&value).is_err());
    value["gguf"] = Value::Null;
    assert!(packet(&value).is_ok());
    for model in [
        "flint-nvfp4",
        "RecursiveSI/flint-nvfp4-gguf@main/flint-nvfp4.gguf",
        "other/repo",
    ] {
        value["model"] = json!(model);
        assert!(packet(&value).is_err());
    }
}

#[test]
fn qwen38_requires_the_exact_download_pin_and_no_local_fixture() {
    let mut value = config();
    value["model"] = json!(QWEN38_MODEL_REF);
    assert!(packet(&value).is_err());
    value["gguf"] = Value::Null;
    assert!(packet(&value).is_ok());
    for model in [
        "Qwen/Qwen3.8-27B",
        "unsloth/Qwen3.8-27B-GGUF@main/Qwen3.8-27B-UD-Q4_K_M.gguf",
        "unsloth/Qwen3.5-27B-GGUF@4ca720788d1e01f1bff70c033e0d0028fd02e502/Qwen3.5-27B-UD-Q4_K_M.gguf",
    ] {
        value["model"] = json!(model);
        assert!(packet(&value).is_err());
    }
}

#[test]
fn native_diagnostics_accept_only_exact_closed_labels() {
    assert_eq!(
        native_label(b"m8s native diagnostic: native-download-error"),
        Some("native-download-error")
    );
    for input in [
        b"m8s native diagnostic: private-value".as_slice(),
        b"m8s native diagnostic: native-download-error private-value",
        b"ssh error private-value",
    ] {
        assert_eq!(native_label(input), None);
    }
}
