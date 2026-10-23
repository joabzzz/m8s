//! Size and SHA-256 verification of the model file MeshLLM resolves at start.
//! A mismatch is terminal: the supervisor stops the native child and the
//! downloaded cache is removed. Absence of the file is "not yet downloaded",
//! never verification.
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::Duration,
};

/// Pinned exact size and digest of the model file, from the artifact contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelPin {
    pub bytes: u64,
    pub sha256: [u8; 32],
}

/// Split `repo@40-hex-revision/file.gguf` into `(repo, revision, file)`.
pub(super) fn parse_model_ref(model: &str) -> Option<(&str, &str, &str)> {
    let (head, file) = model.rsplit_once('/')?;
    let (repo, revision) = head.rsplit_once('@')?;
    (repo.split('/').count() == 2
        && repo.split('/').all(|p| {
            !p.is_empty()
                && !p.starts_with('.')
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
        && revision.len() == 40
        && revision.bytes().all(|b| b.is_ascii_hexdigit())
        && file.ends_with(".gguf")
        && file
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)))
    .then_some((repo, revision, file))
}

/// Standard Hugging Face hub layout inside the cache the node dictates.
pub(super) fn snapshot_path(cache: &Path, model: &str) -> Option<PathBuf> {
    let (repo, revision, file) = parse_model_ref(model)?;
    Some(
        cache
            .join(format!("models--{}", repo.replace('/', "--")))
            .join("snapshots")
            .join(revision)
            .join(file),
    )
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    Absent,
    Verified,
    Mismatch,
}

/// `cache` confines symlinked hub snapshots: the resolved file must stay inside.
pub(super) fn verify_file(path: &Path, cache: Option<&Path>, pin: &ModelPin) -> Verdict {
    let resolved = match cache {
        Some(cache) => {
            match fs::symlink_metadata(path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Verdict::Absent,
                Err(_) => return Verdict::Mismatch,
                Ok(_) => {}
            }
            match (path.canonicalize(), cache.canonicalize()) {
                (Ok(file), Ok(root)) if file.starts_with(&root) => file,
                _ => return Verdict::Mismatch,
            }
        }
        None => match fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Verdict::Absent,
            Err(_) => return Verdict::Mismatch,
            Ok(_) => path.to_owned(),
        },
    };
    let Ok(file) = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&resolved)
    else {
        return Verdict::Mismatch;
    };
    let Ok(before) = file.metadata() else {
        return Verdict::Mismatch;
    };
    // Exact size first: a truncated or extended file never reaches hashing.
    if !before.is_file() || before.len() != pin.bytes {
        return Verdict::Mismatch;
    }
    if hash(file, pin.bytes).is_some_and(|d| d == pin.sha256) {
        // Re-stat the path after hashing so a swap during the read is a failure.
        match fs::symlink_metadata(&resolved) {
            Ok(after)
                if after.dev() == before.dev()
                    && after.ino() == before.ino()
                    && after.len() == pin.bytes =>
            {
                return Verdict::Verified;
            }
            _ => {}
        }
    }
    Verdict::Mismatch
}

fn hash(file: File, expected: u64) -> Option<[u8; 32]> {
    let mut digest = Sha256::new();
    let mut reader = file.take(expected.checked_add(1)?);
    let mut buffer = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buffer).ok()?;
        if n == 0 {
            break;
        }
        total += n as u64;
        digest.update(&buffer[..n]);
    }
    (total == expected).then(|| digest.finalize().into())
}

const PENDING: u8 = 0;
const VERIFIED: u8 = 1;
const FAILED: u8 = 2;

