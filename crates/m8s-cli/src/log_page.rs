//! Explicit one-shot page and shared safe encoding, never raw terminal rendering.
use std::{io::Write, path::Path};
const HELP: &str = "m8s --socket PATH logs --after SEQ --limit 1..200 [--json]\nBoth paging flags required once, in either order after logs. ASCII decimal cursor 0..i64::MAX. One SELECT snapshot of this state's history, not global continuity/current time/event-loss proof. Mock-only; no inference. Legacy logs without paging flags remains tail output.\nFollow: add --follow [--poll-ms 50..60000] [--max-pages 1..10000]; default poll250ms, pages include empty. logs --follow --help for limits.\n";
pub(super) fn decimal(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}
pub(super) fn run(args: &[String]) -> Result<(), &'static str> {
    if args == ["logs", "--help"] {
        return std::io::stdout()
            .lock()
            .write_all(HELP.as_bytes())
            .map_err(|_| "log-page-output-unavailable");
    }
    let json = args.iter().any(|s| s == "--json");
    let result = (|| {
        if args.len() < 3
            || args[0] != "--socket"
            || args[2] != "logs"
            || args[1].is_empty()
            || args[1].len() > 4096
            || args[1].contains('\0')
        {
            return Err("invalid-log-page-arguments");
        }
        let (mut after, mut limit, mut json_seen) = (None, None, false);
        let mut rest = &args[3..];
        while let Some((arg, tail)) = rest.split_first() {
            rest = tail;
            match arg.as_str() {
                "--json" if !json_seen => json_seen = true,
                "--after" | "--limit" => {
                    let slot = if arg == "--after" {
                        &mut after
                    } else {
                        &mut limit
                    };
                    if slot.is_some() {
                        return Err("invalid-log-page-arguments");
                    }
                    let (value, tail) = rest.split_first().ok_or("invalid-log-page-arguments")?;
                    *slot = Some(decimal(value).ok_or("invalid-log-page-arguments")?);
                    rest = tail;
                }
                _ => return Err("invalid-log-page-arguments"),
            }
        }
        let after = after
            .filter(|n| *n <= i64::MAX as u64)
            .ok_or("invalid-log-page-arguments")?;
        let limit = limit
            .filter(|n| (1..=200).contains(n))
            .ok_or("invalid-log-page-arguments")? as u32;
        let response = m8s_api::call_log_page(Path::new(&args[1]), after, limit)
            .map_err(|_| "log-page-unavailable")?;
        if !response.ok {
            return Err("log-page-refused");
        }
        encode(&response)
    })();
    let mut out = std::io::stdout().lock();
    match result {
        Ok(mut bytes) => {
            bytes.push(b'\n');
            out.write_all(&bytes)
                .map_err(|_| "log-page-output-unavailable")
        }
        Err(code) => {
            if json {
                let bytes = serde_json::to_vec(&serde_json::json!({"ok":false,"error":code}))
                    .map_err(|_| "log-page-output-unavailable")?;
                out.write_all(&bytes)
                    .and_then(|()| out.write_all(b"\n"))
                    .map_err(|_| "log-page-output-unavailable")?;
            }
            Err(code)
        }
    }
}

// Shared safe page projection; caller appends newline.
pub(super) fn encode(response: &m8s_api::LogPageResponse) -> Result<Vec<u8>, &'static str> {
    let encoded = serde_json::to_string(response).map_err(|_| "log-page-output-unavailable")?;
    crate::safe_output::ascii(&encoded).map_err(|_| "log-page-output-unavailable")
}
