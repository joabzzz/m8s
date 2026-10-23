//! Trusted local Docker adapter. No daemon discovery, pulls, adoption or Ready proof.
//! This is unwired: the existing BootGate must qualify the exact image's secure
//! entrypoint and native certificate enforcement. No gate is constructed here.
use super::*;
use crate::supervisor::BootGate;
use serde::Deserialize;
use std::{
    os::unix::fs::FileTypeExt,
    time::{Duration, Instant},
};

#[derive(Debug, thiserror::Error)]
#[error("container operation refused or outcome uncertain; retain private run material")]
pub struct ContainerError;
type CResult<T> = std::result::Result<T, ContainerError>;
/// Explicit local inputs. Image must be a preinstalled immutable reference, not
/// a client image. Run ID is fresh caller-selected lowercase32-hex, not a permit.
pub struct ContainerSpec {
    pub docker: PathBuf,
    pub socket: PathBuf,
    pub image: String,
    pub run_id: String,
    pub prompt_file: PathBuf,
    pub max_tokens: u32,
    pub request_timeout_ms: u64,
    pub command_timeout: Duration,
    pub stop_grace_seconds: u16,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerState {
    CreateUncertain,
    Created,
    StartUncertain,
    RunningObserved,
    TeardownUncertain,
    /// Exact ID absent from a successful fresh Docker all-container query.
    /// Not provider absence, tmpfs wipe evidence, inference or whole-tree proof.
    RemovedObserved,
}
/// Owns staged anchors and exact returned ID; Drop intentionally does NOT issue
/// Docker commands, delete material or claim teardown. Caller retains custody.
pub struct ContainerSession {
    material: Option<NodeMaterial>,
    launch: MeshLaunch,
    root: PathBuf,
    anchors: Vec<Anchor>,
    socket_parent: Anchor,
    spec: ContainerSpec,
    docker: Anchor,
    socket_dev: u64,
    socket_ino: u64,
    socket_uid: u32,
    socket_mode: u32,
    id: Option<String>,
    state: ContainerState,
}
fn hex(s: &str, n: usize) -> bool {
    s.len() == n
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn endpoint(id: EndpointId) -> String {
    id.0.iter().map(|b| format!("{b:02x}")).collect()
}
fn selected_path(p: &Path) -> bool {
    p.to_str().is_some_and(|s| {
        s.len() <= 4096
            && !s.is_empty()
            && s.bytes()
                .all(|b| (33..127).contains(&b) && !b",\"'\\%?#".contains(&b))
    }) && p.is_absolute()
        && p.canonicalize().ok().as_deref() == Some(p)
}
fn image(s: &str) -> bool {
    let Some((repo, digest)) = s.split_once("@sha256:") else {
        return false;
    };
    !repo.is_empty()
        && repo.len() <= 255
        && !repo.starts_with('-')
        && repo
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-:".contains(&b))
        && hex(digest, 64)
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Inspection {
    id: String,
    name: String,
    config: Config,
    state: State,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Config {
    image: String,
    labels: Labels,
}
#[derive(Deserialize)]
struct Labels {
    #[serde(rename = "org.m8s.run")]
    run: String,
    #[serde(rename = "org.m8s.node")]
    node: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct State {
    running: bool,
}
/// Captured local HTTP result, NOT authenticated mesh identity/owner routing.
/// Deliberately no Debug/Serialize: callers explicitly handle model output.
pub struct LocalCompletion {
    pub model: String,
    pub content: String,
    pub finish_reason: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompletionFrame {
    schema: String,
    scope: String,
    model: String,
    content: String,
    finish_reason: String,
    identity: String,
    readiness: String,
    owner_routing: String,
    provider_qualification: String,
    launch_performed: bool,
}
// Private failure/order seam only; public paths always use actual write/fsync.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecordStep {
    Write,
    FileSync,
    DirectorySync,
    ParentSync,
}
trait RecordIo {
    fn step(&mut self, name: &str, step: RecordStep, file: &File, bytes: &[u8]) -> CResult<()>;
}
struct SystemRecordIo;
impl RecordIo for SystemRecordIo {
    fn step(&mut self, _name: &str, step: RecordStep, file: &File, bytes: &[u8]) -> CResult<()> {
        match step {
            RecordStep::Write => {
                let mut file = file;
                file.write_all(bytes).map_err(|_| ContainerError)
            }
            _ => file.sync_all().map_err(|_| ContainerError),
        }
    }
}
impl ContainerSession {
    pub fn state(&self) -> ContainerState {
        self.state
    }
    /// Observed created ID for the coordinator's upcoming exact Store/TLS binding.
    /// Plain metadata, not provider confirmation, admission or a launch claim.
    pub fn created_container_id(&self) -> Option<&str> {
        (self.state == ContainerState::Created)
            .then_some(self.id.as_deref())
            .flatten()
    }
    /// Create a STOPPED local qualification container before boot redemption,
    /// so its actual ID can be bound by the authoritative Store/TLS handshake.
    /// Explicit trusted local mutation, NOT admission or a paid launch claim.
    /// No container process starts here; seal + canonical checks + BootGate are
    /// mandatory later. Any ambiguous create retains custody without adoption.
    pub fn create_stopped_for_qualification(
        root: &Path,
        launch: MeshLaunch,
        spec: ContainerSpec,
    ) -> CResult<Self> {
        Self::create_with_io(root, launch, spec, &mut SystemRecordIo)
    }
    fn create_with_io(
        root: &Path,
        launch: MeshLaunch,
        spec: ContainerSpec,
        io: &mut impl RecordIo,
    ) -> CResult<Self> {
        if !image(&spec.image)
            || !hex(&spec.run_id, 32)
            || !(1..=128).contains(&spec.max_tokens)
            || spec.request_timeout_ms == 0
            || u128::from(spec.request_timeout_ms) >= spec.command_timeout.as_millis()
            || spec.command_timeout.is_zero()
            || spec.command_timeout > Duration::from_secs(60)
            || spec.stop_grace_seconds == 0
            || spec.stop_grace_seconds > 30
            || spec.command_timeout <= Duration::from_secs(u64::from(spec.stop_grace_seconds))
            || [
                &spec.socket,
                &spec.docker,
                &spec.prompt_file,
                &launch.gguf,
                &launch.node_key,
            ]
            .iter()
            .any(|p| !selected_path(p))
        {
            return Err(ContainerError);
        }
        if spec.prompt_file == launch.node_key
            || spec.prompt_file == launch.gguf
            || launch.model.is_empty()
            || launch.model.len() > 128
            || !launch
                .model
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.:/".contains(&b))
        {
            return Err(ContainerError);
        }
        let parent = root.parent().ok_or(ContainerError)?;
        if !root.to_str().is_some_and(|s| {
            s.len() <= 4096
                && s.bytes()
                    .all(|b| (33..127).contains(&b) && !b",\"'\\%?#".contains(&b))
        }) || !root.is_absolute()
            || root.file_name().is_none()
            || parent
                .join(root.file_name().ok_or(ContainerError)?)
                .as_os_str()
                != root.as_os_str()
        {
            return Err(ContainerError);
        }
        let mut anchors = vec![
            Anchor::open(parent, Kind::Directory).map_err(|_| ContainerError)?,
            Anchor::open(&launch.program, Kind::Binary).map_err(|_| ContainerError)?,
            Anchor::open(&launch.gguf, Kind::Model).map_err(|_| ContainerError)?,
            Anchor::open(&launch.node_key, Kind::Private).map_err(|_| ContainerError)?,
            Anchor::open(
                launch.node_key.parent().ok_or(ContainerError)?,
                Kind::Directory,
            )
            .map_err(|_| ContainerError)?,
        ];
        if anchors[3].len == 0 || anchors[3].len > 4096 {
            return Err(ContainerError);
        }
        for selected in [
            root,
            launch.program.as_path(),
            launch.gguf.as_path(),
            launch.node_key.as_path(),
            spec.docker.as_path(),
            spec.socket.as_path(),
        ] {
            for path in selected.ancestors().skip(1) {
                anchors.push(Anchor::open(path, Kind::Ancestor).map_err(|_| ContainerError)?);
            }
        }
        let prompt = Anchor::open(&spec.prompt_file, Kind::Private).map_err(|_| ContainerError)?;
        let mut prompt_bytes = Vec::new();
        use std::io::Read;
        (&prompt.file)
            .take(8193)
            .read_to_end(&mut prompt_bytes)
            .map_err(|_| ContainerError)?;
        if prompt_bytes.is_empty()
            || prompt_bytes.len() > 8192
            || std::str::from_utf8(&prompt_bytes).is_err()
        {
            return Err(ContainerError);
        }
        prompt.check().map_err(|_| ContainerError)?;
        anchors.push(prompt);
        let docker = Anchor::open(&spec.docker, Kind::Binary).map_err(|_| ContainerError)?;
        let socket_parent =
            Anchor::open(spec.socket.parent().ok_or(ContainerError)?, Kind::Directory)
                .map_err(|_| ContainerError)?;
        let socket = fs::symlink_metadata(&spec.socket).map_err(|_| ContainerError)?;
        let uid = nix::unistd::Uid::effective().as_raw();
        if !socket.file_type().is_socket()
            || (socket.uid() != uid && socket.uid() != 0)
            || socket.mode() & 0o077 != 0
        {
            return Err(ContainerError);
        }
        check(&anchors).map_err(|_| ContainerError)?;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root)
            .map_err(|_| ContainerError)?;
        anchors.push(Anchor::open(root, Kind::Directory).map_err(|_| ContainerError)?);
        let prompt_path = root.join("request-prompt");
        let prompt_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&prompt_path)
            .map_err(|_| ContainerError)?;
        let mut prompt_anchor = Anchor::capture(&prompt_path, prompt_file, Kind::Private)
            .map_err(|_| ContainerError)?;
        check(&anchors).map_err(|_| ContainerError)?;
        prompt_anchor
            .file
            .write_all(&prompt_bytes)
            .map_err(|_| ContainerError)?;
        prompt_anchor.len = prompt_bytes.len() as u64;
        prompt_anchor.check().map_err(|_| ContainerError)?;
        anchors.push(prompt_anchor);
        let intent = serde_json::to_vec(&serde_json::json!({
            "schema":"m8s.container-intent/v2","image":spec.image,"run_id":spec.run_id,"node":endpoint(launch.node),
            "transport": {
                "docker":spec.docker,"socket":spec.socket,
                "docker_dev":docker.dev,"docker_ino":docker.ino,"docker_uid":docker.uid,"docker_mode":docker.mode,
                "socket_dev":socket.dev(),"socket_ino":socket.ino(),"socket_uid":socket.uid(),"socket_mode":socket.mode()
            }
        })).map_err(|_| ContainerError)?;
        let mut session = Self {
            material: None,
            launch,
            root: root.into(),
            anchors,
            socket_parent,
            spec,
            docker,
            socket_dev: socket.dev(),
            socket_ino: socket.ino(),
            socket_uid: socket.uid(),
            socket_mode: socket.mode(),
            id: None,
            state: ContainerState::CreateUncertain,
        };
        session.record_with_io("container-intent.json", &intent, io)?;
        let args = session.create_args();
        if let Ok(bytes) = session.execute(&args)
            && let Ok(raw) = std::str::from_utf8(&bytes)
            && let Some(id) = raw.strip_suffix('\n').filter(|s| hex(s, 64))
        {
            session.id = Some(id.to_owned());
            if session.recheck_material().is_ok() && session.inspect().is_ok_and(|running| !running)
            {
                let record = serde_json::to_vec(
                    &serde_json::json!({"schema":"m8s.container-created/v1","id":id,"run_id":session.spec.run_id,"image":session.spec.image}),
                );
                if record.is_ok_and(|bytes| {
                    session
                        .record_with_io("container-created.json", &bytes, io)
                        .is_ok()
                }) {
                    session.state = ContainerState::Created;
                }
            }
        }
        Ok(session)
    }
    /// Stage only AFTER the coordinator's actual-ID-bound authoritative Store
    /// burn and direct TLS receipt. ReceivedBundle itself is not receipt proof;
    /// the independent BootGate must verify that composition before start.
    pub fn seal(
        &mut self,
        received: ReceivedBundle,
        verifier: &Mp001CertificateVerifier,
        now: i64,
    ) -> CResult<()> {
        self.recheck_material()?;
        if self.state != ContainerState::Created || self.material.is_some() {
            return Err(ContainerError);
        }
        if self.inspect()? {
            return Err(ContainerError);
        }
        let launch = self.launch_copy();
        let material =
            NodeMaterial::stage(&self.root.join("material"), received, launch, verifier, now)
                .map_err(|_| ContainerError)?;
        self.material = Some(material);
        Ok(())
    }
    fn launch_copy(&self) -> MeshLaunch {
        MeshLaunch {
            program: self.launch.program.clone(),
            gguf: self.launch.gguf.clone(),
            node_key: self.launch.node_key.clone(),
            owner: self.launch.owner,
            node: self.launch.node,
            mesh_owner_id: self.launch.mesh_owner_id.clone(),
            model: self.launch.model.clone(),
            heartbeat_ttl_seconds: self.launch.heartbeat_ttl_seconds,
            port: self.launch.port,
            console_port: self.launch.console_port,
        }
    }
    fn recheck_material(&self) -> CResult<()> {
        check(&self.anchors).map_err(|_| ContainerError)?;
        if let Some(material) = &self.material {
            check(&material.anchors).map_err(|_| ContainerError)?;
        }
        Ok(())
    }
    fn record_with_io(&mut self, name: &str, bytes: &[u8], io: &mut impl RecordIo) -> CResult<()> {
        check(&self.anchors).map_err(|_| ContainerError)?;
        let path = self.root.join(name);
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
            .map_err(|_| ContainerError)?;
        let mut anchor = Anchor::capture(&path, file, Kind::Private).map_err(|_| ContainerError)?;
        check(&self.anchors).map_err(|_| ContainerError)?;
        io.step(name, RecordStep::Write, &anchor.file, bytes)?;
        anchor.len = bytes.len() as u64;
        anchor.check().map_err(|_| ContainerError)?;
        io.step(name, RecordStep::FileSync, &anchor.file, &[])?;
        anchor.check().map_err(|_| ContainerError)?;
        check(&self.anchors).map_err(|_| ContainerError)?;
        let directory = self
            .anchors
            .iter()
            .find(|a| {
                a.path.as_os_str() == self.root.as_os_str() && matches!(a.kind, Kind::Directory)
            })
            .ok_or(ContainerError)?;
        io.step(name, RecordStep::DirectorySync, &directory.file, &[])?;
        // The run directory itself is new: syncing only its children does not
        // establish durability of its name in the already-existing parent.
        if name == "container-intent.json" {
            let parent = self.anchors.first().ok_or(ContainerError)?;
            io.step(name, RecordStep::ParentSync, &parent.file, &[])?;
        }
        anchor.check().map_err(|_| ContainerError)?;
        check(&self.anchors).map_err(|_| ContainerError)?;
        self.anchors.push(anchor);
        Ok(())
    }
    fn name(&self) -> String {
        format!("m8s-{}", self.spec.run_id)
    }
    fn create_args(&self) -> Vec<String> {
        let uid = nix::unistd::Uid::effective().as_raw();
        let gid = nix::unistd::Gid::effective().as_raw();
        vec![
            "container".into(),
            "create".into(),
            "--pull=never".into(),
            "--name".into(),
            self.name(),
            "--label".into(),
            format!("org.m8s.run={}", self.spec.run_id),
            "--label".into(),
            format!("org.m8s.node={}", endpoint(self.launch.node)),
            "--read-only".into(),
            "--cap-drop=ALL".into(),
            "--security-opt=no-new-privileges".into(),
            "--user".into(),
            format!("{uid}:{gid}"),
            "--tmpfs".into(),
            format!("/run/m8s:rw,nosuid,nodev,noexec,mode=0700,uid={uid},gid={gid},size=8388608"),
            "--mount".into(),
            format!(
                "type=bind,src={},dst=/bootstrap,readonly",
                self.root.display()
            ),
            "--mount".into(),
            format!(
                "type=bind,src={},dst=/model.gguf,readonly",
                self.launch.gguf.display()
            ),
            "--mount".into(),
            format!(
                "type=bind,src={},dst=/node-key,readonly",
                self.launch.node_key.display()
            ),
            "--env".into(),
            "HOME=/run/m8s".into(),
            "--env".into(),
            "MESH_LLM_ANALYTICS=0".into(),
            "--env".into(),
            "DO_NOT_TRACK=1".into(),
            "--env".into(),
            "MESH_LLM_NODE_KEY_PATH=/node-key".into(),
            "--env".into(),
            "MESH_LLM_NATIVE_RUNTIME_BUNDLE_DIR=/opt/mesh/native-runtimes".into(),
            "--entrypoint".into(),
            "/opt/mesh/mesh-llm".into(),
            self.spec.image.clone(),
            "serve".into(),
            "--gguf".into(),
            "/model.gguf".into(),
            "--join-file".into(),
            "/bootstrap/material/join-token".into(),
            "--node-ownership".into(),
            "/bootstrap/material/node-ownership.json".into(),
            "--node-ownership-required".into(),
            "--config".into(),
            "/bootstrap/material/runtime.toml".into(),
            "--trust-policy".into(),
            "allowlist".into(),
            "--trust-owner".into(),
            self.launch.mesh_owner_id.clone(),
            "--headless".into(),
            "--no-default-plugins".into(),
            "--port".into(),
            "9337".into(),
            "--console".into(),
            "3131".into(),
        ]
    }
    fn recheck_inputs(&self) -> CResult<()> {
        self.docker.check().map_err(|_| ContainerError)?;
        self.socket_parent.check().map_err(|_| ContainerError)?;
        let socket = fs::symlink_metadata(&self.spec.socket).map_err(|_| ContainerError)?;
        if !socket.file_type().is_socket()
            || socket.dev() != self.socket_dev
            || socket.ino() != self.socket_ino
            || socket.uid() != self.socket_uid
            || socket.mode() != self.socket_mode
            || socket.mode() & 0o077 != 0
        {
            return Err(ContainerError);
        }
        Ok(())
    }
    fn execute(&self, args: &[String]) -> CResult<Vec<u8>> {
        let deadline = Instant::now()
            .checked_add(self.spec.command_timeout)
            .ok_or(ContainerError)?;
        self.recheck_inputs()?;
        let mut command = Command::new(&self.spec.docker);
        command
            .arg("--host")
            .arg(format!("unix://{}", self.spec.socket.display()))
            .args(args)
            .env_clear()
            .env("HOME", &self.root);
        let bytes =
            super::process::capture(command, deadline, 16 * 1024).map_err(|_| ContainerError)?;
        self.recheck_inputs()?;
        if Instant::now() >= deadline {
            return Err(ContainerError);
        }
        Ok(bytes)
    }
    fn inspect(&self) -> CResult<bool> {
        inspect_exact(self)
    }

    pub fn start(
        &mut self,
        gate: &mut impl BootGate,
        dms: &crate::DeadManSwitch,
        verifier: &Mp001CertificateVerifier,
        now: i64,
    ) -> CResult<()> {
        let material = self.material.as_ref().ok_or(ContainerError)?;
        material
            .recheck(verifier, now)
            .and_then(|()| material.check_dms(dms, now))
            .map_err(|_| ContainerError)?;
        if !gate.authorize() {
            return Err(ContainerError);
        }
        let material = self.material.as_ref().ok_or(ContainerError)?;
        material
            .recheck(verifier, now)
            .and_then(|()| material.check_dms(dms, now))
            .map_err(|_| ContainerError)?;
        self.start_checked()
    }
    /// Explicit one-shot local qualification lifecycle after sealing. The same
    /// canonical verifier, fresh DMS and external BootGate remain mandatory.
    /// A failed/ambiguous start or request still gets one fenced teardown path;
    /// no completion escapes unless fresh exact-ID absence was observed.
    /// This is not a retry loop, admission grant or surviving provider watchdog.
    pub fn run_once_and_remove(
        &mut self,
        gate: &mut impl BootGate,
        dms: &crate::DeadManSwitch,
        verifier: &Mp001CertificateVerifier,
        now: i64,
    ) -> CResult<LocalCompletion> {
        let started = self.start(gate, dms, verifier, now);
        self.finish_once(started)
    }
    fn finish_once(&mut self, started: CResult<()>) -> CResult<LocalCompletion> {
        let completion = started.and_then(|()| self.request());
        // Do not let `?` on start/request skip cleanup. Its uncertainty is
        // retained even when cleanup succeeds; never retry the request.
        self.stop_and_remove()?;
        completion
    }
    fn start_checked(&mut self) -> CResult<()> {
        self.start_with_io(&mut SystemRecordIo)
    }
    fn start_with_io(&mut self, io: &mut impl RecordIo) -> CResult<()> {
        if self
            .material
            .as_ref()
            .is_none_or(|material| !material.config_empty)
        {
            return Err(ContainerError);
        }
        self.recheck_material()?;
        if self.state != ContainerState::Created || self.inspect()? {
            return Err(ContainerError);
        }
        self.state = ContainerState::StartUncertain;
        self.record_with_io("container-start-attempt", b"explicit-start-attempt\n", io)?;
        self.execute(&[
            "container".into(),
            "start".into(),
            self.id.clone().ok_or(ContainerError)?,
        ])?;
        if !self.inspect()? {
            return Err(ContainerError);
        }
        self.recheck_material()?;
        self.state = ContainerState::RunningObserved;
        Ok(())
    }
    /// One explicit local HTTP qualification request *inside* the owned container.
    /// No published port or listener widening is necessary. Output is bounded
    /// captured bytes, never printed here; it is not authenticated canary proof.
    pub fn request(&self) -> CResult<LocalCompletion> {
        self.recheck_material()?;
        if self.state != ContainerState::RunningObserved || !self.inspect()? {
            return Err(ContainerError);
        }
        let mut args = vec![
            "container".into(),
            "exec".into(),
            self.id.clone().ok_or(ContainerError)?,
            "/usr/local/bin/m8s".into(),
            "prototype".into(),
            "request".into(),
            "--local-qualification".into(),
            "--endpoint".into(),
            "http://127.0.0.1:9337".into(),
            "--model".into(),
            self.launch.model.clone(),
            "--prompt-file".into(),
            "/bootstrap/request-prompt".into(),
            "--max-tokens".into(),
            self.spec.max_tokens.to_string(),
            "--timeout-ms".into(),
            self.spec.request_timeout_ms.to_string(),
            "--json".into(),
        ];
        // Reserve at least half of the existing total request deadline for the
        // single inference POST. Only bounded model-discovery GETs may retry.
        if self.spec.request_timeout_ms >= 2 {
            args.extend([
                "--wait-model-ms".into(),
                (self.spec.request_timeout_ms / 2).to_string(),
            ]);
        }
        let bytes = self.execute(&args)?;
        self.recheck_material()?;
        let frame: CompletionFrame = serde_json::from_slice(&bytes).map_err(|_| ContainerError)?;
        if frame.schema != "m8s.prototype-completion/v1"
            || frame.scope != "local-http-qualification-only"
            || frame.model != self.launch.model
            || frame.content.is_empty()
            || frame.content.len() > 32768
            || !matches!(frame.finish_reason.as_str(), "stop" | "length")
            || frame.identity != "not-verified"
            || frame.readiness != "not-assessed"
            || frame.owner_routing != "not-verified"
            || frame.provider_qualification != "not-performed"
            || frame.launch_performed
        {
            return Err(ContainerError);
        }
        Ok(LocalCompletion {
            model: frame.model,
            content: frame.content,
            finish_reason: frame.finish_reason,
        })
    }
    /// Explicit teardown only. Every destructive call is fenced to a retained ID
    /// with matching observed run/node/image metadata. Never adopt by name.
    pub fn stop_and_remove(&mut self) -> CResult<()> {
        if self.state == ContainerState::RemovedObserved {
            return Ok(());
        }
        self.state = ContainerState::TeardownUncertain;
        remove_exact(self)?;
        self.state = ContainerState::RemovedObserved;
        Ok(())
    }
    /// Explicit read-only resolution after a lost teardown reply. One fresh
    /// query, no repeated destruction, name adoption or retry loop.
    pub fn observe_removed(&mut self) -> CResult<bool> {
        let absent = observe_absent(self)?;
        if absent {
            self.state = ContainerState::RemovedObserved;
        }
        Ok(absent)
    }
}
#[cfg(test)]
mod tests;

mod recovery;
pub use recovery::{OwnedContainerStop, StopSpec};

// One destructive algorithm for retained and explicitly recovered ownership.
trait ExactTarget {
    fn execute_exact(&self, args: &[String]) -> CResult<Vec<u8>>;
    fn exact_id(&self) -> CResult<&str>;
    fn run_id(&self) -> &str;
    fn exact_image(&self) -> &str;
    fn node_id(&self) -> String;
    fn grace(&self) -> u16;
}
impl ExactTarget for ContainerSession {
    fn execute_exact(&self, args: &[String]) -> CResult<Vec<u8>> {
        self.execute(args)
    }
    fn exact_id(&self) -> CResult<&str> {
        self.id.as_deref().ok_or(ContainerError)
    }
    fn run_id(&self) -> &str {
        &self.spec.run_id
    }
    fn exact_image(&self) -> &str {
        &self.spec.image
    }
    fn node_id(&self) -> String {
        endpoint(self.launch.node)
    }
    fn grace(&self) -> u16 {
        self.spec.stop_grace_seconds
    }
}
fn inspect_exact(target: &impl ExactTarget) -> CResult<bool> {
    let id = target.exact_id()?;
    let bytes = target.execute_exact(&["container".into(), "inspect".into(), id.into()])?;
    let data: Vec<Inspection> = serde_json::from_slice(&bytes).map_err(|_| ContainerError)?;
    let [data] = data.as_slice() else {
        return Err(ContainerError);
    };
    if data.id != id
        || data.name != format!("/m8s-{}", target.run_id())
        || data.config.image != target.exact_image()
        || data.config.labels.run != target.run_id()
        || data.config.labels.node != target.node_id()
    {
        return Err(ContainerError);
    }
    Ok(data.state.running)
}
fn remove_exact(target: &impl ExactTarget) -> CResult<()> {
    let id = target.exact_id()?.to_owned();
    if inspect_exact(target)? {
        target.execute_exact(&[
            "container".into(),
            "stop".into(),
            "--time".into(),
            target.grace().to_string(),
            id.clone(),
        ])?;
        if inspect_exact(target)? {
            return Err(ContainerError);
        }
    }
    target.execute_exact(&["container".into(), "rm".into(), id.clone()])?;
    if !observe_absent(target)? {
        return Err(ContainerError);
    }
    Ok(())
}
fn observe_absent(target: &impl ExactTarget) -> CResult<bool> {
    let id = target.exact_id()?.to_owned();
    let observed = target.execute_exact(&[
        "container".into(),
        "ls".into(),
        "--all".into(),
        "--no-trunc".into(),
        "--filter".into(),
        format!("id={id}"),
        "--format".into(),
        "{{.ID}}".into(),
    ])?;
    if observed.is_empty() {
        Ok(true)
    } else if observed == format!("{id}\n").as_bytes() {
        Ok(false)
    } else {
        Err(ContainerError)
    }
}
