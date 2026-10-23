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
