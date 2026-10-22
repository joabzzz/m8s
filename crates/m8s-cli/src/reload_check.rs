//! Startup-path-only Mock preflight; no apply, overrides or later success promise.
use std::{io::Write, path::Path};
const HELP: &str = "m8s --socket PATH reload-config [--json]\nm8s --socket PATH reload-config --check [--json]\nCheck validates startup-selected files and retained desired without applying. Bounds difference is membership/min/max ONLY, not whole configuration equality. Later reload can fail/change; files and desired may change. Mock admission only, not authorization or readiness.\n";
pub(super) fn run(args: &[String]) -> Result<(), &'static str> {
    if args == ["reload-config", "--help"] {
        return std::io::stdout()
            .lock()
            .write_all(HELP.as_bytes())
            .map_err(|_| "reload-check-output-unavailable");
    }
    let json = args.iter().any(|a| a == "--json");
    let result = (|| {
        if args.len() < 4
            || args.len() > 5
            || args[0] != "--socket"
            || args[2] != "reload-config"
            || args[1].is_empty()
            || args[1].len() > 4096
            || args[1].contains('\0')
            || args[3..].iter().filter(|a| a.as_str() == "--check").count() != 1
            || args[3..].iter().filter(|a| a.as_str() == "--json").count() != usize::from(json)
            || args[3..].iter().any(|a| a != "--check" && a != "--json")
        {
            return Err("invalid-reload-check-arguments");
        }
        let r = m8s_api::call_reload_check(Path::new(&args[1]))
            .map_err(|_| "reload-check-unavailable")?;
        if !r.ok {
            return Err("reload-check-refused");
        }
        let mut b = serde_json::to_vec(&r).map_err(|_| "reload-check-output-unavailable")?;
        if b.len() >= m8s_api::MAX_FRAME {
            return Err("reload-check-output-unavailable");
        }
        b.push(b'\n');
        Ok(b)
    })();
    let mut out = std::io::stdout().lock();
    match result {
        Ok(b) => out
            .write_all(&b)
            .and_then(|()| out.flush())
            .map_err(|_| "reload-check-output-unavailable"),
        Err(code) => {
            if json {
                let mut b = serde_json::to_vec(&serde_json::json!({"ok":false,"error":code}))
                    .map_err(|_| "reload-check-output-unavailable")?;
                b.push(b'\n');
                out.write_all(&b)
                    .map_err(|_| "reload-check-output-unavailable")?;
            }
            Err(code)
        }
    }
}
