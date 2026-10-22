//! Offline matching calendar events only, never scheduler activation or state.
use serde_json::json;
use std::{collections::BTreeMap, io::Write, path::Path};
const CAP: usize = 256 * 1024;
const HELP: &str = concat!(
    "m8s schedule preview --config ABS --catalog ABS --tier ID --from UNIX_SECONDS --hours 1..168 --limit 1..256 [--json]\n",
    "All flags once, any order after preview. Explicit nonnegative ASCII decimal UTC seconds <=i64MAX, no real clock. Whole inputs validated.\n",
    "Scan UTC minute instants in (from, from+hours*3600], convert to owner IANA zone; list/range/step sets intersect calendar fields, Sunday0. DOM or DOW must be LITERAL *; otherwise unsupported even if equivalent */1.\n",
    "Matching open/close events only: no current interval/state, conflict resolution, desired targets, firing policy or effects. Spring gaps omit matches; fall repeat preserves distinct UTC events, NOT a live double-fire policy. Limit truncation explicit.\n",
);
fn number(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        None
    } else {
        s.parse().ok()
    }
}
fn execute(args: &[String]) -> Result<Vec<u8>, &'static str> {
    let invalid = "invalid-schedule-arguments";
    if args.first().map(String::as_str) != Some("preview") {
        return Err(invalid);
    }
    let mut values = BTreeMap::new();
    let mut json_mode = false;
    let mut rest = &args[1..];
    while let Some((arg, tail)) = rest.split_first() {
        rest = tail;
        match arg.as_str() {
            "--json" if !json_mode => json_mode = true,
            "--config" | "--catalog" | "--tier" | "--from" | "--hours" | "--limit" => {
                let (v, tail) = rest.split_first().ok_or(invalid)?;
                if values.insert(arg.as_str(), v.as_str()).is_some() {
                    return Err(invalid);
                }
                rest = tail;
            }
            _ => return Err(invalid),
        }
    }
    if values.len() != 6 {
        return Err(invalid);
    }
    for key in ["--config", "--catalog"] {
        if !Path::new(values[key]).is_absolute() || values[key].len() > 4096 {
            return Err(invalid);
        }
    }
    let id = values["--tier"];
    if id.is_empty()
        || id.len() > 48
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(invalid);
    }
    let from = number(values["--from"])
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or(invalid)? as i64;
    let hours = number(values["--hours"])
        .filter(|n| (1..=168).contains(n))
        .ok_or(invalid)? as u16;
    let limit = number(values["--limit"])
        .filter(|n| (1..=256).contains(n))
        .ok_or(invalid)? as u16;
    let catalog = super::models::load_catalog(Path::new(values["--catalog"]))
        .map_err(|()| "invalid-schedule-configuration")?;
    let config = m8s_config::Config::from_path(
        Path::new(values["--config"]),
        &catalog,
        &m8s_config::ValidationLimits::default(),
    )
    .map_err(|_| "invalid-schedule-configuration")?;
    let preview =
        m8s_config::schedule::preview(&config, id, from, hours, limit).map_err(|e| match e {
            m8s_config::schedule::Error::Context => "schedule-context-unavailable",
            m8s_config::schedule::Error::Unsupported => "unsupported-schedule-preview",
            m8s_config::schedule::Error::Range => "schedule-range-unavailable",
        })?;
    let value = json!({"schema":"m8s.schedule-preview/v1","tier":id,"preview":preview,"semantics":"matching-events-only",
        "current_state":"not-assessed","authorization":"not-assessed","effects_performed":false,"live_firing_policy":"not-assessed"});
    let mut bytes = if json_mode {
        serde_json::to_vec(&value)
    } else {
        serde_json::to_vec_pretty(&value)
    }
    .map_err(|_| "schedule-output-unavailable")?;
    bytes.push(b'\n');
    if bytes.len() > CAP {
        return Err("schedule-output-unavailable");
    }
    Ok(bytes)
}
pub(super) fn run(args: &[String]) -> Result<(), &'static str> {
    let mut out = std::io::stdout().lock();
    if args == ["--help"] || args == ["preview", "--help"] {
        return out
            .write_all(HELP.as_bytes())
            .map_err(|_| "schedule-output-unavailable");
    }
    match execute(args) {
        Ok(bytes) => out
            .write_all(&bytes)
            .and_then(|()| out.flush())
            .map_err(|_| "schedule-output-unavailable"),
        Err(code) => {
            if args.iter().any(|a| a == "--json") {
                let mut bytes = serde_json::to_vec(
                    &json!({"schema":"m8s.schedule-preview/v1","ok":false,"error":code}),
                )
                .map_err(|_| "schedule-output-unavailable")?;
                bytes.push(b'\n');
                out.write_all(&bytes)
                    .map_err(|_| "schedule-output-unavailable")?;
            }
            Err(code)
        }
    }
}
