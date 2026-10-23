//! Explicit restart-safe cleanup, never launch authority or name adoption.
use super::*;
use std::io::Read;

pub struct StopSpec {
    pub docker: PathBuf,
    pub socket: PathBuf,
    pub run_id: String,
    pub container_id: String,
    pub command_timeout: Duration,
    pub stop_grace_seconds: u16,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    schema: String,
    image: String,
    run_id: String,
    node: String,
    transport: Transport,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Transport {
    docker: PathBuf,
    socket: PathBuf,
    docker_dev: u64,
    docker_ino: u64,
    docker_uid: u32,
    docker_mode: u32,
    socket_dev: u64,
    socket_ino: u64,
    socket_uid: u32,
    socket_mode: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Created {
    schema: String,
    id: String,
    run_id: String,
    image: String,
}
/// Local private records are custody evidence, not a provider identity/admission
/// certificate. The explicitly selected Docker daemon remains trusted. No Drop effects.
pub struct OwnedContainerStop {
    root: PathBuf,
    anchors: Vec<Anchor>,
    docker: Anchor,
    socket_parent: Anchor,
    socket_identity: (u64, u64, u32, u32),
    spec: StopSpec,
    image: String,
    node: String,
    attempted: bool,
}
impl OwnedContainerStop {
    pub fn open(root: &Path, spec: StopSpec) -> CResult<Self> {
        if !selected_path(root)
            || !selected_path(&spec.docker)
            || !selected_path(&spec.socket)
            || !hex(&spec.run_id, 32)
            || !hex(&spec.container_id, 64)
            || spec.command_timeout > Duration::from_secs(60)
            || spec.stop_grace_seconds == 0
            || spec.stop_grace_seconds > 30
            || spec.command_timeout <= Duration::from_secs(u64::from(spec.stop_grace_seconds))
        {
            return Err(ContainerError);
        }
        let mut anchors = vec![Anchor::open(root, Kind::Directory).map_err(|_| ContainerError)?];
        for selected in [root, spec.docker.as_path(), spec.socket.as_path()] {
            for path in selected.ancestors().skip(1) {
                anchors.push(Anchor::open(path, Kind::Ancestor).map_err(|_| ContainerError)?);
            }
        }
        fn read_record<T: serde::de::DeserializeOwned>(
            root: &Path,
            name: &str,
            anchors: &mut Vec<Anchor>,
        ) -> CResult<T> {
            let anchor =
                Anchor::open(&root.join(name), Kind::Private).map_err(|_| ContainerError)?;
            if anchor.len == 0 || anchor.len > 4096 {
                return Err(ContainerError);
            }
            let mut bytes = Vec::new();
            (&anchor.file)
                .take(4097)
                .read_to_end(&mut bytes)
                .map_err(|_| ContainerError)?;
            if bytes.len() > 4096 {
                return Err(ContainerError);
            }
            anchor.check().map_err(|_| ContainerError)?;
            let value = serde_json::from_slice(&bytes).map_err(|_| ContainerError)?;
            anchors.push(anchor);
            Ok(value)
        }
        let intent: Intent = read_record(root, "container-intent.json", &mut anchors)?;
        let created: Created = read_record(root, "container-created.json", &mut anchors)?;
        if intent.schema != "m8s.container-intent/v2"
            || created.schema != "m8s.container-created/v1"
            || intent.run_id != spec.run_id
            || created.run_id != spec.run_id
            || created.id != spec.container_id
            || created.image != intent.image
            || !image(&intent.image)
            || !hex(&intent.node, 64)
        {
            return Err(ContainerError);
        }
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
        let expected = &intent.transport;
        if expected.docker != spec.docker
            || expected.socket != spec.socket
            || (
                expected.docker_dev,
                expected.docker_ino,
                expected.docker_uid,
                expected.docker_mode,
            ) != (docker.dev, docker.ino, docker.uid, docker.mode)
            || (
                expected.socket_dev,
                expected.socket_ino,
                expected.socket_uid,
                expected.socket_mode,
            ) != (socket.dev(), socket.ino(), socket.uid(), socket.mode())
        {
            return Err(ContainerError);
        }
        let handle = Self {
            root: root.into(),
            anchors,
            docker,
            socket_parent,
            socket_identity: (socket.dev(), socket.ino(), socket.uid(), socket.mode()),
            spec,
            image: intent.image,
            node: intent.node,
            attempted: false,
        };
        handle.check_inputs()?;
        Ok(handle)
    }
    fn check_inputs(&self) -> CResult<()> {
        check(&self.anchors).map_err(|_| ContainerError)?;
        self.docker.check().map_err(|_| ContainerError)?;
        self.socket_parent.check().map_err(|_| ContainerError)?;
        let socket = fs::symlink_metadata(&self.spec.socket).map_err(|_| ContainerError)?;
        if !socket.file_type().is_socket()
            || self.socket_identity != (socket.dev(), socket.ino(), socket.uid(), socket.mode())
        {
            return Err(ContainerError);
        }
        Ok(())
    }
    /// One read-only exact-ID query for explicit uncertain-removal resolution.
    /// No inspect-error-as-absence shortcut and no destructive call/retry.
    pub fn observe_removed(&self) -> CResult<bool> {
        observe_absent(self)
    }
    /// One attempt on this handle. An absent/missing inspect is NOT confirmation.
    /// Failed rm/confirmation remains uncertain; no retry or artifact deletion.
    pub fn stop_and_remove(&mut self) -> CResult<()> {
        if self.attempted {
            return Err(ContainerError);
        }
        self.attempted = true;
        remove_exact(self)
    }
}
impl ExactTarget for OwnedContainerStop {
    fn execute_exact(&self, args: &[String]) -> CResult<Vec<u8>> {
        self.check_inputs()?;
        let deadline = Instant::now()
            .checked_add(self.spec.command_timeout)
            .ok_or(ContainerError)?;
        let mut command = Command::new(&self.spec.docker);
        command
            .arg("--host")
            .arg(format!("unix://{}", self.spec.socket.display()))
            .args(args)
            .env_clear()
            .env("HOME", &self.root);
        let bytes = super::super::process::capture(command, deadline, 16 * 1024)
            .map_err(|_| ContainerError)?;
        self.check_inputs()?;
        if Instant::now() >= deadline {
            return Err(ContainerError);
        }
        Ok(bytes)
    }
    fn exact_id(&self) -> CResult<&str> {
        Ok(&self.spec.container_id)
    }
    fn run_id(&self) -> &str {
        &self.spec.run_id
    }
    fn exact_image(&self) -> &str {
        &self.image
    }
    fn node_id(&self) -> String {
        self.node.clone()
    }
    fn grace(&self) -> u16 {
        self.spec.stop_grace_seconds
    }
}
