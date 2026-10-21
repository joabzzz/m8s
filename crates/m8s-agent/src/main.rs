//! Local lifecycle exerciser, NOT a production node entrypoint.
use m8s_agent::supervisor::{
    ChildSpec, LocalOnlyBoot, RetryTeardown, Supervisor, TeardownDriver, TeardownState,
};
use m8s_agent::{DeadManSwitch, Effect, Outcome};
use std::time::{Duration, Instant};

// Binary-only: importing or using the library never changes signal dispositions.
#[cfg(unix)]
mod termination {
    use signal_hook::consts::{SIGINT, SIGTERM};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    pub(super) struct ShutdownSignals {
        requested: Arc<AtomicBool>,
        handlers: Vec<signal_hook::SigId>,
    }
    impl ShutdownSignals {
        pub(super) fn install() -> Result<Self, &'static str> {
            let mut signals = Self {
                requested: Arc::new(AtomicBool::new(false)),
                handlers: Vec::new(),
            };
            for signal in [SIGTERM, SIGINT] {
                // signal-hook's flag handler only stores to an atomic. No locks,
                // allocation, logging, subprocess operations or teardown in it.
                let id = signal_hook::flag::register(signal, signals.requested.clone())
                    .map_err(|_| "local termination handler installation failed")?;
                signals.handlers.push(id);
            }
            Ok(signals)
        }
        pub(super) fn requested(&self) -> bool {
            self.requested.load(Ordering::Relaxed)
        }
    }
    impl Drop for ShutdownSignals {
        fn drop(&mut self) {
            for handler in self.handlers.drain(..) {
                signal_hook::low_level::unregister(handler);
            }
        }
    }
}

#[cfg(not(unix))]
mod termination {
    pub(super) struct ShutdownSignals;
    impl ShutdownSignals {
        pub(super) fn install() -> Result<Self, &'static str> {
            Err("local termination handling unavailable on this platform")
        }
        pub(super) fn requested(&self) -> bool {
            false
        }
    }
}

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
    // Install before spawn so a signal during launch is remembered, not lost.
    // This local mode performs a single teardown batch, never indefinite retries.
    let signals = termination::ShutdownSignals::install()?;
    if signals.requested() {
        return Err("local termination requested before launch; provider absence unconfirmed");
    }
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
        if signals.requested() {
            // Idempotent on repeated signals. All effectful cleanup runs here,
            // on the ordinary main thread, never inside the signal handler.
            supervisor.shutdown();
        }
        let now = i64::try_from(start.elapsed().as_secs()).unwrap_or(i64::MAX);
        if supervisor
            .tick(now)
            .map_err(|_| "local child termination failed")?
            != TeardownState::Running
        {
            // Tick requests teardown without waiting for direct-child exit.
            // Pending local cleanup is not provider-confirmed destruction.
            return Err(
                "local teardown requested; provider absence unconfirmed; owner cleanup required",
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
