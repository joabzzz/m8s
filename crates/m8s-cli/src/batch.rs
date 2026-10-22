//! Atomic manual desired request, not atomic provider effects or an execution permit.
use std::{collections::BTreeMap, io::Write, path::Path};
const HELP: &str = "m8s --socket PATH scale-batch TIER=N [TIER=N ...] [--json]\n1..32 unique ASCII alnum/_/- tier IDs<=64; decimal0..16. --json once, anywhere after command. Unmentioned tiers unchanged, explicit zero stops. Mock-only: atomic desired+nonce cancellation, NOT provider effects/convergence/Ready. A lost/failed reply may follow commit; never automatically retry.\n";
fn execute(args: &[String]) -> Result<Vec<u8>, &'static str> {
    let invalid = "invalid-scale-batch-arguments";
    if args.len() < 4
        || args[0] != "--socket"
        || args[2] != "scale-batch"
        || args[1].is_empty()
        || args[1].len() > 4096
        || args[1].contains('\0')
    {
        return Err(invalid);
    }
    let mut requested = BTreeMap::new();
    let mut json_seen = false;
    for arg in &args[3..] {
        if arg == "--json" && !json_seen {
            json_seen = true;
            continue;
        }
        let (id, n) = arg.split_once('=').ok_or(invalid)?;
        if id.is_empty()
            || id.len() > 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || n.is_empty()
            || !n.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(invalid);
        }
        let n = n.parse::<u32>().ok().filter(|n| *n <= 16).ok_or(invalid)?;
        if requested.insert(id.to_owned(), n).is_some() || requested.len() > 32 {
            return Err(invalid);
        }
    }
    if requested.is_empty() {
        return Err(invalid);
    }
    let targets: Vec<_> = requested
        .iter()
        .map(|(id, n)| m8s_api::ScaleTarget {
            tier: id.clone(),
            replicas: *n,
        })
        .collect();
    let response = m8s_api::call(
        Path::new(&args[1]),
        &m8s_api::Request::ScaleBatch {
            targets: targets.clone(),
        },
    )
    .map_err(|_| "scale-batch-outcome-unknown")?;
    if response.mode != "mock-only" {
        return Err("scale-batch-outcome-unknown");
    }
    if !response.ok {
        return Err("scale-batch-refused-or-outcome-unknown");
    }
    if response.error.is_some() || response.tiers.len() > 256 {
        return Err("scale-batch-outcome-unknown");
    }
    let mut desired = BTreeMap::new();
    for (id, n) in response.tiers {
        if id.is_empty()
            || id.len() > 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || n > 16
            || desired.insert(id, n).is_some()
        {
            return Err("scale-batch-outcome-unknown");
        }
    }
    if requested.iter().any(|(id, n)| desired.get(id) != Some(n)) {
        return Err("scale-batch-outcome-unknown");
    }
    let desired: Vec<_> = desired
        .into_iter()
        .map(|(tier, replicas)| m8s_api::ScaleTarget { tier, replicas })
        .collect();
    let value = serde_json::json!({"schema":"m8s.scale-batch/v1","mode":"mock-only","requested":targets,"observed_desired":desired,
        "atomicity":"desired-and-explicit-zero-nonce-cancellation-only","provider_convergence":"not-assessed","readiness":"not-assessed","authorization":"not-assessed"});
    let mut b = serde_json::to_vec(&value).map_err(|_| "scale-batch-output-unavailable")?;
    b.push(b'\n');
    if b.len() > m8s_api::MAX_FRAME {
        return Err("scale-batch-output-unavailable");
    }
    Ok(b)
}
pub(super) fn run(args: &[String]) -> Result<(), &'static str> {
    let mut out = std::io::stdout().lock();
    if args == ["scale-batch", "--help"] {
        return out
            .write_all(HELP.as_bytes())
            .map_err(|_| "scale-batch-output-unavailable");
    }
    let bytes = execute(args)?;
    out.write_all(&bytes)
        .and_then(|()| out.flush())
        .map_err(|_| "scale-batch-output-unavailable")
}
