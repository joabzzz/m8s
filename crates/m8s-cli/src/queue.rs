//! Offline, authored counterfactual observations; no feedback or invoker authority.
use m8s_config::{Config, ValidationLimits};
use m8s_core::queue::{Controller, Intent, Policy, QueueSample, Reason, Thresholds, Timing};
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Write, path::Path};

const SCHEMA: &str = "m8s.queue-replay/v1";
const CAP: usize = 256 * 1024;
const HELP: &str = concat!(
    "m8s queue replay --config ABS --catalog ABS --tier ID --samples ABS --freshness-seconds N --max-gap-seconds N [--json]\n",
    "All flags required once except --json; any flag order after replay. Positive ASCII decimal seconds <= i64MAX.\n",
    "Samples JSON: [\"m8s.queue-samples/v1\", [[now,desired,\"enabled\"|\"paused\",null|[observed_at,queue_wait_ms,inflight]],...]].\n",
    "1..256 positional records, <=65536 bytes; no object/free-text fields. Times are nonnegative integer Unix seconds; metrics unsigned integers.\n",
    "Authored simulation only: proposals NOT authorized. Each desired is input, NOT updated by proposals (counterfactual, no feedback). No effects, authenticated/live metrics or readiness claims.\n",
);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Error {
    Arguments,
    Config,
    Tier,
    Samples,
    Observation,
    Output,
}
impl Error {
    fn category(self) -> &'static str {
        match self {
            Self::Arguments => "invalid-queue-arguments",
            Self::Config => "invalid-queue-configuration",
            Self::Tier => "invalid-queue-tier",
            Self::Samples => "invalid-queue-samples",
            Self::Observation => "invalid-queue-observation",
            Self::Output => "queue-output-unavailable",
        }
    }
}
struct Options<'a> {
    values: BTreeMap<&'a str, &'a str>,
    json: bool,
}
impl<'a> Options<'a> {
    fn parse(args: &'a [String]) -> Result<Self, Error> {
        if args.first().map(String::as_str) != Some("replay") {
            return Err(Error::Arguments);
        }
        let mut values = BTreeMap::new();
        let mut json = false;
        let mut rest = &args[1..];
        while let Some((flag, tail)) = rest.split_first() {
            rest = tail;
            match flag.as_str() {
                "--json" if !json => json = true,
                "--config"
                | "--catalog"
                | "--tier"
                | "--samples"
                | "--freshness-seconds"
                | "--max-gap-seconds" => {
                    let (value, tail) = rest.split_first().ok_or(Error::Arguments)?;
                    if values.insert(flag.as_str(), value.as_str()).is_some() {
                        return Err(Error::Arguments);
                    }
                    rest = tail;
                }
                _ => return Err(Error::Arguments),
            }
        }
        if values.len() != 6 {
            return Err(Error::Arguments);
        }
        for key in ["--config", "--catalog", "--samples"] {
            if values[key].len() > 4096 || !Path::new(values[key]).is_absolute() {
                return Err(Error::Arguments);
            }
        }
        let tier = values["--tier"];
        if tier.is_empty()
            || tier.len() > 48
            || !tier
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        {
            return Err(Error::Arguments);
        }
        for key in ["--freshness-seconds", "--max-gap-seconds"] {
            seconds(values[key])?;
        }
        Ok(Self { values, json })
    }
}
fn seconds(value: &str) -> Result<u64, Error> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::Arguments);
    }
    let n = value.parse::<u64>().map_err(|_| Error::Arguments)?;
    if n == 0 || n > i64::MAX as u64 {
        return Err(Error::Arguments);
    }
    Ok(n)
}
fn tuple(value: &Value, len: usize) -> Result<&[Value], Error> {
    let array = value.as_array().ok_or(Error::Samples)?;
    if array.len() != len {
        return Err(Error::Samples);
    }
    Ok(array)
}
fn unsigned(value: &Value) -> Result<u64, Error> {
    value.as_u64().ok_or(Error::Samples)
}
fn timestamp(value: &Value) -> Result<i64, Error> {
    i64::try_from(unsigned(value)?).map_err(|_| Error::Samples)
}
fn replay(options: &Options<'_>) -> Result<Value, Error> {
    let v = &options.values;
    let catalog =
        super::models::load_catalog(Path::new(v["--catalog"])).map_err(|()| Error::Config)?;
    let config = Config::from_path(
        Path::new(v["--config"]),
        &catalog,
        &ValidationLimits::default(),
    )
    .map_err(|_| Error::Config)?;
    let tier = config.tier(v["--tier"]).ok_or(Error::Tier)?;
    let queue = tier.queue().ok_or(Error::Tier)?;
    let replicas = tier.replicas();
    let thresholds = Thresholds {
        queue_wait_ms_gt: queue.queue_wait_ms_gt(),
        inflight_gt: queue.inflight_gt(),
    };
    let timing = Timing {
        scale_up_for_seconds: queue.scale_up_for_seconds(),
        scale_down_idle_seconds: queue.scale_down_idle_seconds(),
        maximum_sample_gap_seconds: seconds(v["--max-gap-seconds"])?,
        freshness_seconds: seconds(v["--freshness-seconds"])?,
    };
    let policy = Policy::new(replicas.min.into(), replicas.max.into(), thresholds, timing)
        .map_err(|_| Error::Tier)?;
    let bytes =
        super::models::load_bytes(Path::new(v["--samples"])).map_err(|()| Error::Samples)?;
    // Positional arrays deliberately have no object keys to duplicate or free text to echo.
    let document: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Samples)?;
    let root = tuple(&document, 2)?;
    if root[0].as_str() != Some("m8s.queue-samples/v1") {
        return Err(Error::Samples);
    }
    let records = root[1].as_array().ok_or(Error::Samples)?;
    if records.is_empty() || records.len() > 256 {
        return Err(Error::Samples);
    }
    let mut controller = Controller::new(policy);
    let mut results = Vec::with_capacity(records.len());
    for (index, record) in records.iter().enumerate() {
        let r = tuple(record, 4)?;
        let now = timestamp(&r[0])?;
        let desired = u32::try_from(unsigned(&r[1])?).map_err(|_| Error::Samples)?;
        let intent = match r[2].as_str() {
            Some("enabled") => Intent::Enabled,
            Some("paused") => Intent::Paused,
            _ => return Err(Error::Samples),
        };
        let sample = if r[3].is_null() {
            None
        } else {
            let s = tuple(&r[3], 3)?;
            Some(QueueSample {
                observed_at: timestamp(&s[0])?,
                queue_wait_ms: unsigned(&s[1])?,
                inflight: unsigned(&s[2])?,
            })
        };
        let proposal = controller
            .observe(now, desired, intent, sample)
            .map_err(|_| Error::Observation)?;
        results.push(json!({"index":index,"proposal":proposal.map(|p|json!({"target":p.target,"reason":match p.reason { Reason::SustainedPressure => "sustained-pressure", Reason::SustainedIdle => "sustained-idle" }}))}));
    }
    Ok(
        json!({"schema":SCHEMA,"ok":true,"simulation":"authored-counterfactual-no-feedback","authorization":"proposals-not-authorized",
        "effects_performed":false,"metrics_authenticity":"not-assessed","live_currentness":"not-assessed","readiness":"not-assessed",
        "policy":{"tier":v["--tier"],"min":replicas.min,"max":replicas.max,"queue_wait_ms_gt":thresholds.queue_wait_ms_gt,
        "inflight_gt":thresholds.inflight_gt,"scale_up_for_seconds":timing.scale_up_for_seconds,"scale_down_idle_seconds":timing.scale_down_idle_seconds,
        "freshness_seconds":timing.freshness_seconds,"maximum_sample_gap_seconds":timing.maximum_sample_gap_seconds},"records":results}),
    )
}
fn render(value: &Value, json: bool) -> Result<Vec<u8>, Error> {
    let mut bytes = if json {
        serde_json::to_vec(value)
    } else {
        serde_json::to_vec_pretty(value)
    }
    .map_err(|_| Error::Output)?;
    bytes.push(b'\n');
    if bytes.len() > CAP {
        return Err(Error::Output);
    }
    Ok(bytes)
}
fn write(bytes: &[u8]) -> Result<(), Error> {
    let mut out = std::io::stdout().lock();
    out.write_all(bytes)
        .and_then(|()| out.flush())
        .map_err(|_| Error::Output)
}
pub(super) fn run(args: &[String]) -> Result<(), &'static str> {
    if args == ["--help"] || args == ["replay", "--help"] {
        return write(HELP.as_bytes()).map_err(Error::category);
    }
    let result = Options::parse(args).and_then(|o| replay(&o).and_then(|v| render(&v, o.json)));
    match result {
        Ok(bytes) => write(&bytes).map_err(Error::category),
        Err(error) => {
            if args.iter().any(|a| a == "--json") {
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
