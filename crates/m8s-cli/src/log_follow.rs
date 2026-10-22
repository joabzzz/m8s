//! Explicit read-only UI polling, not event continuity or automatic effect authority.
use std::{io::Write, path::Path, time::Duration};
const HELP: &str = concat!(
    "m8s --socket PATH logs --follow --after SEQ --limit 1..200 [--poll-ms 50..60000] [--max-pages 1..10000] [--json]\n",
    "Flags once, any order after logs. ASCII decimal cursor 0..i64MAX. Default poll 250ms; no max-pages means follow until externally stopped. No terminal/signal setup.\n",
    "Each successful page (including empty) is an ASCII-escaped NDJSON checkpoint in BOTH text/JSON modes. Cursor advances only after complete output and flush. has_more drains immediately; end/empty waits poll interval. Finite max-pages counts every successful emitted page, stops without extra request/sleep.\n",
    "Each fetch has one 3s deadline, NOT a session deadline. First failure stops, never retries a failed fetch. Local-history cursors do not authenticate history/generation/currentness or guarantee no loss/exactly-once delivery across restart/reset. Mock-only; no inference or effects.\n",
);
struct Options<'a> {
    socket: &'a str,
    after: u64,
    limit: u32,
    poll_ms: u64,
    max_pages: Option<u64>,
}
impl<'a> Options<'a> {
    fn parse(args: &'a [String]) -> Result<Self, &'static str> {
        let invalid = "invalid-log-follow-arguments";
        if args.len() < 3
            || args[0] != "--socket"
            || args[2] != "logs"
            || args[1].is_empty()
            || args[1].len() > 4096
            || args[1].contains('\0')
        {
            return Err(invalid);
        }
        let (mut after, mut limit, mut poll_ms, mut max_pages) = (None, None, None, None);
        let (mut follow, mut json) = (false, false);
        let mut rest = &args[3..];
        while let Some((arg, tail)) = rest.split_first() {
            rest = tail;
            match arg.as_str() {
                "--follow" if !follow => follow = true,
                "--json" if !json => json = true,
                "--after" | "--limit" | "--poll-ms" | "--max-pages" => {
                    let slot = match arg.as_str() {
                        "--after" => &mut after,
                        "--limit" => &mut limit,
                        "--poll-ms" => &mut poll_ms,
                        _ => &mut max_pages,
                    };
                    if slot.is_some() {
                        return Err(invalid);
                    }
                    let (value, tail) = rest.split_first().ok_or(invalid)?;
                    *slot = Some(super::log_page::decimal(value).ok_or(invalid)?);
                    rest = tail;
                }
                _ => return Err(invalid),
            }
        }
        if !follow
            || poll_ms.is_some_and(|n| !(50..=60000).contains(&n))
            || max_pages.is_some_and(|n| !(1..=10000).contains(&n))
        {
            return Err(invalid);
        }
        Ok(Self {
            socket: &args[1],
            after: after.filter(|n| *n <= i64::MAX as u64).ok_or(invalid)?,
            limit: limit.filter(|n| (1..=200).contains(n)).ok_or(invalid)? as u32,
            poll_ms: poll_ms.unwrap_or(250),
            max_pages,
        })
    }
}
pub(super) fn run(args: &[String]) -> Result<(), &'static str> {
    if args == ["logs", "--follow", "--help"] {
        return std::io::stdout()
            .lock()
            .write_all(HELP.as_bytes())
            .map_err(|_| "log-follow-output-unavailable");
    }
    let options = Options::parse(args)?;
    let mut cursor = options.after;
    let mut pages_left = options.max_pages;
    let mut out = std::io::stdout().lock();
    loop {
        let page = m8s_api::call_log_page(Path::new(options.socket), cursor, options.limit)
            .map_err(|_| "log-follow-unavailable")?;
        if !page.ok {
            return Err("log-follow-refused");
        }
        let mut bytes =
            super::log_page::encode(&page).map_err(|_| "log-follow-output-unavailable")?;
        bytes.push(b'\n');
        out.write_all(&bytes)
            .and_then(|()| out.flush())
            .map_err(|_| "log-follow-output-unavailable")?;
        cursor = page.next_cursor;
        if let Some(left) = &mut pages_left {
            *left -= 1;
            if *left == 0 {
                return Ok(());
            }
        }
        if !page.has_more {
            std::thread::sleep(Duration::from_millis(options.poll_ms));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_are_ui_polling_only_and_follow_is_explicit() {
        let args: Vec<_> = [
            "--socket",
            "/synthetic",
            "logs",
            "--follow",
            "--after",
            "0",
            "--limit",
            "1",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let options = Options::parse(&args).unwrap();
        assert_eq!(options.poll_ms, 250);
        assert_eq!(options.max_pages, None);
        let mut missing = args;
        missing.remove(3);
        assert!(Options::parse(&missing).is_err());
    }
}
