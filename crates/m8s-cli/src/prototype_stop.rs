//! Explicit selected-run cleanup only. No launch, adoption or provider absence.
use std::{io::Write, path::PathBuf};
const HELP: &str = "m8s prototype stop --run-directory ABS --run-id HEX32 --container-id HEX64 --docker ABS --docker-socket ABS --command-timeout-ms 1001..60000 --stop-grace-seconds 1..30 [--observe-only] [--json]\nRequires prototype-container feature on Unix. Reads private intent/created records; exact ID/run/node/image inspection before stop/removal. No name adoption, scan/prune, restart, retries or provider/billing absence claim. Failure retains uncertain resources and all material.\n";
struct Options {
    root: PathBuf,
    run: String,
    id: String,
    docker: PathBuf,
    socket: PathBuf,
    timeout: u64,
    grace: u16,
    json: bool,
    observe: bool,
}
fn parse(args: &[String]) -> Result<Options, &'static str> {
    let invalid = "invalid-prototype-stop-arguments";
    let mut values = std::collections::BTreeMap::new();
    let mut json = false;
    let mut observe = false;
    let mut rest = args;
    while let Some((key, tail)) = rest.split_first() {
        rest = tail;
        if key == "--observe-only" && !observe {
            observe = true;
            continue;
        }
        if key == "--json" && !json {
            json = true;
            continue;
        }
        if !matches!(
            key.as_str(),
            "--run-directory"
                | "--run-id"
                | "--container-id"
                | "--docker"
                | "--docker-socket"
                | "--command-timeout-ms"
                | "--stop-grace-seconds"
        ) || values.contains_key(key)
        {
            return Err(invalid);
        }
        let (value, tail) = rest.split_first().ok_or(invalid)?;
        values.insert(key.clone(), value.clone());
        rest = tail;
    }
    let mut take = |key: &str| values.remove(key).ok_or(invalid);
    let root = PathBuf::from(take("--run-directory")?);
    let run = take("--run-id")?;
    let id = take("--container-id")?;
    let docker = PathBuf::from(take("--docker")?);
    let socket = PathBuf::from(take("--docker-socket")?);
    fn number(s: String) -> Option<u64> {
        if s.is_empty() || s.len() > 5 || !s.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        s.parse().ok()
    }
    let timeout = number(take("--command-timeout-ms")?)
        .filter(|n| (1001..=60000).contains(n))
        .ok_or(invalid)?;
    let grace = number(take("--stop-grace-seconds")?)
        .filter(|n| (1..=30).contains(n))
        .ok_or(invalid)? as u16;
    let hex = |s: &str, n| {
        s.len() == n
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    if !hex(&run, 32)
        || !hex(&id, 64)
        || [&root, &docker, &socket].iter().any(|p| !p.is_absolute())
        || timeout <= u64::from(grace) * 1000
    {
        return Err(invalid);
    }
    Ok(Options {
        root,
        run,
        id,
        docker,
        socket,
        timeout,
        grace,
        json,
        observe,
    })
}
pub fn run(args: &[String]) -> Result<(), &'static str> {
    if args == ["--help"] {
        return std::io::stdout()
            .lock()
            .write_all(HELP.as_bytes())
            .map_err(|_| "prototype-stop-output-unavailable");
    }
    let options = parse(args)?;
    perform(options)
}
#[cfg(all(unix, feature = "prototype-container"))]
fn perform(options: Options) -> Result<(), &'static str> {
    use m8s_agent::node::container::{OwnedContainerStop, StopSpec};
    let mut stop = OwnedContainerStop::open(
        &options.root,
        StopSpec {
            docker: options.docker,
            socket: options.socket,
            run_id: options.run,
            container_id: options.id,
            command_timeout: std::time::Duration::from_millis(options.timeout),
            stop_grace_seconds: options.grace,
        },
    )
    .map_err(|_| "prototype-stop-refused-or-uncertain")?;
    if options.observe {
        if !stop
            .observe_removed()
            .map_err(|_| "prototype-stop-refused-or-uncertain")?
        {
            return Err("prototype-container-still-present");
        }
    } else {
        stop.stop_and_remove()
            .map_err(|_| "prototype-stop-refused-or-uncertain")?;
    }
    let output = if options.json {
        "{\"schema\":\"m8s.prototype-stop/v1\",\"local_container\":\"removed-observed\",\"provider_absence\":\"not-assessed\",\"billing\":\"not-assessed\"}\n"
    } else {
        "local container removed (observed); provider absence and billing not assessed\n"
    };
    std::io::stdout()
        .lock()
        .write_all(output.as_bytes())
        .map_err(|_| "prototype-stop-output-unavailable")
}
#[cfg(not(all(unix, feature = "prototype-container")))]
fn perform(options: Options) -> Result<(), &'static str> {
    // Consume parsed fields without touching filesystem/processes in disabled builds.
    let Options {
        root,
        run,
        id,
        docker,
        socket,
        timeout,
        grace,
        json,
        observe,
    } = options;
    let _ = (root, run, id, docker, socket, timeout, grace, json, observe);
    Err("prototype-container-unavailable")
}
#[cfg(test)]
mod tests {
    use super::*;
    fn args() -> Vec<String> {
        [
            "--run-directory",
            "/private/run",
            "--run-id",
            &"a".repeat(32),
            "--container-id",
            &"b".repeat(64),
            "--docker",
            "/private/docker",
            "--docker-socket",
            "/private/docker.sock",
            "--command-timeout-ms",
            "3000",
            "--stop-grace-seconds",
            "1",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    }
    #[test]
    fn explicit_bounded_inputs_only() {
        assert!(parse(&args()).is_ok());
        let mut invalid = args();
        invalid.extend(["--run-id".into(), "a".repeat(32)]);
        assert!(parse(&invalid).is_err());
        for (slot, value) in [
            (1, "relative"),
            (3, "latest"),
            (5, "name-not-id"),
            (11, "1000"),
            (13, "31"),
        ] {
            let mut invalid = args();
            invalid[slot] = value.into();
            assert!(parse(&invalid).is_err());
        }
    }
    #[test]
    fn help_is_standalone_and_json_not_repeatable() {
        assert!(parse(&["--help".into(), "--json".into()]).is_err());
        let mut invalid = args();
        invalid.extend(["--json".into(), "--json".into()]);
        assert!(parse(&invalid).is_err());
    }
    #[cfg(not(all(unix, feature = "prototype-container")))]
    #[test]
    fn disabled_stop_fails_before_any_io() {
        assert_eq!(run(&args()), Err("prototype-container-unavailable"));
    }
}
