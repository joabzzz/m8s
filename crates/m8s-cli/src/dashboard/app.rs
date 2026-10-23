//! Explicit opt-in executable adapter. Parsing/imports perform no I/O.
//! Only `run` may connect the status socket or initialize terminal/signal scopes.
use crate::dashboard::terminal;
use crate::dashboard::{
    client::StatusClient,
    view::{Dashboard, bounded_area, text_snapshot},
};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{Terminal, TerminalOptions, Viewport, backend::CrosstermBackend};
use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

struct Signals {
    requested: Arc<AtomicBool>,
    ids: Vec<signal_hook::SigId>,
}
impl Signals {
    fn install() -> Result<Self, &'static str> {
        let mut signals = Self {
            requested: Arc::new(AtomicBool::new(false)),
            ids: vec![],
        };
        for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
            let id = signal_hook::flag::register(signal, signals.requested.clone())
                .map_err(|_| "termination setup failed")?;
            signals.ids.push(id);
        }
        Ok(signals)
    }
}
impl Drop for Signals {
    fn drop(&mut self) {
        for id in self.ids.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}
/// Validated arguments; construction/parsing has no terminal, signal or socket effects.
pub struct Options {
    socket: PathBuf,
    json: bool,
    once: bool,
    show_config: bool,
    timeout: Duration,
}
impl Options {
    /// Parses an explicit argument slice, excluding executable/subcommand names.
    /// Paths retain OS bytes; diagnostics never include argument values.
    pub fn parse(args: &[std::ffi::OsString]) -> Result<Self, &'static str> {
        let mut args = args.iter();
        let mut socket = None;
        let mut json = false;
        let mut once = false;
        let mut show_config = false;
        let mut timeout = Duration::from_secs(1);
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--socket") if socket.is_none() => {
                    socket = Some(PathBuf::from(args.next().ok_or("missing socket path")?))
                }
                Some("--json") if !json => json = true,
                Some("--once") if !once => once = true,
                Some("--show-config") if !show_config => show_config = true,
                Some("--timeout-ms") => {
                    let millis = args
                        .next()
                        .and_then(|v| v.to_str()?.parse::<u64>().ok())
                        .filter(|v| (1..=10000).contains(v))
                        .ok_or("invalid status timeout")?;
                    timeout = Duration::from_millis(millis);
                }
                _ => {
                    return Err(
                        "usage: m8s-top --socket PATH [--once | --json] [--timeout-ms 1..10000]",
                    );
                }
            }
        }
        Ok(Options {
            socket: socket.ok_or("missing --socket PATH")?,
            json,
            once,
            show_config,
            timeout,
        })
    }
}
/// Runs the read-only application in the calling process. This is deliberately
/// effectful: it uses stdin/stdout, connects the validated private status socket,
/// and, only when interactive, temporarily changes terminal modes and registers
/// SIGINT/SIGTERM callbacks. Call serially from an explicit executable command;
/// do not embed concurrently with another terminal/signal owner. Cleanup remains
/// best-effort RAII, not restoration of arbitrary prior signal dispositions.
pub fn run(options: Options) -> Result<(), String> {
    let client = StatusClient::new(options.timeout).map_err(|e| e.to_string())?;
    let read = || {
        if options.show_config {
            client.status_with_config(&options.socket)
        } else {
            client.status(&options.socket)
        }
    };
    if options.json || options.once || !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        let snapshot = read().map_err(|e| e.to_string())?;
        if options.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&snapshot).map_err(|_| "status encoding failed")?
            );
        } else {
            let dashboard = Dashboard {
                snapshot: Some(snapshot),
                ..Default::default()
            };
            print!(
                "{}",
                text_snapshot(&dashboard, 150, 24).map_err(|_| "status render failed")?
            );
        }
        return Ok(());
    }
    let signals = Signals::install().map_err(str::to_owned)?;
    let _guard =
        terminal::TerminalGuard::enter(terminal::RealModes).map_err(|_| "terminal setup failed")?;
    let (width, height) = crossterm::terminal::size().map_err(|_| "terminal size unavailable")?;
    let mut terminal = Terminal::with_options(
        CrosstermBackend::new(io::stdout()),
        TerminalOptions {
            viewport: Viewport::Fixed(bounded_area(width, height)),
        },
    )
    .map_err(|_| "terminal setup failed")?;
    let mut dashboard = Dashboard::default();
    let mut refresh = Instant::now();
    loop {
        if signals.requested.load(Ordering::Relaxed) {
            break;
        }
        if Instant::now() >= refresh {
            dashboard.update(read());
            refresh = Instant::now() + Duration::from_secs(1);
        }
        if signals.requested.load(Ordering::Relaxed) {
            break;
        }
        terminal
            .draw(|frame| dashboard.render(frame))
            .map_err(|_| "terminal render failed")?;
        if event::poll(Duration::from_millis(50)).map_err(|_| "terminal event failed")? {
            match event::read().map_err(|_| "terminal event failed")? {
                Event::Resize(width, height) => {
                    terminal
                        .resize(bounded_area(width, height))
                        .map_err(|_| "terminal resize failed")?;
                }
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    KeyCode::Char('r') => refresh = Instant::now(),
                    KeyCode::Down | KeyCode::Char('j') => {
                        dashboard.offset = dashboard.offset.saturating_add(1)
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        dashboard.offset = dashboard.offset.saturating_sub(1)
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<std::ffi::OsString> {
        values.iter().map(Into::into).collect()
    }
    #[test]
    fn parsing_is_effect_free_and_preserves_standalone_flag_behavior() {
        // Nonexistent path: parsing does not stat/connect, touch TTYs or install
        // handlers. Options fields are private; only this parser constructs them.
        let options = Options::parse(&args(&[
            "--socket",
            "/synthetic/nonexistent",
            "--json",
            "--once",
            "--timeout-ms",
            "1",
            "--timeout-ms",
            "10000",
        ]))
        .unwrap();
        assert!(options.json && options.once);
        assert_eq!(options.timeout, Duration::from_secs(10));
        assert_eq!(options.socket, PathBuf::from("/synthetic/nonexistent"));
        for values in [
            vec![],
            vec!["--socket"],
            vec!["--socket", "x", "--socket", "y"],
            vec!["--socket", "x", "--once", "--once"],
            vec!["--socket", "x", "--json", "--json"],
            vec!["--socket", "x", "--timeout-ms", "0"],
            vec!["--socket", "x", "--timeout-ms", "10001"],
            vec!["--socket", "x", "--timeout-ms", "opaque-invalid-value"],
        ] {
            let error = Options::parse(&args(&values)).err().unwrap();
            assert!(!error.contains("opaque-invalid-value"));
        }
    }
    #[cfg(unix)]
    #[test]
    fn standalone_path_bytes_stay_opaque_and_invalid_flag_is_not_echoed() {
        use std::os::unix::ffi::OsStringExt;
        let bytes = std::ffi::OsString::from_vec(b"synthetic-\xff".to_vec());
        let options = Options::parse(&["--socket".into(), bytes.clone()]).unwrap();
        assert_eq!(options.socket.as_os_str(), bytes);
        assert!(Options::parse(&["--socket".into(), "x".into(), bytes]).is_err());
    }
}

#[cfg(test)]
mod config_option_tests {
    use super::*;
    #[test]
    fn config_flag_is_explicit_effectfree_and_not_repeatable() {
        let args: Vec<_> = ["--socket", "/synthetic/missing", "--show-config", "--json"]
            .into_iter()
            .map(Into::into)
            .collect();
        assert!(Options::parse(&args).unwrap().show_config);
        let mut duplicate = args;
        duplicate.push("--show-config".into());
        assert!(Options::parse(&duplicate).is_err());
    }
}
