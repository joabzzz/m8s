//! Ephemeral, certificate-enforcing native owner client. No provider access.
//! The signing key stays in the parent; only a node key and public certificate
//! reach the child. Raw native output (which can include invites) is discarded.
use mesh_llm_identity::{
    OwnerKeypair,
    exact_node_signing::{ExactNodeSigningRequest, ExactSigningPolicy, sign_node_ownership_exact},
};
use std::{
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::process::{Child, Command};
use zeroize::Zeroizing;
type Result<T> = std::result::Result<T, &'static str>;
const ERROR: &str = "prototype-owner-client-unavailable";
pub(super) fn utc() -> Result<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .ok_or(ERROR)
}
pub(super) fn private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| ERROR)?;
    m8s_secrets::check_no_extended_acl(&f).map_err(|_| ERROR)?;
    f.write_all(bytes)
        .and_then(|()| f.sync_all())
        .map_err(|_| ERROR)
}
pub(super) fn fresh_key() -> Result<iroh::SecretKey> {
    let mut bytes = Zeroizing::new([0u8; 32]);
    getrandom::fill(bytes.as_mut()).map_err(|_| ERROR)?;
    Ok(iroh::SecretKey::from_bytes(&bytes))
}
pub(super) struct OwnerClient {
    child: Child,
    root: PathBuf,
    node_id: String,
    owner_id: String,
    cert_id: String,
    root_identity: (u64, u64),
    pub port: u16,
    console: u16,
}
impl OwnerClient {
    pub fn start(
        program: &Path,
        root: &Path,
        owner: &OwnerKeypair,
        port: u16,
        console: u16,
        expires_at: i64,
    ) -> Result<Self> {
        let now = utc()?;
        if port == 0
            || console == 0
            || port == console
            || expires_at <= now
            || expires_at - now > 1800
        {
            return Err(ERROR);
        }
        let m = fs::symlink_metadata(program).map_err(|_| ERROR)?;
        if program.canonicalize().map_err(|_| ERROR)? != program
            || !m.is_file()
            || m.mode() & 0o7022 != 0
            || m.mode() & 0o111 == 0
        {
            return Err(ERROR);
        }
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root)
            .map_err(|_| ERROR)?;
        if root.canonicalize().map_err(|_| ERROR)? != root {
            return Err(ERROR);
        }
        m8s_secrets::check_no_extended_acl(&fs::File::open(root).map_err(|_| ERROR)?)
            .map_err(|_| ERROR)?;
        let meta = fs::symlink_metadata(root).map_err(|_| ERROR)?;
        let root_identity = (meta.dev(), meta.ino());
        let node = fresh_key()?;
        // Native /api/status exposes a short display ID. Authentication below
        // also requires its exact full-node certificate ID and verified owner.
        let node_id = node.public().fmt_short().to_string();
        let owner_id = owner.owner_id();
        let request = ExactNodeSigningRequest::new(
            *node.public().as_bytes(),
            now as u64 * 1000,
            expires_at as u64 * 1000,
        )
        .map_err(|_| ERROR)?;
        let policy = ExactSigningPolicy::new(1_800_000, 5000).map_err(|_| ERROR)?;
        let cert = sign_node_ownership_exact(owner, &request, &policy).map_err(|_| ERROR)?;
        let key_bytes = Zeroizing::new(node.to_bytes());
        let encoded = Zeroizing::new(hex::encode(key_bytes.as_slice()));
        private_file(&root.join("node-key"), encoded.as_bytes())?;
        private_file(
            &root.join("node-ownership.json"),
            &serde_json::to_vec(&cert).map_err(|_| ERROR)?,
        )?;
        private_file(&root.join("runtime.toml"), b"")?;
        let mut command = Command::new(program);
        command
            .arg("client")
            .arg("--node-ownership")
            .arg(root.join("node-ownership.json"))
            .arg("--node-ownership-required")
            .arg("--config")
            .arg(root.join("runtime.toml"))
            .args([
                "--trust-policy",
                "allowlist",
                "--trust-owner",
                &owner_id,
                "--headless",
                "--log-format",
                "json",
                "--no-default-plugins",
                "--port",
                &port.to_string(),
                "--console",
                &console.to_string(),
            ])
            .env_clear()
            .env("HOME", root)
            .env("MESH_LLM_NODE_KEY_PATH", root.join("node-key"))
            .env("MESH_LLM_ANALYTICS", "0")
            .env("DO_NOT_TRACK", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .kill_on_drop(true);
        let child = command.spawn().map_err(|_| ERROR)?;
        Ok(Self {
            child,
            root: root.to_owned(),
            node_id,
            owner_id,
            cert_id: cert.claim.cert_id,
            root_identity,
            port,
            console,
        })
    }
    pub async fn invite(&mut self) -> Result<Zeroizing<Vec<u8>>> {
        let deadline = Instant::now() + Duration::from_secs(45);
        let mut failure = "owner-status-http-unavailable";
        while Instant::now() < deadline {
            if self.child.try_wait().map_err(|_| ERROR)?.is_some() {
                return Err("owner-native-process-exited");
            }
            let port = self.console;
            let response = tokio::task::spawn_blocking(move || {
                super::http::status(port, Instant::now() + Duration::from_secs(1))
            })
            .await
            .map_err(|_| ERROR)?;
            if let Ok(bytes) = response {
                let bytes = Zeroizing::new(bytes);
                failure = "owner-status-schema-unavailable";
                if let Ok(status) = serde_json::from_slice::<Status>(&bytes) {
                    failure = if status.node_id != self.node_id {
                        "owner-node-id-mismatch"
                    } else if status.owner.owner_id.as_deref() != Some(&self.owner_id) {
                        "owner-signing-id-mismatch"
                    } else if !status.owner.verified || status.owner.status != "verified" {
                        "owner-certificate-unverified"
                    } else if !status.is_client || status.is_host || status.llama_ready {
                        "owner-client-role-mismatch"
                    } else {
                        "owner-invite-unavailable"
                    };
                    let token = Zeroizing::new(status.token);
                    if status.node_id == self.node_id
                        && status.owner.owner_id.as_deref() == Some(&self.owner_id)
                        && status.owner.cert_id.as_deref() == Some(&self.cert_id)
                        && status.owner.verified
                        && status.owner.status == "verified"
                        && status.is_client
                        && !status.is_host
                        && !status.llama_ready
                        && !token.is_empty()
                        && token.len() <= 16384
                    {
                        return Ok(Zeroizing::new(token.as_bytes().to_vec()));
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Err(failure)
    }
    pub async fn stop(&mut self) -> Result<()> {
        if self.child.try_wait().map_err(|_| ERROR)?.is_none() {
            if let Some(pid) = self.child.id().and_then(|v| i32::try_from(v).ok()) {
                unsafe {
                    libc::kill(-pid, libc::SIGTERM);
                }
            }
            if tokio::time::timeout(Duration::from_secs(3), self.child.wait())
                .await
                .is_err()
            {
                if let Some(pid) = self.child.id().and_then(|v| i32::try_from(v).ok()) {
                    unsafe {
                        libc::kill(-pid, libc::SIGKILL);
                    }
                }
                self.child.wait().await.map_err(|_| ERROR)?;
            }
        }
        // This directory was exclusively created by this object. Check its
        // retained identity before deleting native-created keys and runtime state.
        // remove_dir_all does not follow symlinks; this is unlink, not secure erase.
        let meta = fs::symlink_metadata(&self.root).map_err(|_| ERROR)?;
        if !meta.is_dir() || (meta.dev(), meta.ino()) != self.root_identity {
            return Err(ERROR);
        }
        fs::remove_dir_all(&self.root).map_err(|_| ERROR)?;
        Ok(())
    }
}
#[derive(serde::Deserialize)]
struct Status {
    node_id: String,
    owner: Ownership,
    token: String,
    is_client: bool,
    is_host: bool,
    llama_ready: bool,
}
#[derive(serde::Deserialize)]
struct Ownership {
    cert_id: Option<String>,
    owner_id: Option<String>,
    verified: bool,
    status: String,
}

pub(super) fn check(args: &[String]) -> Result<()> {
    if args.len() != 4 || args[0] != "--mesh" || args[2] != "--directory" {
        return Err("invalid-owner-check-arguments");
    }
    let program = PathBuf::from(&args[1]);
    let directory = PathBuf::from(&args[3]);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| ERROR)?;
    runtime.block_on(async {
        let owner = OwnerKeypair::generate();
        let a = std::net::TcpListener::bind("127.0.0.1:0").map_err(|_| ERROR)?;
        let b = std::net::TcpListener::bind("127.0.0.1:0").map_err(|_| ERROR)?;
        let port = a.local_addr().map_err(|_| ERROR)?.port();
        let console = b.local_addr().map_err(|_| ERROR)?.port();
        drop((a,b));
        let mut client = OwnerClient::start(&program, &directory, &owner, port, console, utc()?+1800)?;
        let receipt = client.invite().await;
        let cleanup = client.stop().await;
        receipt?;
        cleanup?;
        println!("{}", serde_json::json!({"schema":"m8s.owner-client-check/v1","certificate_verified":true,"invite_observed":true,"local_inference_backend":false,"client_stopped":true,"node_key_removed":true,"provider_calls":false,"api_port":client.port}));
        Ok(())
    })
}
