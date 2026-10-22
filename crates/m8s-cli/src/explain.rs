//! Stored observations only, not complete reconcile eligibility or a launch permit.
use std::{io::Write, path::Path};
const HELP: &str = "m8s --socket PATH explain-tier TIER [--json]\nOne consistent stored snapshot/evaluation of existing 3-launches/tier/hour and future-attempt gate ONLY. desired=null means no persisted intent, NOT zero. All stored nodes/phases/deleting are DB metadata, NOT provider evidence. Strict cutoff at>evaluated_at.saturating_sub(3600), including future attempts in count; any future attempt blocks rate availability. Evaluated time is daemon calculation input, not delay/currentness/clock proof. Configured context from active memory only; missing context is not admission. NOT overall eligibility, a will-launch prediction, authorization, provider absence or Ready.\n";
pub(super) fn run(args: &[String]) -> Result<(), &'static str> {
    let mut out = std::io::stdout().lock();
    if args == ["explain-tier", "--help"] {
        return out
            .write_all(HELP.as_bytes())
            .map_err(|_| "explain-tier-output-unavailable");
    }
    if !(args.len() == 4 || (args.len() == 5 && args[4] == "--json"))
        || args[0] != "--socket"
        || args[2] != "explain-tier"
        || args[1].is_empty()
        || args[1].len() > 4096
        || args[1].contains('\0')
        || args[3].is_empty()
        || args[3].len() > 64
        || !args[3]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err("invalid-explain-tier-arguments");
    }
    let response = m8s_api::call_explain_tier(Path::new(&args[1]), &args[3])
        .map_err(|_| "explain-tier-unavailable")?;
    if !response.ok {
        return Err("explain-tier-refused");
    }
    let mut bytes = serde_json::to_vec(&response).map_err(|_| "explain-tier-output-unavailable")?;
    bytes.push(b'\n');
    if bytes.len() > m8s_api::MAX_FRAME {
        return Err("explain-tier-output-unavailable");
    }
    out.write_all(&bytes)
        .and_then(|()| out.flush())
        .map_err(|_| "explain-tier-output-unavailable")
}
