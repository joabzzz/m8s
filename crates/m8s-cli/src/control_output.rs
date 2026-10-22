//! Display-only validation of legacy Mock metadata, never authentication/eligibility.
use m8s_api::{Request, Response};
use m8s_core::Phase;
use std::{collections::BTreeSet, io::Write};
const UNKNOWN: &str = "control-outcome-unknown";
const OUTPUT: &str = "control-output-unavailable-outcome-unknown";
fn id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn validate(r: &Response) -> Result<(), &'static str> {
    if r.mode != "mock-only"
        || !matches!(
            r.secure_bootstrap.as_str(),
            "unavailable" | "unavailable; nodes cannot become Ready"
        )
    {
        return Err(UNKNOWN);
    }
    if !r.ok {
        return Err("control-refused-or-outcome-unknown");
    }
    if r.error.is_some() || r.tiers.len() > 256 || r.nodes.len() > 256 || r.events.len() > 200 {
        return Err(UNKNOWN);
    }
    let mut tiers = BTreeSet::new();
    for (tier, n) in &r.tiers {
        if !id(tier) || *n > 16 || !tiers.insert(tier) {
            return Err(UNKNOWN);
        }
    }
    let mut nodes = BTreeSet::new();
    for n in &r.nodes {
        if n.uid.is_empty()
            || n.uid.len() > 256
            || !nodes.insert(&n.uid)
            || !id(&n.tier)
            || n.owner.len() > 256
            || n.instance_id.as_ref().is_some_and(|s| s.len() > 256)
            || n.phase == Phase::Ready
        {
            return Err(UNKNOWN);
        }
    }
    if r.events.iter().any(|e| e.len() > 4096) {
        return Err(UNKNOWN);
    }
    Ok(())
}
// These checks bind only the returned stored snapshot to manual intent. They do
// not authenticate the peer, prove provider convergence, or permit a retry.
fn acknowledge(r: &Response, request: &Request) -> Result<(), &'static str> {
    validate(r)?;
    let consistent = match request {
        Request::Up { tier } => r.tiers.iter().any(|(id, n)| id == tier && *n > 0),
        Request::Scale { tier, replicas } => {
            r.tiers.iter().any(|(id, n)| id == tier && n == replicas)
        }
        Request::Down {} => r.tiers.iter().all(|(_, n)| *n == 0),
        Request::Shutdown {} => r.tiers.iter().all(|(_, n)| *n == 0) && r.nodes.is_empty(),
        // Read-only commands and reload have no requested desired-state target.
        _ => true,
    };
    if consistent { Ok(()) } else { Err(UNKNOWN) }
}
// Preserve printable ASCII verbatim; escape free-field controls before the shared
// non-ASCII/DEL pass. Fixed layout separators are not remote-controlled.
fn text(s: &str) -> Result<String, &'static str> {
    let mut controls = String::new();
    for c in s.chars() {
        if c.is_ascii_control() && c != '\u{7f}' {
            use std::fmt::Write;
            write!(controls, "\\u{:04x}", c as u32).map_err(|_| OUTPUT)?;
        } else {
            controls.push(c);
        }
    }
    String::from_utf8(crate::safe_output::ascii(&controls).map_err(|_| OUTPUT)?).map_err(|_| OUTPUT)
}
fn render(r: &Response, json: bool, logs: bool) -> Result<Vec<u8>, &'static str> {
    validate(r)?;
    let mut bytes = if json {
        let encoded = serde_json::to_string(r).map_err(|_| OUTPUT)?;
        crate::safe_output::ascii(&encoded).map_err(|_| OUTPUT)?
    } else {
        use std::fmt::Write;
        let mut out = format!(
            "mode: {} · secure bootstrap: {}\n",
            r.mode, r.secure_bootstrap
        );
        for (tier, n) in &r.tiers {
            writeln!(out, "{tier}: desired {n}").map_err(|_| OUTPUT)?;
            if out.len() > m8s_api::MAX_FRAME {
                return Err(OUTPUT);
            }
        }
        for n in &r.nodes {
            writeln!(
                out,
                "{} {} {:?} {}",
                n.tier,
                text(&n.uid)?,
                n.phase,
                text(n.instance_id.as_deref().unwrap_or("unresolved"))?
            )
            .map_err(|_| OUTPUT)?;
            if out.len() > m8s_api::MAX_FRAME {
                return Err(OUTPUT);
            }
        }
        if logs {
            for e in &r.events {
                writeln!(out, "{}", text(e)?).map_err(|_| OUTPUT)?;
                if out.len() > m8s_api::MAX_FRAME {
                    return Err(OUTPUT);
                }
            }
        }
        out.into_bytes()
    };
    if json {
        bytes.push(b'\n');
    }
    if bytes.len() > m8s_api::MAX_FRAME {
        return Err(OUTPUT);
    }
    Ok(bytes)
}
pub(super) fn write(r: &Response, request: &Request, json: bool) -> Result<(), &'static str> {
    acknowledge(r, request)?;
    let logs = matches!(request, Request::Logs {});
    let bytes = render(r, json, logs)?;
    let mut out = std::io::stdout().lock();
    out.write_all(&bytes)
        .and_then(|()| out.flush())
        .map_err(|_| OUTPUT)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn response() -> Response {
        Response {
            ok: true,
            mode: "mock-only".into(),
            nodes: vec![],
            tiers: vec![],
            events: vec![],
            error: None,
            secure_bootstrap: "unavailable".into(),
        }
    }
    #[test]
    fn validated_metadata_bounds_and_empty_success_remain_distinct_from_refusal() {
        let mut r = response();
        assert!(render(&r, true, false).is_ok());
        r.ok = false;
        assert_eq!(
            render(&r, true, false),
            Err("control-refused-or-outcome-unknown")
        );
        r.ok = true;
        r.tiers = (0..256).map(|i| (format!("tier{i}"), 16)).collect();
        assert!(validate(&r).is_ok());
        r.tiers.push(("extra".into(), 0));
        assert_eq!(validate(&r), Err(UNKNOWN));
        assert!(id(&"x".repeat(64)));
        assert!(!id(&"x".repeat(65)));
    }
    #[test]
    fn expansion_cap_refuses_complete_json_and_text() {
        let mut r = response();
        r.events = vec!["é".repeat(1000); 20];
        assert!(validate(&r).is_ok());
        assert_eq!(render(&r, true, true), Err(OUTPUT));
        assert_eq!(render(&r, false, true), Err(OUTPUT));
    }
    #[test]
    fn printable_ascii_is_preserved_and_free_line_separators_are_not() {
        assert_eq!(text("a \"b\" \\ path").unwrap(), "a \"b\" \\ path");
        assert_eq!(
            text("\0\n\r\t\u{7f}\u{202e}").unwrap(),
            "\\u0000\\u000a\\u000d\\u0009\\u007f\\u202e"
        );
    }
}
