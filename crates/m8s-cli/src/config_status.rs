//! One actual active Mock admission snapshot, no file/Store/default discovery.
use std::{io::Write, path::Path};
const HELP: &str = "m8s --socket PATH config-status [--json]\nOne active in-memory Mock admission snapshot, not desired state, file validation, authentication, runtime readiness or authorization. No file reread; configured=false means no configured policy, not no desired state.\n";
pub(super) fn run(args: &[String]) -> Result<(), &'static str> {
    if args == ["config-status", "--help"] {
        return std::io::stdout()
            .lock()
            .write_all(HELP.as_bytes())
            .map_err(|_| "config-status-output-unavailable");
    }
    let json = args.iter().any(|a| a == "--json");
    let result = (|| {
        if !(args.len() == 3 || (args.len() == 4 && args[3] == "--json"))
            || args[0] != "--socket"
            || args[2] != "config-status"
            || args[1].is_empty()
            || args[1].len() > 4096
            || args[1].contains('\0')
        {
            return Err("invalid-config-status-arguments");
        }
        let response = m8s_api::call_config_status(Path::new(&args[1]))
            .map_err(|_| "config-status-unavailable")?;
        if !response.ok {
            return Err("config-status-refused");
        }
        // Only checked fixed labels, ASCII IDs and bounded integers survive.
        let mut bytes =
            serde_json::to_vec(&response).map_err(|_| "config-status-output-unavailable")?;
        if bytes.len() >= m8s_api::MAX_FRAME {
            return Err("config-status-output-unavailable");
        }
        bytes.push(b'\n');
        Ok(bytes)
    })();
    let mut out = std::io::stdout().lock();
    match result {
        Ok(bytes) => out
            .write_all(&bytes)
            .map_err(|_| "config-status-output-unavailable"),
        Err(code) => {
            if json {
                let bytes = serde_json::to_vec(&serde_json::json!({"ok":false,"error":code}))
                    .map_err(|_| "config-status-output-unavailable")?;
                out.write_all(&bytes)
                    .and_then(|()| out.write_all(b"\n"))
                    .map_err(|_| "config-status-output-unavailable")?;
            }
            Err(code)
        }
    }
}
