mod batch;
mod config;
mod config_status;
mod explain;
mod fit;
mod init;
mod log_follow;
mod log_page;
mod models;
mod plan;
mod queue;
mod reload_check;
mod schedule;
mod secrets;
use m8s_api::Request;
const HELP: &str = concat!(
    "m8s command guide\n",
    "  init --mock --directory ABS [--json]     Fresh Unix Mock starter files only\n",
    "  models --catalog ABS list [--json]       Catalog-declared metadata only\n",
    "  models --catalog ABS show ID [--json]    One catalog profile, not runtime verification\n",
    "  models fit --help                       Offline capacity estimate, not a live offer\n",
    "  config validate --help                  Whole configuration intent, not authorization\n",
    "  schedule preview --help                 Offline matching calendar events only\n",
    "  queue replay --help                     Offline queue proposals, no effects\n",
    "  plan --help                             Effect-free intent, not authorization\n",
    "  local-demo --help                       Unix, local-development default OFF; synthetic only\n",
    "  local-inspect --help                    Unix, local-development default OFF; historical only\n",
    "  secrets --help                          Explicit local storage; Keychain default fails closed\n",
    "  --socket PATH up TIER|down|shutdown|reload-config|status|logs|scale TIER N [--json]  Mock-only control\n",
    "  explain-tier --help                    Stored launch-gate observations only\n",
    "  scale-batch --help                     Atomic manual desired updates, Mock-only\n",
    "  reload-config --help                   Explicit reload or check-only preflight\n",
    "  config-status --help                   Active Mock admission snapshot, not desired state\n",
    "  logs --help                            History paging/opt-in follow, Mock-only\n",
    "  top --socket PATH [--once] [--json]      Read-only daemon dashboard\n",
    "No model download, provider approval or runtime/security qualification is implied.\n",
);
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.into_string().map_err(|_| "invalid arguments"))
        .collect::<Result<_, _>>()?;
    if args == ["--help"] {
        use std::io::Write;
        std::io::stdout()
            .lock()
            .write_all(HELP.as_bytes())
            .map_err(|_| "help-output-unavailable")?;
        return Ok(());
    }
    if args.first().is_some_and(|a| a == "explain-tier")
        || (args.first().is_some_and(|a| a == "--socket")
            && args.get(2).is_some_and(|a| a == "explain-tier"))
    {
        return explain::run(&args).map_err(Into::into);
    }
    if args.first().is_some_and(|a| a == "scale-batch")
        || (args.first().is_some_and(|a| a == "--socket")
            && args.get(2).is_some_and(|a| a == "scale-batch"))
    {
        return batch::run(&args).map_err(Into::into);
    }
    if args.first().is_some_and(|a| a == "init") {
        return init::run(&args[1..]).map_err(Into::into);
    }
    if args.first().is_some_and(|a| a == "reload-config")
        || (args.first().is_some_and(|a| a == "--socket")
            && args.get(2).is_some_and(|a| a == "reload-config")
            && args[3..].iter().any(|a| a == "--check"))
    {
        return reload_check::run(&args).map_err(Into::into);
    }
    if args.first().is_some_and(|arg| arg == "config-status")
        || (args.first().is_some_and(|a| a == "--socket")
            && args.get(2).is_some_and(|a| a == "config-status"))
    {
        return config_status::run(&args).map_err(Into::into);
    }
    if (args.first().is_some_and(|a| a == "logs") && args[1..].iter().any(|a| a == "--follow"))
        || (args.first().is_some_and(|a| a == "--socket")
            && args.get(2).is_some_and(|a| a == "logs")
            && args[3..].iter().any(|a| a == "--follow"))
    {
        return log_follow::run(&args).map_err(Into::into);
    }
    if args == ["logs", "--help"]
        || (args.first().is_some_and(|a| a == "--socket")
            && args.get(2).is_some_and(|a| a == "logs")
            && (args.len() > 4 || args[3..].iter().any(|a| a != "--json")))
    {
        return log_page::run(&args).map_err(Into::into);
    }
    if args.first().is_some_and(|arg| arg == "schedule") {
        return schedule::run(&args[1..]).map_err(Into::into);
    }
    if args.first().is_some_and(|arg| arg == "queue") {
        return queue::run(&args[1..]).map_err(Into::into);
    }
    if args.first().is_some_and(|arg| arg == "config") {
        return config::run(&args[1..]).map_err(Into::into);
    }
    if args.first().is_some_and(|arg| arg == "models") {
        return models::run(&args[1..]).map_err(Into::into);
    }
    // Preserve whole-argv UTF-8 validation, but own all local-demo flags before
    // legacy socket/JSON parsing. No daemon, credential or executable discovery.
    if args.first().is_some_and(|arg| arg == "local-inspect") {
        if args[1..] == ["--help"] {
            println!("usage: m8s local-inspect --state-dir CANONICAL_ABSOLUTE_PATH");
            println!(
                "Unix only; build with --features local-development (disabled by default). Historical synthetic state; no child or readiness assessment."
            );
            return Ok(());
        }
        #[cfg(all(unix, feature = "local-development"))]
        {
            let inspect_args = args[1..]
                .iter()
                .take(3)
                .map(std::ffi::OsString::from)
                .collect();
            if m8sd::local_demo::inspect_cli(inspect_args, &mut std::io::stdout().lock()) != 0 {
                return Err("local-inspect snapshot unavailable".into());
            }
            return Ok(());
        }
        #[cfg(not(feature = "local-development"))]
        return Err("local-inspect unavailable: build with --features local-development".into());
        #[cfg(all(feature = "local-development", not(unix)))]
        return Err("local-inspect unavailable on this platform".into());
    }
    if args.first().is_some_and(|arg| arg == "local-demo") {
        if args[1..] == ["--help"] {
            println!(
                "usage: m8s local-demo --synthetic-accounting --state-dir NEW_CANONICAL_ABSOLUTE_PATH --synthetic-cap-micro-usd 1..1000000 --scenario cancel|deadline"
            );
            println!(
                "Unix only; build with --features local-development (disabled by default). Synthetic accounting is not approval; no model or readiness."
            );
            return Ok(());
        }
        #[cfg(all(unix, feature = "local-development"))]
        {
            let demo_args = args[1..]
                .iter()
                .take(17)
                .map(std::ffi::OsString::from)
                .collect();
            let code = m8sd::local_demo::run_cli(demo_args, &mut std::io::stdout().lock());
            if code != 0 {
                return Err("local-demo failed; no retry or reset".into());
            }
            return Ok(());
        }
        #[cfg(not(feature = "local-development"))]
        return Err("local-demo unavailable: build with --features local-development".into());
        #[cfg(all(feature = "local-development", not(unix)))]
        return Err("local-demo unavailable on this platform".into());
    }
    if args.first().is_some_and(|arg| arg == "plan") {
        return plan::run(&args[1..]).map_err(Into::into);
    }
    if args.first().is_some_and(|arg| arg == "secrets") {
        return secrets::run(&args[1..]).map_err(Into::into);
    }
    // Keep the existing whole-argv UTF-8 validation above, including paths.
    // Dispatch before legacy --json extraction: the shared dashboard owns its
    // flags/projection. No PATH search, subprocess, shell or global argv changes.
    let top_args = if args.first().is_some_and(|arg| arg == "top") {
        Some(
            args[1..]
                .iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>(),
        )
    } else if args.first().is_some_and(|arg| arg == "--socket")
        && args.get(2).is_some_and(|arg| arg == "top")
    {
        Some(
            args[..2]
                .iter()
                .chain(&args[3..])
                .map(std::ffi::OsString::from)
                .collect(),
        )
    } else {
        None
    };
    if let Some(top_args) = top_args {
        let options = m8s_top::app::Options::parse(&top_args)?;
        return m8s_top::app::run(options).map_err(Into::into);
    }
    let json = if let Some(i) = args.iter().position(|a| a == "--json") {
        args.remove(i);
        true
    } else {
        false
    };
    if args.len() < 3 || args[0] != "--socket" {
        return Err(
            "usage: m8s --socket PATH <up TIER|down|shutdown|reload-config|status|logs|scale TIER N|top [--once] [--timeout-ms 1..10000]> [--json]; m8s secrets --help; m8s plan --help".into(),
        );
    }
    let socket = std::path::PathBuf::from(&args[1]);
    let request = match args[2].as_str() {
        "up" if args.len() == 4 => Request::Up {
            tier: args[3].clone(),
        },
        "scale" if args.len() == 5 => Request::Scale {
            tier: args[3].clone(),
            replicas: args[4].parse()?,
        },
        "down" if args.len() == 3 => Request::Down {},
        "shutdown" if args.len() == 3 => Request::Shutdown {},
        "reload-config" if args.len() == 3 => Request::ReloadConfig {},
        "status" if args.len() == 3 => Request::Status {},
        "logs" if args.len() == 3 => Request::Logs {},
        _ => return Err("invalid command; only mock control is available".into()),
    };
    let response = m8s_api::call(&socket, &request)?;
    if json {
        println!("{}", serde_json::to_string(&response)?);
    } else {
        println!(
            "mode: {} · secure bootstrap: {}",
            response.mode, response.secure_bootstrap
        );
        for (tier, n) in &response.tiers {
            println!("{tier}: desired {n}");
        }
        for n in &response.nodes {
            println!(
                "{} {} {:?} {}",
                n.tier,
                n.uid,
                n.phase,
                n.instance_id.as_deref().unwrap_or("unresolved")
            );
        }
        if matches!(request, Request::Logs {}) {
            for e in &response.events {
                println!("{e}");
            }
        }
        if let Some(error) = &response.error {
            eprintln!("{error}");
        }
    }
    if !response.ok {
        std::process::exit(1);
    }
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("m8s: {e}");
        std::process::exit(1)
    }
}