/// Background verification: hashing tens of GiB must never block supervision.
pub struct ModelWatch {
    state: Arc<AtomicU8>,
    stop: Arc<AtomicBool>,
}
impl ModelWatch {
    pub(super) fn spawn(path: PathBuf, cache: Option<PathBuf>, pin: ModelPin) -> Self {
        Self::spawn_polling(path, cache, pin, Duration::from_millis(250))
    }
    pub(crate) fn spawn_polling(
        path: PathBuf,
        cache: Option<PathBuf>,
        pin: ModelPin,
        poll: Duration,
    ) -> Self {
        let state = Arc::new(AtomicU8::new(PENDING));
        let stop = Arc::new(AtomicBool::new(false));
        let (s, st) = (state.clone(), stop.clone());
        std::thread::spawn(move || {
            while !st.load(Ordering::Acquire) {
                match verify_file(&path, cache.as_deref(), &pin) {
                    Verdict::Absent => std::thread::sleep(poll),
                    Verdict::Verified => {
                        s.store(VERIFIED, Ordering::Release);
                        return;
                    }
                    Verdict::Mismatch => {
                        // Publish failure first so the child is stopped before
                        // (and regardless of whether) cleanup succeeds.
                        s.store(FAILED, Ordering::Release);
                        if let Some(cache) = &cache {
                            let _ = fs::remove_dir_all(cache);
                        }
                        return;
                    }
                }
            }
        });
        Self { state, stop }
    }
    pub fn verified(&self) -> bool {
        self.state.load(Ordering::Acquire) == VERIFIED
    }
    pub fn failed(&self) -> bool {
        self.state.load(Ordering::Acquire) == FAILED
    }
}
impl Drop for ModelWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    const BODY: &[u8] = b"synthetic-model-bytes-not-a-real-gguf";
    const REF: &str = "org/repo@0123456789012345678901234567890123456789/model.gguf";

    fn pin(body: &[u8]) -> ModelPin {
        ModelPin {
            bytes: body.len() as u64,
            sha256: Sha256::digest(body).into(),
        }
    }
    fn dir(tag: &str) -> PathBuf {
        let root = Path::new("/tmp").join(format!("m8s-model-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root.canonicalize().unwrap()
    }
    // Hub layout: snapshot entry is a symlink to a blob inside the cache.
    fn hub(cache: &Path, body: &[u8]) -> PathBuf {
        let snapshot = snapshot_path(cache, REF).unwrap();
        fs::create_dir_all(snapshot.parent().unwrap()).unwrap();
        fs::create_dir_all(cache.join("blobs")).unwrap();
        let blob = cache.join("blobs/abc");
        fs::write(&blob, body).unwrap();
        symlink(&blob, &snapshot).unwrap();
        snapshot
    }
    fn wait(watch: &ModelWatch) {
        for _ in 0..200 {
            if watch.verified() || watch.failed() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn matching_hub_file_verifies_and_absent_is_not_verified() {
        let root = dir("ok");
        let cache = root.join("hf-cache");
        let path = snapshot_path(&cache, REF).unwrap();
        assert_eq!(
            verify_file(&path, Some(&cache), &pin(BODY)),
            Verdict::Absent
        );
        hub(&cache, BODY);
        assert_eq!(
            verify_file(&path, Some(&cache), &pin(BODY)),
            Verdict::Verified
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn truncated_extended_and_corrupted_files_mismatch() {
        let root = dir("bad");
        let cache = root.join("hf-cache");
        let path = hub(&cache, &BODY[..BODY.len() - 1]);
        assert_eq!(
            verify_file(&path, Some(&cache), &pin(BODY)),
            Verdict::Mismatch
        );
        fs::write(cache.join("blobs/abc"), [BODY, b"x"].concat()).unwrap();
        assert_eq!(
            verify_file(&path, Some(&cache), &pin(BODY)),
            Verdict::Mismatch
        );
        let mut corrupt = BODY.to_vec();
        corrupt[3] ^= 1; // same length, different digest
        fs::write(cache.join("blobs/abc"), corrupt).unwrap();
        assert_eq!(
            verify_file(&path, Some(&cache), &pin(BODY)),
            Verdict::Mismatch
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn snapshot_link_escaping_cache_mismatches() {
        let root = dir("escape");
        let cache = root.join("hf-cache");
        let outside = root.join("outside.gguf");
        fs::write(&outside, BODY).unwrap();
        let path = snapshot_path(&cache, REF).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        symlink(&outside, &path).unwrap();
        assert_eq!(
            verify_file(&path, Some(&cache), &pin(BODY)),
            Verdict::Mismatch
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_path_refuses_symlink_and_checks_digest() {
        let root = dir("explicit");
        let file = root.join("m.gguf");
        fs::write(&file, BODY).unwrap();
        assert_eq!(verify_file(&file, None, &pin(BODY)), Verdict::Verified);
        let link = root.join("link.gguf");
        symlink(&file, &link).unwrap();
        assert_eq!(verify_file(&link, None, &pin(BODY)), Verdict::Mismatch);
        let mut wrong = pin(BODY);
        wrong.sha256[0] ^= 1;
        assert_eq!(verify_file(&file, None, &wrong), Verdict::Mismatch);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn watch_verifies_after_download_appears() {
        let root = dir("watch-ok");
        let cache = root.join("hf-cache");
        let path = snapshot_path(&cache, REF).unwrap();
        let watch = ModelWatch::spawn_polling(
            path,
            Some(cache.clone()),
            pin(BODY),
            Duration::from_millis(5),
        );
        std::thread::sleep(Duration::from_millis(30));
        assert!(!watch.verified() && !watch.failed());
        hub(&cache, BODY);
        wait(&watch);
        assert!(watch.verified() && !watch.failed());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn watch_failure_is_terminal_and_removes_downloaded_cache() {
        let root = dir("watch-bad");
        let cache = root.join("hf-cache");
        let path = hub(&cache, &BODY[..10]);
        let watch = ModelWatch::spawn_polling(
            path,
            Some(cache.clone()),
            pin(BODY),
            Duration::from_millis(5),
        );
        wait(&watch);
        assert!(watch.failed() && !watch.verified());
        for _ in 0..200 {
            if !cache.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!cache.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ref_parsing_rejects_unpinned_or_traversing_refs() {
        assert!(parse_model_ref(REF).is_some());
        for bad in [
            "org/repo@main/model.gguf",
            "org/repo@0123456789012345678901234567890123456789/../model.gguf",
            "org/repo/model.gguf",
            "../repo@0123456789012345678901234567890123456789/model.gguf",
        ] {
            assert!(parse_model_ref(bad).is_none(), "{bad}");
        }
    }
}
