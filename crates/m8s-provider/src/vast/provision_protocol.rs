//! Secret-free progress on the already host-key-pinned SSH channel. These
//! records never grant bootstrap admission, certificate validity, or Ready.
pub const CONFIG: &[u8] = b"M8S_NODE_CONFIG_V1\n";
pub const IDENTITY: &[u8] = b"M8S_NODE_IDENTITY_V1\n";
pub const PROVIDER: &[u8] = b"M8S_NODE_PROVIDER_V1\n";
pub const LISTENING: &[u8] = b"M8S_NODE_LISTENING_V1\n";
pub const STAGES: &[(&[u8], &str)] = &[
    (CONFIG, "prototype-provision-config-unconfirmed"),
    (IDENTITY, "prototype-provision-identity-unconfirmed"),
    (
        PROVIDER,
        "prototype-provision-instance-credential-unconfirmed",
    ),
    (LISTENING, "prototype-provision-listener-unconfirmed"),
];

/// Immutable upstream model selection resolved by MeshLLM on the rented node.
pub const FLINT_MODEL_REF: &str =
    "RecursiveSI/flint-nvfp4-gguf@6ec43e19b565fc8d560e2d04985d7724f20673bf/flint-nvfp4.gguf";

/// Exact Qwen3.8 27B text-only demo quantization; weights download on the node.
pub const QWEN38_MODEL_REF: &str =
    "unsloth/Qwen3.8-27B-GGUF@4ca720788d1e01f1bff70c033e0d0028fd02e502/Qwen3.8-27B-UD-Q4_K_M.gguf";

/// Pinned size and SHA-256 of a model file MeshLLM resolves at node start. Must
/// equal `deploy/container/*-model-artifact.json` (checked by a unit test).
pub fn pinned_model(model_ref: &str) -> Option<(u64, [u8; 32])> {
    let (bytes, hex) = match model_ref {
        FLINT_MODEL_REF => (
            22_863_084_160,
            "97cc8009ff9b132d5cf3c488e6767962bb5a5dc158a90ed661bbbddf876b8bde",
        ),
        QWEN38_MODEL_REF => (
            16_464_440_224,
            "322e194ff79741c7baa497c240f677f54b201b0efab44ca8e50f122b39123482",
        ),
        _ => return None,
    };
    let mut digest = [0u8; 32];
    for (i, pair) in hex.as_bytes().chunks(2).enumerate() {
        digest[i] = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some((bytes, digest))
}

#[cfg(test)]
mod pin_tests {
    use super::*;

    #[test]
    fn pins_match_checked_in_artifact_contracts() {
        for (reference, contract) in [
            (
                FLINT_MODEL_REF,
                include_str!("../../../../deploy/container/flint-model-artifact.json"),
            ),
            (
                QWEN38_MODEL_REF,
                include_str!("../../../../deploy/container/qwen38-model-artifact.json"),
            ),
        ] {
            let value: serde_json::Value = serde_json::from_str(contract).unwrap();
            let (bytes, digest) = pinned_model(reference).unwrap();
            assert_eq!(value["bytes"].as_u64(), Some(bytes));
            let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(value["sha256"].as_str(), Some(hex.as_str()));
        }
        assert!(pinned_model("org/repo@main/file.gguf").is_none());
    }
}
