//! Optional private daemon stop marker. Never extends the admitted deadline.
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub(super) fn validate(directory: &Option<PathBuf>) -> Result<(), &'static str> {
    use std::os::unix::fs::MetadataExt;
    if let Some(path) = directory {
        m8sd::verify_private_directory(path).map_err(|_| "prototype-control-refused")?;
        let m = std::fs::symlink_metadata(path).map_err(|_| "prototype-control-refused")?;
        if !path.is_absolute()
            || path
                .canonicalize()
                .map_err(|_| "prototype-control-refused")?
                != *path
            || !m.is_dir()
            || m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o7777 != 0o700
            || requested(path)
        {
            return Err("prototype-control-refused");
        }
    }
    Ok(())
}
pub(super) fn requested(directory: &Path) -> bool {
    // Any observation other than explicit NotFound requests cleanup. No marker
    // content can select a command, reset a claim or change a deadline.
    !matches!(std::fs::symlink_metadata(directory.join("down-requested")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound)
}
pub(super) async fn cancellation(directory: Option<PathBuf>, complete: Arc<AtomicBool>) {
    let Some(directory) = directory else {
        return std::future::pending().await;
    };
    loop {
        // Once work completes, its own bounded hold returns that completion on
        // down. Avoid racing the shutdown branch and discarding the answer.
        if complete.load(Ordering::Acquire) {
            return std::future::pending().await;
        }
        if requested(&directory) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
pub(super) async fn hold(directory: Option<PathBuf>, complete: Arc<AtomicBool>) {
    complete.store(true, Ordering::Release);
    if let Some(directory) = directory {
        eprintln!("prototype stage: completion-held-until-down-or-deadline");
        while !requested(&directory) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn control_requires_a_private_directory_and_no_prior_stop_marker() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().canonicalize().unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(validate(&Some(directory.clone())).is_ok());
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(validate(&Some(directory.clone())).is_err());
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(directory.join("down-requested"), b"down\n").unwrap();
        assert!(validate(&Some(directory)).is_err());
        assert!(validate(&None).is_ok());
    }
    #[test]
    fn stop_cancels_unfinished_work_but_preserves_the_completed_hold_path() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().canonicalize().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let complete = Arc::new(AtomicBool::new(false));
        rt.block_on(async {
            assert!(
                tokio::time::timeout(
                    Duration::from_millis(10),
                    cancellation(Some(directory.clone()), complete.clone())
                )
                .await
                .is_err()
            );
            std::fs::write(directory.join("down-requested"), b"down\n").unwrap();
            tokio::time::timeout(
                Duration::from_secs(1),
                cancellation(Some(directory.clone()), complete.clone()),
            )
            .await
            .unwrap();
            hold(Some(directory.clone()), complete.clone()).await;
            assert!(complete.load(Ordering::Acquire));
            assert!(
                tokio::time::timeout(
                    Duration::from_millis(10),
                    cancellation(Some(directory), complete)
                )
                .await
                .is_err()
            );
        });
    }
}
