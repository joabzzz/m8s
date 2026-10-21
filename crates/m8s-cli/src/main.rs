mod secrets;
use m8s_api::Request;
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.into_string().map_err(|_| "invalid arguments"))
        .collect::<Result<_, _>>()?;
    if args.first().is_some_and(|arg| arg == "secrets") {
        return secrets::run(&args[1..]).map_err(Into::into);
    }
    let json = if let Some(i) = args.iter().position(|a| a == "--json") {
        args.remove(i);
        true
    } else {
        false
    };
    if args.len() < 3 || args[0] != "--socket" {
        return Err(
            "usage: m8s --socket PATH <up TIER|down|status|logs|scale TIER N> [--json]; m8s secrets --help".into(),
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
