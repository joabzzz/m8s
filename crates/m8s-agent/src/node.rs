//! Sealed bootstrap material -> enforcing mesh argv, not admission or Ready proof.
//! This library seam is unwired to the agent binary. The trusted coordinator must
//! obtain a real TLS bundle, bind its actual local endpoint/key and current policy,
//! and retain control/DMS plus scoped destruction. No owner key is loaded here.
use crate::supervisor::ChildSpec;
use m8s_bootstrap::mp001::Mp001CertificateVerifier;
use m8s_bootstrap::transport::ReceivedBundle;
use m8s_bootstrap::{
    CertificateClaims, CertificateVerifier, ControlAcl, EndpointId, NodeCertificate,
};
use std::{
    fs::{self, File},
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("node launch policy refused")]
    Policy,
    #[error("node certificate refused")]
    Certificate,
    #[error("node material unavailable; partial private artifacts may remain")]
    Material,
}
type Result<T> = std::result::Result<T, Error>;
/// Explicit trusted launch inputs; never reconstruct from remote claims.
/// Paths must be explicit canonical files; node_key is the ALREADY-created fresh
/// node key used by the bootstrap endpoint, not an owner key. Its canonical format
/// and endpoint match remain native mesh responsibilities, not inferred from mode.
pub struct MeshLaunch {
    pub program: PathBuf,
    pub gguf: PathBuf,
    pub node_key: PathBuf,
    pub owner: EndpointId,
    pub node: EndpointId,
    pub mesh_owner_id: String,
    pub model: String,
    pub heartbeat_ttl_seconds: u64,
    pub port: u16,
    pub console_port: u16,
}
#[derive(Clone, Copy)]
enum Kind {
    Ancestor,
    Directory,
    Private,
    Binary,
    Model,
}
struct Anchor {
    path: PathBuf,
    file: File,
    kind: Kind,
    uid: u32,
    dev: u64,
    ino: u64,
    mode: u32,
    len: u64,
}
impl Anchor {
    fn open(path: &Path, kind: Kind) -> Result<Self> {
        if !path.is_absolute()
            || path
                .canonicalize()
                .map_err(|_| Error::Material)?
                .as_os_str()
                != path.as_os_str()
        {
            return Err(Error::Material);
        }
        let mut options = fs::OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        if matches!(kind, Kind::Directory | Kind::Ancestor) {
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_DIRECTORY);
        }
        let file = options.open(path).map_err(|_| Error::Material)?;
        Self::capture(path, file, kind)
    }
    fn capture(path: &Path, file: File, kind: Kind) -> Result<Self> {
        let m = file.metadata().map_err(|_| Error::Material)?;
        let anchor = Self {
            path: path.into(),
            file,
            kind,
            uid: m.uid(),
            dev: m.dev(),
            ino: m.ino(),
            mode: m.mode(),
            len: m.len(),
        };
        anchor.check()?;
        Ok(anchor)
    }
    fn check(&self) -> Result<()> {
        m8s_secrets::check_no_extended_acl(&self.file).map_err(|_| Error::Material)?;
        let m = self.file.metadata().map_err(|_| Error::Material)?;
        let p = fs::symlink_metadata(&self.path).map_err(|_| Error::Material)?;
        let uid = nix::unistd::Uid::effective().as_raw();
        if m.dev() != self.dev
            || m.ino() != self.ino
            || m.uid() != self.uid
            || m.mode() != self.mode
            || p.dev() != self.dev
            || p.ino() != self.ino
            || p.uid() != self.uid
            || p.mode() != self.mode
            || p.file_type().is_symlink()
        {
            return Err(Error::Material);
        }
        match self.kind {
            Kind::Ancestor => {
                if !m.is_dir()
                    || !p.is_dir()
                    || (self.uid != uid && self.uid != 0)
                    || (m.mode() & 0o022 != 0 && !(self.uid == 0 && m.mode() & 0o1000 != 0))
                {
                    return Err(Error::Material);
                }
            }
            Kind::Directory => {
                if !m.is_dir() || !p.is_dir() || self.uid != uid || m.mode() & 0o7777 != 0o700 {
                    return Err(Error::Material);
                }
            }
            _ => {
                if !m.is_file()
                    || !p.is_file()
                    || m.nlink() != 1
                    || p.nlink() != 1
                    || m.len() != self.len
                    || p.len() != self.len
                {
                    return Err(Error::Material);
                }
                match self.kind {
                    Kind::Private => {
                        if self.uid != uid || m.mode() & 0o7777 != 0o600 || self.len > 512 * 1024 {
                            return Err(Error::Material);
                        }
                    }
                    Kind::Binary | Kind::Model => {
                        if (self.uid != uid && self.uid != 0)
                            || m.mode() & 0o7022 != 0
                            || self.len == 0
                            || (matches!(self.kind, Kind::Binary) && m.mode() & 0o111 == 0)
                        {
                            return Err(Error::Material);
                        }
                    }
                    Kind::Directory | Kind::Ancestor => unreachable!(),
                }
            }
        }
        m8s_secrets::check_no_extended_acl(&self.file).map_err(|_| Error::Material)
    }
}
/// Retained files and exact public certificate binding, NOT a BootGate/permit.
/// No Debug/Serialize/Clone; never exposes join/config bytes. Drop closes anchors
/// but intentionally does not delete files: partial failure is not rollback or wipe.
pub struct NodeMaterial {
    launch: MeshLaunch,
    root: PathBuf,
    anchors: Vec<Anchor>,
    claims: CertificateClaims,
    certificate: NodeCertificate,
    acl: ControlAcl,
    config_empty: bool,
}
impl NodeMaterial {
    pub fn stage(
        root: &Path,
        received: ReceivedBundle,
        launch: MeshLaunch,
        verifier: &Mp001CertificateVerifier,
        now: i64,
    ) -> Result<Self> {
        Self::stage_with(root, received, launch, verifier, now)
    }
    // Production always supplies the concrete canonical verifier above. Unit
    // fixtures exercise this same file engine without minting current certs.
    fn stage_with(
        root: &Path,
        received: ReceivedBundle,
        launch: MeshLaunch,
        verifier: &impl CertificateVerifier,
        now: i64,
    ) -> Result<Self> {
        let claims = &received.claims;
        if now < 0
            || claims.owner != launch.owner
            || claims.node != launch.node
            || claims.issued_at < 0
            || claims.expires_at <= claims.issued_at
            || now < claims.issued_at
            || now >= claims.expires_at
            || launch.port == 0
            || launch.console_port == 0
            || launch.port == launch.console_port
            || launch.heartbeat_ttl_seconds == 0
            || launch.heartbeat_ttl_seconds > i64::MAX as u64
            || received.settings.heartbeat_ttl_seconds != launch.heartbeat_ttl_seconds
            || received.settings.model != launch.model
            || launch.model.is_empty()
            || launch.model.len() > 256
            || launch.model.contains('\0')
            || launch.mesh_owner_id.len() != 64
            || !launch
                .mesh_owner_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || received.join_token.is_empty()
            || received.join_token.len() > 128 * 1024
            || received.join_token.contains(&0)
            || std::str::from_utf8(&received.join_token).is_err()
            || received.certificate.0.is_empty()
            || received.certificate.0.len() > 128 * 1024
            || received.settings.config.len() > 512 * 1024
            || std::str::from_utf8(&received.settings.config).is_err()
        {
            return Err(Error::Policy);
        }
        verifier
            .verify(&received.certificate, claims)
            .map_err(|_| Error::Certificate)?;
        let parent = root.parent().ok_or(Error::Material)?;
        let name = root.file_name().ok_or(Error::Material)?;
        if !root.is_absolute() || name.is_empty() || name.len() > 128 || parent.join(name) != root {
            return Err(Error::Material);
        }
        let mut ancestors = Vec::new();
        for selected in [
            root,
            launch.program.as_path(),
            launch.gguf.as_path(),
            launch.node_key.as_path(),
        ] {
            for path in selected.ancestors().skip(1) {
                ancestors.push(Anchor::open(path, Kind::Ancestor)?);
            }
        }
        let mut anchors = vec![
            Anchor::open(parent, Kind::Directory)?,
            Anchor::open(&launch.program, Kind::Binary)?,
            Anchor::open(&launch.gguf, Kind::Model)?,
            Anchor::open(&launch.node_key, Kind::Private)?,
        ];
        if anchors[3].len == 0 || anchors[3].len > 4096 {
            return Err(Error::Material);
        }
        anchors.push(Anchor::open(
            launch.node_key.parent().ok_or(Error::Material)?,
            Kind::Directory,
        )?);
        anchors.extend(ancestors);
        check(&anchors)?;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root)
            .map_err(|_| Error::Material)?;
        anchors.push(Anchor::open(root, Kind::Directory)?);
        for (name, bytes) in [
            ("join-token", received.join_token.as_slice()),
            ("node-ownership.json", received.certificate.0.as_slice()),
            ("runtime.toml", received.settings.config.as_slice()),
        ] {
            check(&anchors)?;
            let path = root.join(name);
            let file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&path)
                .map_err(|_| Error::Material)?;
            let mut anchor = Anchor::capture(&path, file, Kind::Private)?;
            // Check inherited ACL/type/UID/mode BEFORE the first sensitive byte.
            check(&anchors)?;
            anchor.file.write_all(bytes).map_err(|_| Error::Material)?;
            anchor.len = bytes.len() as u64;
            anchor.check()?;
            anchors.push(anchor);
        }
        let result = Self {
            launch,
            root: root.into(),
            anchors,
            claims: received.claims,
            certificate: received.certificate,
            acl: received.acl,
            config_empty: received.settings.config.is_empty(),
        };
        result.recheck_with(verifier, now)?;
        Ok(result)
    }
    /// Actual received ACL metadata; caller must install it in live control policy.
    pub fn control_acl(&self) -> &ControlAcl {
        &self.acl
    }
    pub(crate) fn recheck(&self, verifier: &Mp001CertificateVerifier, now: i64) -> Result<()> {
        self.recheck_with(verifier, now)
    }
    fn recheck_with(&self, verifier: &impl CertificateVerifier, now: i64) -> Result<()> {
        if now < self.claims.issued_at || now >= self.claims.expires_at {
            return Err(Error::Certificate);
        }
        verifier
            .verify(&self.certificate, &self.claims)
            .map_err(|_| Error::Certificate)?;
        check(&self.anchors)
    }
    pub(crate) fn check_dms(&self, dms: &crate::DeadManSwitch, now: i64) -> Result<()> {
        if dms.triggered
            || dms.ttl != self.launch.heartbeat_ttl_seconds as i64
            || dms.started != now
            || dms.last_heartbeat != now
            || dms.last_tick != now
            || dms.max_lifetime
                > self
                    .claims
                    .expires_at
                    .checked_sub(now)
                    .ok_or(Error::Policy)?
        {
            return Err(Error::Policy);
        }
        Ok(())
    }
    pub(crate) fn node(&self) -> EndpointId {
        self.launch.node
    }
    pub(crate) fn command(&self) -> Result<Command> {
        // Opaque native config is retained, NOT interpreted as launch policy.
        // Refuse nonempty config until its native policy mapping is implemented;
        // never silently ignore it or permit owner-key/trust overrides.
        if !self.config_empty {
            return Err(Error::Policy);
        }

        // No token/config/credential bytes in argv/environment. Native mesh still
        // checks ownership against its actual node key and enforcing trust store.
        // Native bad-cert handling can downgrade to Unsigned: the BootGate MUST
        // first establish native key/endpoint matching. This is not proof of it.
        // --owner-required demands an owner keystore and is deliberately absent.
        let spec = ChildSpec {
            program: self.launch.program.clone(),
            args: vec![
                "serve".into(),
                "--gguf".into(),
                self.launch.gguf.as_os_str().into(),
                "--join-file".into(),
                self.root.join("join-token").into_os_string(),
                "--node-ownership".into(),
                self.root.join("node-ownership.json").into_os_string(),
                "--config".into(),
                self.root.join("runtime.toml").into_os_string(),
                "--trust-policy".into(),
                "allowlist".into(),
                "--trust-owner".into(),
                self.launch.mesh_owner_id.clone().into(),
                "--headless".into(),
                "--no-default-plugins".into(),
                "--port".into(),
                self.launch.port.to_string().into(),
                "--console".into(),
                self.launch.console_port.to_string().into(),
            ],
        };
        let mut command = Command::new(spec.program);
        command
            .args(spec.args)
            .env_clear()
            .env("HOME", &self.root)
            .env("MESH_LLM_NODE_KEY_PATH", &self.launch.node_key)
            .env("MESH_LLM_ANALYTICS", "0")
            .env("DO_NOT_TRACK", "1");
        Ok(command)
    }
}
fn check(anchors: &[Anchor]) -> Result<()> {
    for anchor in anchors {
        anchor.check()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
