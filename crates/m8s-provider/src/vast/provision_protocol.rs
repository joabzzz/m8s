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
