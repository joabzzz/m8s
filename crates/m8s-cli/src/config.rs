//! Whole-configuration intent diagnostics, never execution or approval.
use m8s_config::{Config, ValidationLimits};
use serde_json::{Value, json};
use std::{collections::BTreeSet, io::Write, path::PathBuf};

const CAP: usize = 65_536;
const SCHEMA: &str = "m8s.config-validation/v1";
const HELP: &str = "m8s config validate --config ABSOLUTE_CONFIG --catalog ABSOLUTE_CATALOG [--json]\nFlags may appear anywhere after config, once each. Validates all configured tiers and catalog profiles, not authorization or execution. Local CPU/unmeasured cannot satisfy GPU/provider requirements.\n";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Error {
    Arguments,
    Catalog,
    Document,
    Output,
}
impl Error {
    fn category(self) -> &'static str {
        match self {
            Self::Arguments => "invalid-config-arguments",
            Self::Catalog => "invalid-config-catalog",
            Self::Document => "invalid-config-document",
            Self::Output => "config-output-unavailable",
        }
    }
}
struct Options {
    config: PathBuf,
    catalog: PathBuf,
    json: bool,
}
impl Options {
    fn parse(args: &[String]) -> Result<Self, Error> {
        let (mut config, mut catalog) = (None, None);
        let (mut validate, mut json) = (false, false);
        let mut rest = args;
        while let Some((arg, tail)) = rest.split_first() {
            rest = tail;
            match arg.as_str() {
                "validate" if !validate => validate = true,
                "--json" if !json => json = true,
                "--config" | "--catalog" => {
                    let slot = if arg == "--config" {
                        &mut config
                    } else {
                        &mut catalog
                    };
                    if slot.is_some() {
                        return Err(Error::Arguments);
                    }
                    let (value, tail) = rest.split_first().ok_or(Error::Arguments)?;
                    let path = PathBuf::from(value);
                    if !path.is_absolute() || value.len() > 4096 {
                        return Err(Error::Arguments);
                    }
                    *slot = Some(path);
                    rest = tail;
                }
                _ => return Err(Error::Arguments),
            }
        }
        if !validate {
            return Err(Error::Arguments);
        }
        Ok(Self {
            config: config.ok_or(Error::Arguments)?,
            catalog: catalog.ok_or(Error::Arguments)?,
            json,
        })
    }
}
fn project(config: &Config, catalog_count: usize, limits: &ValidationLimits) -> Value {
    // Config's validated BTreeMap supplies sorted safe IDs, not raw free text.
    let tiers: Vec<_> = config
        .tiers()
        .map(|(id, tier)| json!({"id":id,"model":tier.model()}))
        .collect();
    let referenced_count = config
        .tiers()
        .map(|(_, tier)| tier.model().to_owned())
        .collect::<BTreeSet<_>>()
        .len();
    json!({"schema":SCHEMA,"status":"intent-validated-not-authorized",
        "configuration_schema":config.schema(),"tier_count":tiers.len(),"tiers":tiers,
        "catalog_model_count":catalog_count,"referenced_model_count":referenced_count,
        "budget_ceilings_micro_usd":config.budget(),
        "validation_limits_micro_usd":{"maximum_monthly":limits.maximum_monthly_micro_usd,
            "maximum_total_hourly":limits.maximum_total_hourly_micro_usd,
            "maximum_instance_hourly":limits.maximum_instance_hourly_micro_usd,"approval_evidence":false},
        "effects_performed":false,"local_execution_supported":false,"inference_served":false,
        "authorization":"not_checked","billing":"unknown","readiness":"not_assessed","requirements":"not_verified",
        "blockers":["Configuration validation is not authorization or execution.",
            "Local adapter CPU/unmeasured cannot satisfy configured GPU/provider requirements.",
            "Real model/runtime/security/budget/provider qualification remains pending."]})
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
            b"Whole configuration intent only: validated, NOT authorized or executable.\n".to_vec();
        text.extend(bytes);
        bytes = text;
    }
    bytes.push(b'\n');
    if bytes.len() > CAP {
        return Err(Error::Output);
    }
    Ok(bytes)
}
fn execute(options: &Options) -> Result<Vec<u8>, Error> {
    // Reuse both existing bounded loaders and the actual Config validation path;
    // no parallel catalog rules, selected-tier shortcut or fabricated Config.
    let catalog = super::models::load_catalog(&options.catalog).map_err(|()| Error::Catalog)?;
    let limits = ValidationLimits::default();
    let config = Config::from_path(&options.config, &catalog, &limits).map_err(|error| {
        if error == m8s_config::Error::Catalog {
            Error::Catalog
        } else {
            Error::Document
        }
    })?;
    render(
        &project(&config, catalog.models.len(), &limits),
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
    if args == ["--help"] || args == ["validate", "--help"] {
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
    fn oversized_render_refuses_before_output() {
        for json in [false, true] {
            assert_eq!(
                render(&json!({"oversized": "x".repeat(CAP)}), json),
                Err(Error::Output)
            );
        }
    }
}
