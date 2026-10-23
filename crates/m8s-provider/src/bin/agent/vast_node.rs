//! Explicit node entrypoint. Owner signing keys and account credentials are never
//! loaded here. The launching owner must already hold durable cleanup custody.
use m8s_agent::{
    Effect, Outcome,
    node::{
        MeshLaunch,
        runtime::{RuntimePlan, accept_owner_and_run},
    },
    supervisor::{TeardownDriver, worker::WorkerTeardown},
};
use m8s_bootstrap::{
    BOOT_ALPN, BootNonce, CONTROL_ALPN, EndpointId, Redemption,
    mp001::{Mp001CertificateVerifier, Mp001TrustPin},
};
use m8s_provider::vast::readonly::destroy::ScopedDestroyHttps;
use m8s_secrets::SecretValue;
use serde::Deserialize;
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::SocketAddr,
    num::NonZeroU64,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

type Result<T> = std::result::Result<T, &'static str>;
const REFUSED: &str = "secure Vast node refused; owner cleanup remains required";

// Public launch policy only. Nonce, node key and provider key are separate inputs.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    schema: String,
    instance_id: NonZeroU64,
    container_id: String,
    image_sha256: String,
    owner_endpoint: String,
    owner_signing_key: String,
    mesh_owner_id: String,
    owner_address: SocketAddr,
    bind_address: SocketAddr,
    trust: mesh_llm_identity::TrustStore,
    program: PathBuf,
    gguf: Option<PathBuf>,
    material_root: PathBuf,
    model: String,
    heartbeat_ttl_seconds: u64,
    max_lifetime_seconds: i64,
    port: u16,
    console_port: u16,
}
impl Config {
    fn valid_model_source(&self) -> bool {
        match &self.gguf {
            Some(path) => {
                path.is_absolute()
                    && path.extension().and_then(|v| v.to_str()) == Some("gguf")
                    && path.file_stem().and_then(|v| v.to_str()) == Some(self.model.as_str())
                    && !self.model.contains("-of-")
                    && self
                        .model
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            }
            None => [
                m8s_provider::vast::provision_protocol::FLINT_MODEL_REF,
                m8s_provider::vast::provision_protocol::QWEN38_MODEL_REF,
            ]
            .contains(&self.model.as_str()),
        }
    }

    fn validate(&self) -> Result<()> {
        if self.schema != "m8s.vast-node/v1"
            || self.container_id != self.instance_id.to_string()
            || self.container_id.is_empty()
            || self.container_id.len() > 128
            || !self
                .container_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
            || self.owner_address.port() == 0
            || self.owner_address.ip().is_unspecified()
            || self.owner_address.ip().is_multicast()
            || self.bind_address.port() == 0
            || self.bind_address.ip().is_multicast()
            || self.max_lifetime_seconds <= 0
            || self.max_lifetime_seconds > 1800
            || self.heartbeat_ttl_seconds == 0
            || self.heartbeat_ttl_seconds > self.max_lifetime_seconds as u64
            || self.port == 0
            || self.console_port == 0
            || self.port == self.console_port
            || !self.valid_model_source()
            || self.model.is_empty()
            || self.model.len() > 128
            || !self.program.is_absolute()
            || !self.material_root.is_absolute()
        {
            return Err(REFUSED);
        }
        for value in [
            &self.image_sha256,
            &self.owner_endpoint,
            &self.owner_signing_key,
            &self.mesh_owner_id,
        ] {
            decode32(value.as_bytes())?;
        }
        Ok(())
    }
}
fn decode32(value: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
    if value.len() != 64 {
        return Err(REFUSED);
    }
    let mut output = Zeroizing::new([0; 32]);
    let nibble = |b| match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        _ => Err(REFUSED),
    };
    for (i, pair) in value.as_chunks::<2>().0.iter().enumerate() {
        output[i] = nibble(pair[0])? * 16 + nibble(pair[1])?;
    }
    Ok(output)
}
fn private_input(path: &Path, limit: usize) -> Result<SecretValue> {
    if !path.is_absolute() || path.canonicalize().map_err(|_| REFUSED)? != path {
        return Err(REFUSED);
    }
    let parent = path.parent().ok_or(REFUSED)?;
    let parent_meta = fs::symlink_metadata(parent).map_err(|_| REFUSED)?;
    if !parent_meta.is_dir()
        || parent_meta.uid() != nix::unistd::Uid::effective().as_raw()
        || parent_meta.mode() & 0o7777 != 0o700
    {
        return Err(REFUSED);
    }
    let parent_file = fs::File::open(parent).map_err(|_| REFUSED)?;
    m8s_secrets::check_no_extended_acl(&parent_file).map_err(|_| REFUSED)?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| REFUSED)?;
    let before = file.metadata().map_err(|_| REFUSED)?;
    if !before.is_file()
        || before.uid() != nix::unistd::Uid::effective().as_raw()
        || before.mode() & 0o7777 != 0o600
        || before.nlink() != 1
        || before.len() > limit as u64
    {
        return Err(REFUSED);
    }
    m8s_secrets::check_no_extended_acl(&file).map_err(|_| REFUSED)?;
    let mut bytes = Zeroizing::new(Vec::new());
    (&mut file)
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| REFUSED)?;
    let after = file.metadata().map_err(|_| REFUSED)?;
    let current = fs::symlink_metadata(path).map_err(|_| REFUSED)?;
    let identity = |m: &fs::Metadata| {
        (
            m.dev(),
            m.ino(),
            m.mode(),
            m.uid(),
            m.nlink(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
        )
    };
    if bytes.len() > limit
        || identity(&before) != identity(&after)
        || identity(&before) != identity(&current)
    {
        return Err(REFUSED);
    }
    Ok(SecretValue::new(std::mem::take(&mut *bytes)))
}
fn clock() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(i64::MAX)
}

