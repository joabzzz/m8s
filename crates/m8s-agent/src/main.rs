//! Local lifecycle exerciser, NOT a production node entrypoint.
use m8s_agent::supervisor::{
    ChildSpec, LocalOnlyBoot, RetryTeardown, Supervisor, TeardownDriver, TeardownState,
};
use m8s_agent::{DeadManSwitch, Effect, Outcome};
use std::time::{Duration, Instant};

struct UnavailableDestroy;
impl TeardownDriver for UnavailableDestroy {
    fn perform(&mut self, _: Effect) -> Outcome {
        // No model cache, credentials or provider adapter in local mode.
        Outcome::Failure
    }
}
fn run() -> Result<(), &'static str> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--local")) {
        return Err(
            "secure boot unavailable; local usage: m8s-agent --local TTL_SECONDS -- PROGRAM [NONSECRET_ARGS...]",
        );
    }
    let ttl: i64 = args
        .next()
        .and_then(|v| v.to_str()?.parse().ok())
        .filter(|v| *v > 0)
        .ok_or("invalid local TTL")?;
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--")) {
        return Err("missing command separator");
    }
    let program = args.next().ok_or("missing local command")?;
    let dms = DeadManSwitch::new("local-only".into(), vec![], 0, ttl, ttl)?;
    let mut supervisor = Supervisor::start(
        ChildSpec {
            program: program.into(),
            args: args.collect(),
        },
        &mut LocalOnlyBoot,
        dms,
        UnavailableDestroy,
        RetryTeardown::new(4)?,
    )
    .map_err(|_| "local child launch failed")?;
    eprintln!("local-only supervision; no secure transport, readiness or provider destruction");
    let start = Instant::now();
    loop {
        let now = i64::try_from(start.elapsed().as_secs()).unwrap_or(i64::MAX);
        if supervisor
            .tick(now)
            .map_err(|_| "local child termination failed")?
            != TeardownState::Running
        {
            // Tick already killed/reaped the direct child. Never call this destroyed.
            return Err(
                "local child terminated; provider absence unconfirmed; owner cleanup required",
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
fn main() {
    if let Err(message) = run() {
        eprintln!("{message}");
        std::process::exit(2);
    }
}
