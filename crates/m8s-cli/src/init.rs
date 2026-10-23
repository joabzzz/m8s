//! Explicit fresh Mock starter files, never daemon/credential/provider setup.
use std::io::Write;
const HELP: &str = "m8s init --mock --directory ABS [--json]\nFlags once, any order. Unix only; fresh final directory under existing canonical owned safe parent. No defaults, recursive parents, overwrite/adoption/chmod/cleanup/retry. Partial files may remain on failure; broken stdout may follow successful creation. Mock-only starter files, no daemon/model/credential/provider effects.\n";
#[cfg(unix)]
pub(super) const STARTER_README: &str = concat!(
    "# Mock-only starter workspace\n\n",
    "These public examples configure simulated local control, NOT inference, GPU/provider/security qualification, authorization or Ready. No credentials/keys/approval are generated.\n\n",
    "From this workspace directory, manually launch the hardwired Mock daemon in one shell:\n\n",
    "```sh\nm8sd \"$PWD/state\" --config \"$PWD/m8s.toml\" --catalog \"$PWD/catalog.toml\"\n```\n\n",
    "In another shell at the same workspace directory:\n\n",
    "```sh\nm8s config validate --config \"$PWD/m8s.toml\" --catalog \"$PWD/catalog.toml\" --json\n",
    "m8s --socket \"$PWD/state/control.sock\" up small\n",
    "m8s --socket \"$PWD/state/control.sock\" status --json\n",
    "m8s --socket \"$PWD/state/control.sock\" config-status --json\n",
    "m8s --socket \"$PWD/state/control.sock\" reload-config --check --json\n",
    "m8s --socket \"$PWD/state/control.sock\" logs --follow --after 0 --limit 100 --max-pages 3 --json\n",
    "m8s --socket \"$PWD/state/control.sock\" down\n",
    "m8s --socket \"$PWD/state/control.sock\" shutdown --json\n```\n\n",
    "Startup/reload validation is not runtime or budget approval. File edits alone do not reload. Schedule preview/queue replay are offline projections, NOT automation. Mock nodes never become Ready or serve inference. Local-demo is separate, Unix/local-development default OFF, synthetic accounting only.\n\n",
    "Init creates only these starter files and an empty private state directory; it does not open Store/schema/journal/locks or start the daemon. Creation is NOT transactional: any failure leaves partial artifacts, never automatic rollback/deletion/retry. An output failure may follow complete creation; do not blindly repeat or overwrite. Same-UID hostile path races are NOT confined by these checks.\n",
);
fn parse(args: &[String]) -> Result<&std::path::Path, &'static str> {
    let (mut mock, mut json, mut dir) = (false, false, None);
    let mut rest = args;
    while let Some((arg, tail)) = rest.split_first() {
        rest = tail;
        match arg.as_str() {
            "--mock" if !mock => mock = true,
            "--json" if !json => json = true,
            "--directory" if dir.is_none() => {
                let (v, tail) = rest.split_first().ok_or("invalid-init-arguments")?;
                if v.len() > 4096 || v.contains('\0') {
                    return Err("invalid-init-arguments");
                }
                dir = Some(std::path::Path::new(v));
                rest = tail;
            }
            _ => return Err("invalid-init-arguments"),
        }
    }
    let path = dir
        .filter(|p| p.is_absolute())
        .ok_or("invalid-init-arguments")?;
    if !mock {
        return Err("invalid-init-arguments");
    }
    Ok(path)
}
pub(super) fn run(args: &[String]) -> Result<(), &'static str> {
    if args == ["--help"] {
        return std::io::stdout()
            .lock()
            .write_all(HELP.as_bytes())
            .map_err(|_| "init-output-unavailable");
    }
    let path = parse(args)?;
    create(path)?;
    let mut bytes=serde_json::to_vec(&serde_json::json!({"schema":"m8s.mock-init/v1","mode":"mock-only","effects":"starter-files-created",
        "files":["m8s.toml","catalog.toml","README.md"],"directories":["state"],"daemon_started":false,"inference_served":false,"authorization":"not-assessed","readiness":"not-assessed"})).map_err(|_|"init-output-unavailable")?;
    bytes.push(b'\n');
    let mut out = std::io::stdout().lock();
    out.write_all(&bytes)
        .and_then(|()| out.flush())
        .map_err(|_| "init-output-unavailable")
}
#[cfg(not(unix))]
fn create(_: &std::path::Path) -> Result<(), &'static str> {
    Err("init-unavailable-on-platform")
}
#[cfg(unix)]
fn create(path: &std::path::Path) -> Result<(), &'static str> {
    unix::create(path, None).map_err(|_| "init-failed-partial-artifacts-may-remain")
}
#[cfg(unix)]
mod unix {
    use super::*;
    use std::{
        fs::{self, File, Metadata},
        os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
        path::{Path, PathBuf},
    };
    struct Anchor {
        path: PathBuf,
        file: File,
        uid: u32,
        mode: u32,
        size: Option<u64>,
    }
    impl Anchor {
        fn check(&self, acl: &impl Fn(&File) -> Result<(), ()>) -> Result<(), ()> {
            acl(&self.file)?;
            let fd = self.file.metadata().map_err(|_| ())?;
            let p = fs::symlink_metadata(&self.path).map_err(|_| ())?;
            if fd.dev() != p.dev()
                || fd.ino() != p.ino()
                || p.uid() != self.uid
                || fd.uid() != self.uid
                || p.mode() != self.mode
                || fd.mode() != self.mode
                || self.size.is_some_and(|n| {
                    !p.is_file()
                        || !fd.is_file()
                        || p.nlink() != 1
                        || fd.nlink() != 1
                        || p.len() != n
                        || fd.len() != n
                })
                || (self.size.is_none() && (!p.is_dir() || !fd.is_dir()))
            {
                return Err(());
            }
            acl(&self.file)
        }
    }
    fn directory(
        path: &Path,
        private: bool,
        uid: u32,
        acl: &impl Fn(&File) -> Result<(), ()>,
    ) -> Result<Anchor, ()> {
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_DIRECTORY)
            .open(path)
            .map_err(|_| ())?;
        let m = file.metadata().map_err(|_| ())?;
        if !m.is_dir() || (private && (m.uid() != uid || m.mode() & 0o7777 != 0o700)) {
            return Err(());
        }
        let anchor = Anchor {
            path: path.into(),
            file,
            uid: m.uid(),
            mode: m.mode(),
            size: None,
        };
        anchor.check(acl)?;
        Ok(anchor)
    }
    fn check(anchors: &[Anchor], acl: &impl Fn(&File) -> Result<(), ()>) -> Result<(), ()> {
        for anchor in anchors {
            anchor.check(acl)?;
        }
        Ok(())
    }
    fn owned_file(m: &Metadata, uid: u32) -> bool {
        m.is_file() && m.uid() == uid && m.mode() & 0o7777 == 0o600 && m.nlink() == 1
    }
    pub(super) fn create(path: &Path, fail_after: Option<usize>) -> Result<(), ()> {
        create_checked(path, fail_after, &|file| {
            m8s_secrets::check_no_extended_acl(file).map_err(|_| ())
        })
    }
    fn create_checked(
        path: &Path,
        fail_after: Option<usize>,
        acl: &impl Fn(&File) -> Result<(), ()>,
    ) -> Result<(), ()> {
        // Validate exact shipped bytes before the first directory/file write.
        let catalog = toml::from_str(m8s_core::models::EXAMPLE_CATALOG).map_err(|_| ())?;
        m8s_config::Config::parse(
            m8s_config::EXAMPLE.as_bytes(),
            &catalog,
            &m8s_config::ValidationLimits::default(),
        )
        .map_err(|_| ())?;
        let parent = path.parent().ok_or(())?;
        let name = path.file_name().ok_or(())?;
        if parent.join(name).as_os_str() != path.as_os_str()
            || fs::canonicalize(parent).map_err(|_| ())?.as_os_str() != parent.as_os_str()
        {
            return Err(());
        }
        let uid = nix::unistd::geteuid().as_raw();
        let mut anchors = Vec::new();
        let mut part = PathBuf::new();
        for component in parent.components() {
            if anchors.len() >= 128 {
                return Err(());
            }
            part.push(component.as_os_str());
            let a = directory(&part, false, uid, acl)?;
            let m = a.file.metadata().map_err(|_| ())?;
            if (m.uid() != 0 && m.uid() != uid)
                || (m.mode() & 0o022 != 0 && !(m.uid() == 0 && m.mode() & 0o1000 != 0))
            {
                return Err(());
            }
            anchors.push(a);
        }
        let m = anchors.last().ok_or(())?.file.metadata().map_err(|_| ())?;
        if m.uid() != uid || m.mode() & 0o022 != 0 {
            return Err(());
        }
        check(&anchors, acl)?;
        // Exclusive mkdir refuses every existing final entry, even a safe empty dir.
        fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .map_err(|_| ())?;
        anchors.push(directory(path, true, uid, acl)?);
        check(&anchors, acl)?;
        if fail_after == Some(0) {
            return Err(());
        }
        for (index, (name, bytes)) in [
            ("m8s.toml", m8s_config::EXAMPLE.as_bytes()),
            ("catalog.toml", m8s_core::models::EXAMPLE_CATALOG.as_bytes()),
            ("README.md", STARTER_README.as_bytes()),
        ]
        .into_iter()
        .enumerate()
        {
            check(&anchors, acl)?;
            let child = path.join(name);
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&child)
                .map_err(|_| ())?;
            let m = file.metadata().map_err(|_| ())?;
            if !owned_file(&m, uid) || m.len() != 0 {
                return Err(());
            }
            let mut anchor = Anchor {
                path: child,
                file: file.try_clone().map_err(|_| ())?,
                uid,
                mode: m.mode(),
                size: Some(0),
            };
            anchor.check(acl)?;
            check(&anchors, acl)?;
            file.write_all(bytes).map_err(|_| ())?;
            anchor.size = Some(bytes.len() as u64);
            anchor.check(acl)?;
            anchors.push(anchor);
            check(&anchors, acl)?;
            if fail_after == Some(index + 1) {
                return Err(());
            }
        }
        check(&anchors, acl)?;
        let state = path.join("state");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&state)
            .map_err(|_| ())?;
        anchors.push(directory(&state, true, uid, acl)?);
        check(&anchors, acl)
    }
    #[cfg(test)]
    mod acl_tests {
        use super::*;
        #[test]
        fn injected_parent_acl_denial_prevents_creation_and_file_denial_keeps_empty_partial() {
            let root = tempfile::tempdir().unwrap();
            let parent = root.path().canonicalize().unwrap();
            let m = File::open(&parent).unwrap().metadata().unwrap();
            let identity = (m.dev(), m.ino());
            // Library seam tests distinguish access/default/unknown failures;
            // init intentionally collapses ALL checker failures to one opaque refusal.
            let path = parent.join("denied-parent");
            assert!(
                create_checked(&path, None, &|file| {
                    let m = file.metadata().map_err(|_| ())?;
                    if (m.dev(), m.ino()) == identity {
                        Err(())
                    } else {
                        Ok(())
                    }
                })
                .is_err()
            );
            assert!(!path.exists());
            let partial = parent.join("partial");
            assert!(
                create_checked(
                    &partial,
                    None,
                    &|file| if file.metadata().map_err(|_| ())?.is_file() {
                        Err(())
                    } else {
                        Ok(())
                    }
                )
                .is_err()
            );
            assert_eq!(fs::metadata(partial.join("m8s.toml")).unwrap().len(), 0);
            assert_eq!(fs::read_dir(&partial).unwrap().count(), 1);
            assert!(!partial.join("state").exists());
        }
    }
}
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn injected_failure_retains_partial_artifacts_without_rollback_or_retry() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().canonicalize().unwrap();
        let path = parent.join("partial");
        assert!(unix::create(&path, Some(1)).is_err());
        assert_eq!(
            std::fs::read(path.join("m8s.toml")).unwrap(),
            m8s_config::EXAMPLE.as_bytes()
        );
        assert!(!path.join("catalog.toml").exists());
        assert!(!path.join("state").exists());
        assert!(unix::create(&path, None).is_err());
        assert_eq!(std::fs::read_dir(&path).unwrap().count(), 1);
    }
}
