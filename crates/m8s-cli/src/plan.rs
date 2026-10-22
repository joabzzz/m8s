//! Bounded owner-intent diagnostics only. No execution capability is constructed.
use m8s_config::{ColdBehavior, Config, ValidationLimits};
use m8s_models::ModelCatalog;
use serde_json::{Value, json};
use std::{io::Write, path::PathBuf};

const CAP: usize = 65_536;
const SCHEMA: &str = "m8s.local-intent-plan/v1";
const HELP: &str = "m8s plan --local-development --config ABSOLUTE_CONFIG --catalog ABSOLUTE_CATALOG --tier ID [--json]\nValidates owner intent only; not authorized or executable. Local CPU/unmeasured cannot satisfy GPU/provider requirements.\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Error {
    Arguments,
    Catalog,
    Config,
    Tier,
    Output,
}
impl Error {
    fn category(self) -> &'static str {
        match self {
            Self::Arguments => "invalid-plan-arguments",
            Self::Catalog => "invalid-plan-catalog",
            Self::Config => "invalid-plan-configuration",
            Self::Tier => "unknown-plan-tier",
            Self::Output => "plan-output-unavailable",
        }
    }
}
struct Options {
    config: PathBuf,
    catalog: PathBuf,
    tier: String,
    json: bool,
}
impl Options {
    fn parse(args: &[String]) -> Result<Self, Error> {
        let (mut config, mut catalog, mut tier) = (None, None, None);
        let (mut local, mut json) = (false, false);
        let mut rest = args;
        while let Some((arg, tail)) = rest.split_first() {
            rest = tail;
            match arg.as_str() {
                "--local-development" if !local => local = true,
                "--json" if !json => json = true,
                "--config" | "--catalog" | "--tier" => {
                    let slot = match arg.as_str() {
                        "--config" => &mut config,
                        "--catalog" => &mut catalog,
                        _ => &mut tier,
                    };
                    if slot.is_some() {
                        return Err(Error::Arguments);
                    }
                    let (value, tail) = rest.split_first().ok_or(Error::Arguments)?;
                    if value.is_empty() || value.starts_with('-') {
                        return Err(Error::Arguments);
                    }
                    *slot = Some(value.clone());
                    rest = tail;
                }
                _ => return Err(Error::Arguments),
            }
        }
        let config = PathBuf::from(config.ok_or(Error::Arguments)?);
        let catalog = PathBuf::from(catalog.ok_or(Error::Arguments)?);
        let tier = tier.ok_or(Error::Arguments)?;
        if !local
            || !config.is_absolute()
            || !catalog.is_absolute()
            || tier.len() > 48
            || !tier.as_bytes()[0].is_ascii_lowercase()
            || !tier
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_.".contains(&b))
        {
            return Err(Error::Arguments);
        }
        Ok(Self {
            config,
            catalog,
            tier,
            json,
        })
    }
}

fn load_catalog(path: &std::path::Path) -> Result<ModelCatalog, Error> {
    super::models::load_catalog(path).map_err(|()| Error::Catalog)
}