fn announce(stage: &'static [u8]) -> Result<()> {
    let mut output = std::io::stdout().lock();
    output.write_all(stage).map_err(|_| REFUSED)?;
    output.flush().map_err(|_| REFUSED)
}

pub(super) fn run() -> Result<()> {
    let paths: Vec<_> = std::env::args_os().skip(2).collect();
    if paths.len() != 3 {
        return Err("usage: m8s-agent --vast-node CONFIG NONCE_FILE NODE_KEY_FILE");
    }
    let mut config: Config =
        serde_json::from_slice(private_input(Path::new(&paths[0]), 65536)?.expose())
            .map_err(|_| REFUSED)?;
    config.validate()?;
    announce(m8s_provider::vast::provision_protocol::CONFIG)?;
    let supplied_id = std::env::var("CONTAINER_ID").map_err(|_| REFUSED)?;
    if supplied_id != config.instance_id.to_string() {
        return Err(REFUSED);
    }
    let nonce = private_input(Path::new(&paths[1]), 32)?;
    let nonce_bytes = Zeroizing::new(<[u8; 32]>::try_from(nonce.expose()).map_err(|_| REFUSED)?);
    let node_key_path = PathBuf::from(&paths[2]);
    let encoded_key = private_input(&node_key_path, 65)?;
    let encoded_key = std::str::from_utf8(encoded_key.expose())
        .map_err(|_| REFUSED)?
        .trim();
    let node_key = decode32(encoded_key.as_bytes())?;
    let secret_key = iroh::SecretKey::from_bytes(&node_key);
    let node = EndpointId(*secret_key.public().as_bytes());
    let owner = EndpointId(*decode32(config.owner_endpoint.as_bytes())?);
    let pin = Mp001TrustPin::new(owner, *decode32(config.owner_signing_key.as_bytes())?)
        .map_err(|_| REFUSED)?;
    let verifier = Mp001CertificateVerifier::new(pin, std::mem::take(&mut config.trust))
        .map_err(|_| REFUSED)?;
    announce(m8s_provider::vast::provision_protocol::IDENTITY)?;
    // Only the provider-injected per-instance credential. Never VAST_API_KEY.
    let credential = SecretValue::new(
        std::env::var("CONTAINER_API_KEY")
            .map_err(|_| REFUSED)?
            .into_bytes(),
    );
    let mut provider = ScopedDestroyHttps::new(config.instance_id, move || {
        Ok(SecretValue::new(credential.expose().to_vec()))
    })
    .map_err(|_| REFUSED)?;
    // A valid exact-ID observation is required before asserting credential
    // presence. This is not a cryptographic proof of delete permission.
    if provider.confirm_absent().map_err(|_| REFUSED)?.is_some() {
        return Err(REFUSED);
    }
    announce(m8s_provider::vast::provision_protocol::PROVIDER)?;
    let mut driver = WorkerTeardown::spawn(move |effect| match effect {
        Effect::Drain => Outcome::Success, // Supervisor separately reaps the child.
        Effect::Wipe => Outcome::Failure, // Ephemeral filesystem removal needs provider destruction.
        Effect::Destroy => {
            if provider.destroy().is_ok() {
                Outcome::Success
            } else {
                Outcome::Failure
            }
        }
        Effect::ConfirmAbsent => match provider.confirm_absent() {
            Ok(Some(_)) => Outcome::ConfirmedAbsent,
            _ => Outcome::Failure, // Revoked instance key cannot certify owner-visible absence.
        },
    })
    .map_err(|_| REFUSED)?;
    let result = run_supervised(
        config,
        node_key_path,
        secret_key,
        node,
        owner,
        verifier,
        BootNonce::new(*nonce_bytes),
        &mut driver,
    );
    // Keep this process alive for bounded queued cleanup even after bind/start
    // failure. Independent owner cleanup must survive this process's termination.
    let deadline = Instant::now() + Duration::from_secs(25);
    let mut acknowledged = false;
    while Instant::now() < deadline {
        if !acknowledged {
            acknowledged = driver.perform(Effect::Destroy) == Outcome::Success;
        }
        if driver.perform(Effect::ConfirmAbsent) == Outcome::ConfirmedAbsent {
            return result;
        }
        std::thread::sleep(Duration::from_secs(3));
    }
    Err(REFUSED)
}

