//! Startup-frozen tier membership/bounds for the hardwired MOCK control loop.
use m8s_config::{Config, ValidationLimits};
use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Error {
    Arguments,
    Configuration,
    Unsupported,
    Persisted,
    Admission,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Arguments => "invalid daemon arguments; expected state-dir [owner] with paired --config ABS --catalog ABS",
            Self::Configuration => "mock configuration unavailable or invalid",
            Self::Unsupported => "mock configuration has unsupported tier bounds or identifiers",
            Self::Persisted => "persisted mock desired state incompatible with configuration; state retained",
            Self::Admission => "configured mock tier request refused",
        })
    }
}
impl std::error::Error for Error {}
pub(super) struct Options {
    pub dir: PathBuf,
    pub owner: String,
    paths: Option<(PathBuf, PathBuf)>,
}
impl Options {
    pub fn parse(args: Vec<OsString>) -> Result<Self, Error> {
        let mut args = args.into_iter();
        let dir = args.next().ok_or(Error::Arguments)?;
        if dir.to_str().is_none_or(|s| s.starts_with("--")) {
            return Err(Error::Arguments);
        }
        let (mut owner, mut config, mut catalog) = (None, None, None);
        while let Some(arg) = args.next() {
            match arg.to_str().ok_or(Error::Arguments)? {
                "--config" | "--catalog" => {
                    let slot = if arg == "--config" {
                        &mut config
                    } else {
                        &mut catalog
                    };
                    if slot.is_some() {
                        return Err(Error::Arguments);
                    }
                    let raw = args.next().ok_or(Error::Arguments)?;
                    let text = raw.to_str().ok_or(Error::Arguments)?;
                    let path = PathBuf::from(text);
                    if !path.is_absolute() || text.len() > 4096 {
                        return Err(Error::Arguments);
                    }
                    *slot = Some(path);
                }
                s if !s.starts_with("--") && owner.is_none() => owner = Some(s.to_owned()),
                _ => return Err(Error::Arguments),
            }
        }
        let paths = match (config, catalog) {
            (None, None) => None,
            (Some(c), Some(m)) => Some((c, m)),
            _ => return Err(Error::Arguments),
        };
        Ok(Self {
            dir: dir.into(),
            owner: owner.unwrap_or_else(|| "local-mock-owner".into()),
            paths,
        })
    }
    pub fn policy(&self) -> Result<Option<Policy>, Error> {
        self.paths
            .as_ref()
            .map(|(config, catalog)| {
                let config = Config::from_paths(config, catalog, &ValidationLimits::default())
                    .map_err(|_| Error::Configuration)?;
                Policy::new(&config)
            })
            .transpose()
    }
}
pub(super) struct Policy {
    tiers: BTreeMap<String, (u32, u32)>,
}
impl Policy {
    fn new(config: &Config) -> Result<Self, Error> {
        let mut tiers = BTreeMap::new();
        for (id, tier) in config.tiers() {
            let replicas = tier.replicas();
            // Match the existing Store boundary, not a rewrite of configured IDs.
            if id.is_empty()
                || id.len() > 64
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                || replicas.max > 16
                || replicas.min > replicas.max
            {
                return Err(Error::Unsupported);
            }
            tiers.insert(
                id.to_owned(),
                (u32::from(replicas.min), u32::from(replicas.max)),
            );
        }
        if tiers.is_empty() {
            return Err(Error::Unsupported);
        }
        Ok(Self { tiers })
    }
    pub fn scale(&self, tier: &str, replicas: u32) -> Result<u32, Error> {
        let &(min, max) = self.tiers.get(tier).ok_or(Error::Admission)?;
        if replicas == 0 || (replicas >= min && replicas <= max && replicas <= 16) {
            Ok(replicas)
        } else {
            Err(Error::Admission)
        }
    }
    pub fn up(&self, tier: &str) -> Result<u32, Error> {
        let &(min, _) = self.tiers.get(tier).ok_or(Error::Admission)?;
        self.scale(tier, min.max(1))
    }
    pub fn persisted(&self, tiers: &[(String, u32)]) -> Result<(), Error> {
        for (tier, desired) in tiers {
            self.scale(tier, *desired).map_err(|_| Error::Persisted)?;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_pairs_legacy_arguments_and_pure_admission() {
        let legacy = Options::parse(vec!["relative-state".into()]).unwrap();
        assert_eq!(legacy.owner, "local-mock-owner");
        assert!(legacy.paths.is_none());
        for args in [
            vec![],
            vec!["/state", "--config", "/config"],
            vec!["/state", "--catalog", "/catalog"],
            vec!["/state", "--config", "relative", "--catalog", "/catalog"],
            vec!["/state", "--unknown"],
            vec!["/state", "owner", "extra"],
            vec![
                "/state",
                "--config",
                "/a",
                "--catalog",
                "/b",
                "--config",
                "/a",
            ],
        ] {
            assert!(Options::parse(args.into_iter().map(OsString::from).collect()).is_err());
        }
        let policy = Policy {
            tiers: BTreeMap::from([
                ("positive".into(), (2, 3)),
                ("disabled".into(), (0, 0)),
                ("optional".into(), (0, 2)),
            ]),
        };
        assert_eq!(policy.up("positive"), Ok(2));
        assert_eq!(policy.up("optional"), Ok(1));
        assert_eq!(policy.up("disabled"), Err(Error::Admission));
        for n in [0, 2, 3] {
            assert_eq!(policy.scale("positive", n), Ok(n));
        }
        for n in [1, 4, 17, u32::MAX] {
            assert!(policy.scale("positive", n).is_err());
        }
        assert!(policy.scale("unknown", 0).is_err());
        assert!(policy.persisted(&[("unknown".into(), 0)]).is_err());
        assert!(policy.persisted(&[("positive".into(), 1)]).is_err());
        assert!(
            policy
                .persisted(&[("positive".into(), 0), ("disabled".into(), 0)])
                .is_ok()
        );
    }
}
