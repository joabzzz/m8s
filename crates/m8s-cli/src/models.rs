//! Explicit bounded catalog metadata discovery; no model/runtime authority.
use m8s_core::models::ModelCatalog;
use serde_json::{Value, json};
use std::{io::Write, path::PathBuf};

#[cfg(unix)]
const CAP: usize = 65_536;
const OUTPUT_CAP: usize = 256 * 1024;
const SCHEMA: &str = "m8s.model-catalog/v1";
const HELP: &str = "m8s models --catalog ABSOLUTE_CATALOG list [--json]\nm8s models --catalog ABSOLUTE_CATALOG show ID [--json]\nFlags may appear before or after the command/ID, once each. Explicit catalog only. Metadata is not runtime verification, license permission or deployment approval.\n";

#[cfg(unix)]
pub(super) fn load_bytes(path: &std::path::Path) -> Result<Vec<u8>, ()> {
    use std::{io::Read, os::unix::fs::OpenOptionsExt};
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| ())?;
    let metadata = file.metadata().map_err(|_| ())?;
    if !metadata.is_file() || metadata.len() > CAP as u64 {
        return Err(());
    }
    let mut bytes = Vec::new();
    file.take(CAP as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() > CAP {
        return Err(());
    }
    Ok(bytes)
}
#[cfg(not(unix))]
pub(super) fn load_bytes(_: &std::path::Path) -> Result<Vec<u8>, ()> {
    Err(())
}

pub(super) fn load_catalog(path: &std::path::Path) -> Result<ModelCatalog, ()> {
    let bytes = load_bytes(path)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| ())?;
    toml::from_str(text).map_err(|_| ())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Error {
    Arguments,
    Catalog,
    Model,
    Output,
}
impl Error {
    fn category(self) -> &'static str {
        match self {
            Self::Arguments => "invalid-models-arguments",
            Self::Catalog => "invalid-models-catalog",
            Self::Model => "unknown-model-id",
            Self::Output => "models-output-unavailable",
        }
    }
}
struct Options {
    catalog: PathBuf,
    selected: Option<String>,
    json: bool,
}
impl Options {
    fn parse(args: &[String]) -> Result<Self, Error> {
        let (mut catalog, mut json) = (None, false);
        let mut positional = Vec::new();
        let mut rest = args;
        while let Some((arg, tail)) = rest.split_first() {
            rest = tail;
            match arg.as_str() {
                "--catalog" if catalog.is_none() => {
                    let (value, tail) = rest.split_first().ok_or(Error::Arguments)?;
                    let path = PathBuf::from(value);
                    if !path.is_absolute() || value.len() > 4096 {
                        return Err(Error::Arguments);
                    }
                    catalog = Some(path);
                    rest = tail;
                }
                "--json" if !json => json = true,
                value if !value.starts_with('-') && positional.len() < 2 => {
                    positional.push(value);
                }
                _ => return Err(Error::Arguments),
            }
        }
        let selected = match positional.as_slice() {
            ["list"] => None,
            ["show", id]
                if !id.is_empty()
                    && id.len() <= 96
                    && id.as_bytes()[0].is_ascii_lowercase()
                    && id.bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_.".contains(&b)
                    }) =>
            {
                Some((*id).to_owned())
            }
            _ => return Err(Error::Arguments),
        };
        Ok(Self {
            catalog: catalog.ok_or(Error::Arguments)?,
            selected,
            json,
        })
    }
}
fn project(catalog: &ModelCatalog, selected: Option<&str>) -> Result<Value, Error> {
    // Global validation precedes even lookup: invalid unselected profiles fail.
    m8s_config::validate_catalog(catalog).map_err(|_| Error::Catalog)?;
    let mut models: Vec<_> = catalog.models.iter().collect();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    if let Some(id) = selected {
        models.retain(|model| model.id == id);
        if models.is_empty() {
            return Err(Error::Model);
        }
    }
    let models: Vec<_> = models
        .into_iter()
        .map(|model| {
            json!({
                "id":model.id, "params_billions":model.params_billions,
                "weights_bytes":model.weights_bytes,
                "runtime_overhead_bytes_per_replica":model.runtime_overhead_bytes_per_replica,
                "kv_bytes_per_1024_tokens":model.kv_bytes_per_1024_tokens,
                "max_context_tokens":model.max_context_tokens,
                "declared_license_class":model.license_class,
                "declared_runtime_status":model.runtime_status,
            })
        })
        .collect();
    Ok(json!({"schema":SCHEMA, "catalog_metadata_only":true,
        "runtime_verification":"not-performed", "deployment":"not-assessed",
        "approval":"not-assessed", "models":models}))
}
fn render(value: &Value, json: bool) -> Result<Vec<u8>, Error> {
    let mut bytes = if json {
        serde_json::to_vec(value)
    } else {
        serde_json::to_vec_pretty(value)
    }
    .map_err(|_| Error::Output)?;
    if !json {
        let mut text =
            b"Catalog-declared metadata only; exact bytes, NOT runtime verification or approval.\n"
                .to_vec();
        text.extend(bytes);
        bytes = text;
    }
    bytes.push(b'\n');
    if bytes.len() > OUTPUT_CAP {
        return Err(Error::Output);
    }
    Ok(bytes)
}
fn execute(options: &Options) -> Result<Vec<u8>, Error> {
    let catalog = load_catalog(&options.catalog).map_err(|()| Error::Catalog)?;
    render(
        &project(&catalog, options.selected.as_deref())?,
        options.json,
    )
}
fn write(bytes: &[u8]) -> Result<(), Error> {
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(bytes)
        .and_then(|()| stdout.flush())
        .map_err(|_| Error::Output)
}
pub(super) fn run(args: &[String]) -> Result<(), &'static str> {
    if args.first().is_some_and(|arg| arg == "fit") {
        return super::fit::run(&args[1..]);
    }
    if args == ["--help"] {
        return write(HELP.as_bytes()).map_err(Error::category);
    }
    match Options::parse(args).and_then(|options| execute(&options)) {
        Ok(bytes) => write(&bytes).map_err(Error::category),
        Err(error) => {
            if args.iter().any(|arg| arg == "--json") {
                write(
                    &render(
                        &json!({"schema":SCHEMA,"ok":false,"error":error.category()}),
                        true,
                    )
                    .map_err(Error::category)?,
                )
                .map_err(Error::category)?;
            }
            Err(error.category())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maximum_valid_projection_fits_and_oversized_render_refuses() {
        let mut catalog: ModelCatalog = toml::from_str(m8s_core::models::EXAMPLE_CATALOG).unwrap();
        let mut profile = catalog.models.remove(0);
        profile.params_billions = u32::MAX;
        profile.weights_bytes = 10_000_000_000_000_000_000;
        profile.runtime_overhead_bytes_per_replica = u64::MAX - profile.weights_bytes;
        profile.kv_bytes_per_1024_tokens = u64::MAX;
        profile.max_context_tokens = u64::MAX;
        profile.license_class = "non-commercial".into();
        profile.runtime_status = "community".into();
        catalog.models = (0..m8s_config::MAX_MODELS)
            .map(|i| {
                let mut model = profile.clone();
                model.id = format!("m{i:095}");
                model
            })
            .collect();
        let value = project(&catalog, None).unwrap();
        for json in [true, false] {
            assert!(render(&value, json).unwrap().len() <= OUTPUT_CAP);
            assert_eq!(
                render(&json!({"oversized": "x".repeat(OUTPUT_CAP)}), json),
                Err(Error::Output)
            );
        }
    }
}