// Borrowed immutable validated views: not a Deserialize-able plan or authority.
struct IntentSummary<'a> {
    config: &'a Config,
    tier_id: &'a str,
    tier: m8s_config::Tier<'a>,
    limits: &'a ValidationLimits,
}
fn project<'a>(
    config: &'a Config,
    selected: &'a str,
    limits: &'a ValidationLimits,
) -> Result<IntentSummary<'a>, Error> {
    Ok(IntentSummary {
        config,
        tier_id: selected,
        tier: config.tier(selected).ok_or(Error::Tier)?,
        limits,
    })
}
impl IntentSummary<'_> {
    fn value(&self) -> Value {
        let c = self.config;
        let t = self.tier;
        let cold = match t.cold() {
            ColdBehavior::Wait { timeout_seconds } => {
                json!({"mode":"wait", "timeout_seconds":timeout_seconds})
            }
            ColdBehavior::Reject => json!({"mode":"reject"}),
            ColdBehavior::Fallback { tier } => json!({"mode":"fallback", "tier":tier}),
        };
        let queue = t.queue().map(|q| json!({"queue_wait_ms_gt":q.queue_wait_ms_gt(), "inflight_gt":q.inflight_gt(), "scale_up_for_seconds":q.scale_up_for_seconds(), "scale_down_idle_seconds":q.scale_down_idle_seconds()}));
        let schedules: Vec<_> = t
            .schedules()
            .map(|s| json!({"open":s.open(),"close":s.close()}))
            .collect();
        json!({
            "schema":SCHEMA, "mode":"local-development-dry-run", "status":"intent-validated-not-authorized",
            "effects_performed":false, "local_execution_supported":false, "inference_served":false,
            "authorization":"not_checked", "billing":"unknown", "readiness":"not_assessed",
            "requirements":"not_verified",
            "selected_tier":self.tier_id, "model":t.model(), "replicas":t.replicas(),
            "gpu_requirements":t.gpu(), "provider_requirements":t.providers(),
            "interruptible":t.interruptible(), "manual":t.manual(),
            "budget_ceilings_micro_usd":c.budget(),
            "validation_limits_micro_usd":{
                "maximum_monthly":self.limits.maximum_monthly_micro_usd,
                "maximum_total_hourly":self.limits.maximum_total_hourly_micro_usd,
                "maximum_instance_hourly":self.limits.maximum_instance_hourly_micro_usd,
                "approval_evidence":false},
            "dms_duration_descriptions":{"ttl_seconds":c.dms().ttl_seconds,"max_lifetime_seconds":c.dms().max_lifetime_seconds,"watchdog_guarantee":false},
            "policy_requirements":{"minimum_down_mbps":c.minimum_down_mbps(),"signed_image_required":c.signed_image_required(),"unknown_location_rejected":c.unknown_location_rejected(),"verified":false},
            "behavior_descriptions":{"descriptive_not_executed":true,"cold":cold,"queue":queue,"schedules":schedules,"timezone":c.timezone()},
            "blockers":["Local adapter CPU/unmeasured cannot satisfy configured GPU/provider requirements.","No canonical command, journal, target fingerprint or Store intent is created; this command cannot execute.","Real model/runtime/security/budget/provider qualification remains pending."]
        })
    }
    fn render(&self, json: bool) -> Result<Vec<u8>, Error> {
        let value = self.value();
        let mut output = if json {
            serde_json::to_vec(&value)
        } else {
            serde_json::to_vec_pretty(&value)
        }
        .map_err(|_| Error::Output)?;
        if !json {
            let mut text =
                b"Owner intent only: validated, NOT authorized or executable.\n".to_vec();
            text.extend(output);
            output = text;
        }
        output.push(b'\n');
        if output.len() > CAP {
            return Err(Error::Output);
        }
        Ok(output)
    }
}
fn execute(options: Options) -> Result<Vec<u8>, Error> {
    let catalog = load_catalog(&options.catalog)?;
    let limits = ValidationLimits::default();
    let config =
        Config::from_path(&options.config, &catalog, &limits).map_err(|_| Error::Config)?;
    project(&config, &options.tier, &limits)?.render(options.json)
}
fn write(output: &[u8]) -> Result<(), Error> {
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(output)
        .and_then(|()| stdout.flush())
        .map_err(|_| Error::Output)
}
pub(super) fn run(args: &[String]) -> Result<(), &'static str> {
    if args == ["--help"] {
        return write(HELP.as_bytes()).map_err(Error::category);
    }
    let result = Options::parse(args).and_then(execute);
    match result {
        Ok(output) => write(&output).map_err(Error::category),
        Err(error) => {
            if args.iter().any(|s| s == "--json") {
                let output = serde_json::to_vec(
                    &json!({"schema":SCHEMA,"ok":false,"error":error.category()}),
                )
                .map_err(|_| Error::Output.category())?;
                let mut output = output;
                output.push(b'\n');
                write(&output).map_err(Error::category)?;
            }
            Err(error.category())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pure_projection_and_bounded_rendering() {
        let catalog: ModelCatalog = toml::from_str(m8s_models::EXAMPLE_CATALOG).unwrap();
        let limits = ValidationLimits::default();
        let config = Config::parse(m8s_config::EXAMPLE.as_bytes(), &catalog, &limits).unwrap();
        let summary = project(&config, "small", &limits).unwrap();
        assert_eq!(summary.render(true).unwrap(), summary.render(true).unwrap());
        assert!(summary.render(false).unwrap().len() <= CAP);
        assert_eq!(summary.value()["local_execution_supported"], false);
        assert!(matches!(
            project(&config, "absent", &limits),
            Err(Error::Tier)
        ));
    }
    #[test]
    fn parser_has_no_defaults_or_overrides() {
        let base = [
            "--local-development",
            "--config",
            "/config",
            "--catalog",
            "/catalog",
            "--tier",
            "small",
        ]
        .map(str::to_owned);
        assert!(Options::parse(&base).is_ok());
        for flag in [
            "--execute",
            "--approve",
            "--socket",
            "--json",
            "--config",
            "--local-development",
        ] {
            let mut args = base.to_vec();
            args.extend([flag.to_owned(), flag.to_owned()]);
            assert!(matches!(Options::parse(&args), Err(Error::Arguments)));
        }
        for i in 0..base.len() {
            let mut args = base.to_vec();
            args.remove(i);
            assert!(Options::parse(&args).is_err());
        }
    }
}