#[allow(clippy::too_many_arguments)]
fn run_supervised(
    config: Config,
    node_key: PathBuf,
    secret_key: iroh::SecretKey,
    node: EndpointId,
    owner: EndpointId,
    verifier: Mp001CertificateVerifier,
    nonce: BootNonce,
    driver: &mut WorkerTeardown,
) -> Result<()> {
    let signals = super::termination::ShutdownSignals::install()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| REFUSED)?;
    runtime.block_on(async {
        if signals.requested() {
            return Err(REFUSED);
        }
        // Dedicated direct control endpoint. No relay registration competing with
        // the native mesh endpoint using the same certified node identity.
        let binding = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .secret_key(secret_key)
            .clear_ip_transports()
            .bind_addr(config.bind_address)
            .map_err(|_| REFUSED)?
            .clear_relay_transports()
            .clear_address_lookup()
            .alpns(vec![BOOT_ALPN.to_vec(), CONTROL_ALPN.to_vec()])
            .bind();
        let endpoint = tokio::time::timeout(Duration::from_secs(10), binding)
            .await
            .map_err(|_| REFUSED)?
            .map_err(|_| REFUSED)?;
        announce(m8s_provider::vast::provision_protocol::LISTENING)?;
        let owner_address =
            iroh::EndpointAddr::new(iroh::PublicKey::from_bytes(&owner.0).map_err(|_| REFUSED)?)
                .with_ip_addr(config.owner_address);
        let request = Redemption {
            nonce,
            instance_id: config.instance_id.to_string(),
            container_id: config.container_id,
            node,
            image_sha256: *decode32(config.image_sha256.as_bytes())?,
            self_destruct_present: true,
        };
        // Null-gguf nodes must have a pinned size/digest; unknown refs never serve.
        let model_pin = match config.gguf {
            Some(_) => None,
            None => Some(
                m8s_provider::vast::provision_protocol::pinned_model(&config.model)
                    .map(|(bytes, sha256)| m8s_agent::node::model::ModelPin { bytes, sha256 })
                    .ok_or(REFUSED)?,
            ),
        };
        let plan = RuntimePlan {
            material_root: &config.material_root,
            launch: MeshLaunch {
                program: config.program,
                gguf: config.gguf,
                node_key,
                owner,
                node,
                mesh_owner_id: config.mesh_owner_id,
                model: config.model,
                heartbeat_ttl_seconds: config.heartbeat_ttl_seconds,
                port: config.port,
                console_port: config.console_port,
                model_pin,
            },
            verifier: &verifier,
            max_lifetime_seconds: config.max_lifetime_seconds,
            native_id_timeout: Duration::from_secs(10),
            cleanup_timeout: Duration::from_secs(30),
        };
        let result = accept_owner_and_run(
            &endpoint,
            owner_address,
            request,
            plan,
            driver,
            async {
                while !signals.requested() {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            },
            clock,
        )
        .await
        .map_err(|error| {
            let label = match error {
                m8s_agent::node::Error::Policy => "node-policy-error",
                m8s_agent::node::Error::NativeId => "node-id-error",
                m8s_agent::node::Error::Material => "node-material-error",
                m8s_agent::node::Error::Certificate => "node-certificate-error",
            };
            eprintln!("m8s native diagnostic: {label}");
            REFUSED
        });
        let _ = tokio::time::timeout(Duration::from_secs(5), endpoint.close()).await;
        result.map(|_| ())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, os::unix::fs::DirBuilderExt};

    #[test]
    fn private_inputs_reject_aliases_permissions_and_oversize() {
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "m8s-node-input-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        let path = root.join("synthetic-input");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        file.write_all(b"synthetic-only").unwrap();
        drop(file);
        assert_eq!(
            private_input(&path, 64).unwrap().expose(),
            b"synthetic-only"
        );
        assert!(private_input(&path, 1).is_err());
        let link = root.join("symlink");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(private_input(&link, 64).is_err());
        let hardlink = root.join("hardlink");
        fs::hard_link(&path, &hardlink).unwrap();
        assert!(private_input(&path, 64).is_err());
        fs::remove_file(hardlink).unwrap();
        let public = root.join("public-input");
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&public)
            .unwrap();
        assert!(private_input(&public, 64).is_err());
        assert!(private_input(&root, 64).is_err());
    }
    fn valid() -> Config {
        Config {
            schema: "m8s.vast-node/v1".into(),
            instance_id: NonZeroU64::new(42).unwrap(),
            container_id: "42".into(),
            image_sha256: "11".repeat(32),
            owner_endpoint: "22".repeat(32),
            owner_signing_key: "33".repeat(32),
            mesh_owner_id: "44".repeat(32),
            owner_address: "192.0.2.1:1234".parse().unwrap(),
            bind_address: "0.0.0.0:7000".parse().unwrap(),
            trust: Default::default(),
            program: "/opt/mesh/mesh-llm".into(),
            gguf: Some("/models/qwen.gguf".into()),
            material_root: "/run/m8s/material".into(),
            model: "qwen".into(),
            heartbeat_ttl_seconds: 30,
            max_lifetime_seconds: 600,
            port: 8080,
            console_port: 8081,
        }
    }
    #[test]
    fn trusted_config_limits_are_checked_without_io() {
        assert!(valid().validate().is_ok());
        for mutate in [
            (|c: &mut Config| c.max_lifetime_seconds = 1801) as fn(&mut Config),
            |c| c.heartbeat_ttl_seconds = 601,
            |c| c.owner_address = "0.0.0.0:1234".parse().unwrap(),
            |c| c.owner_address = "192.0.2.1:0".parse().unwrap(),
            |c| c.owner_endpoint = "XX".repeat(32),
            |c| c.program = "mesh-llm".into(),
            |c| c.port = c.console_port,
            |c| c.container_id = "43".into(),
            |c| c.model = "other-model".into(),
            |c| c.schema = "unknown".into(),
        ] {
            let mut config = valid();
            mutate(&mut config);
            assert!(config.validate().is_err());
        }
    }
    #[test]
    fn key_encoding_is_exact_and_error_is_static() {
        assert_eq!(*decode32("01".repeat(32).as_bytes()).unwrap(), [1; 32]);
        for encoded in ["", "00", &"GG".repeat(32), &"01".repeat(33)] {
            assert_eq!(decode32(encoded.as_bytes()).err(), Some(REFUSED));
        }
    }
    #[test]
    fn config_refuses_secret_or_unknown_fields() {
        let value = serde_json::json!({
            "schema":"m8s.vast-node/v1", "instance_id":42, "container_id":"42",
            "image_sha256":"11".repeat(32), "owner_endpoint":"22".repeat(32),
            "owner_signing_key":"33".repeat(32), "mesh_owner_id":"44".repeat(32),
            "owner_address":"192.0.2.1:1234", "bind_address":"0.0.0.0:7000",
            "trust":mesh_llm_identity::TrustStore::default(), "program":"/opt/mesh/mesh-llm",
            "gguf":"/models/qwen.gguf", "material_root":"/run/m8s/material", "model":"qwen",
            "heartbeat_ttl_seconds":30, "max_lifetime_seconds":600, "port":8080, "console_port":8081
        });
        assert!(serde_json::from_value::<Config>(value.clone()).is_ok());
        for field in [
            "api_key",
            "owner_private_key",
            "nonce",
            "disable_tls",
            "skip_verification",
        ] {
            let mut bad = value.clone();
            bad[field] = serde_json::json!("synthetic-only");
            assert!(serde_json::from_value::<Config>(bad).is_err());
        }
    }
}
