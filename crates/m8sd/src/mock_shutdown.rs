//! Only the hardwired Mock lifecycle; not a generic/provider shutdown authority.
use m8s_provider::{Mock, Provider};
use m8sd::{Error, Result, Store};

pub(super) fn shutdown(store: &mut Store, provider: &mut Mock, at: i64) -> Result<()> {
    // Durable cancellation/deletion intent commits BEFORE any provider effect.
    store.down()?;
    // Fixed bound, no retry/poll loop. Only existing reconcile may delete a row,
    // after its own fresh per-instance positive absence, never destroy ACK alone.
    store.reconcile(provider, at)?;
    store.reconcile(provider, at)?;
    if !provider.list_owned(store.owner())?.is_empty()
        || !store.nodes()?.is_empty()
        || store.tiers()?.iter().any(|(_, desired)| *desired != 0)
    {
        return Err(Error::Invalid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use m8s_provider::Fault;
    use std::{
        fs,
        os::unix::fs::DirBuilderExt,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Root(PathBuf);
    impl Root {
        fn new() -> Self {
            let path = fs::canonicalize("/tmp").unwrap().join(format!(
                "m8sh-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }
        fn active(&self) -> (Store, Mock) {
            let mut store =
                Store::open(self.0.join("state.db"), "synthetic-shutdown-owner").unwrap();
            let mut provider = Mock::default();
            store.scale("test", 1).unwrap();
            store.reconcile(&mut provider, 100).unwrap();
            assert_eq!(store.nodes().unwrap().len(), 1);
            (store, provider)
        }
    }
    impl Drop for Root {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn destroy_ack_and_faults_never_claim_absence_or_reset_history() {
        let root = Root::new();
        let (mut store, mut provider) = root.active();
        provider.defer_destroy = true;
        assert!(shutdown(&mut store, &mut provider, 101).is_err());
        assert_eq!(provider.destroy_calls.len(), 2);
        assert_eq!(store.nodes().unwrap().len(), 1);
        assert_eq!(store.tiers().unwrap(), vec![("test".into(), 0)]);
        assert_eq!(store.launch_count("test", 101).unwrap(), 1);
        provider.defer_destroy = false;
        assert!(shutdown(&mut store, &mut provider, 102).is_ok());
        assert!(store.nodes().unwrap().is_empty());
        assert_eq!(store.launch_count("test", 102).unwrap(), 1);
        for faults in [
            vec![Fault::List],
            vec![Fault::Describe, Fault::Describe],
            vec![Fault::Destroy, Fault::Destroy],
        ] {
            let root = Root::new();
            let (mut store, mut provider) = root.active();
            provider.faults = faults;
            assert!(shutdown(&mut store, &mut provider, 101).is_err());
            assert!(provider.destroy_calls.len() <= 2);
            assert!(!store.nodes().unwrap().is_empty());
            assert_eq!(store.tiers().unwrap(), vec![("test".into(), 0)]);
            assert_eq!(store.launch_count("test", 101).unwrap(), 1);
        }
    }
    #[test]
    fn confirmation_is_scoped_to_exact_store_owner() {
        let root = Root::new();
        let (mut store, mut provider) = root.active();
        provider
            .insert(m8s_core::Instance {
                id: "synthetic-foreign".into(),
                owner: "other-synthetic-owner".into(),
                node_uid: "foreign-node".into(),
                tier: "foreign".into(),
                state: m8s_core::InstanceState::Running,
            })
            .unwrap();
        shutdown(&mut store, &mut provider, 101).unwrap();
        assert!(provider.list_owned(store.owner()).unwrap().is_empty());
        assert_eq!(
            provider.list_owned("other-synthetic-owner").unwrap().len(),
            1
        );
        assert!(
            !provider
                .destroy_calls
                .iter()
                .any(|id| id == "synthetic-foreign")
        );
    }
}
