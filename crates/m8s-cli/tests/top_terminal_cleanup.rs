//! Reuses the exact standalone synthetic PTY qualification fixture; no /dev/tty.
#![cfg(unix)]
#[path = "support/terminal_cleanup.rs"]
mod support;
use nix::sys::signal::Signal;
#[test]
fn top_alias_quit_restores_terminal() {
    support::cleanup_case(
        env!("CARGO_BIN_EXE_m8s"),
        &["top"],
        "cli-quit",
        None,
        Some(b"q"),
    );
}
#[test]
fn top_alias_sigterm_restores_terminal() {
    support::cleanup_case(
        env!("CARGO_BIN_EXE_m8s"),
        &["top"],
        "cli-term",
        Some(Signal::SIGTERM),
        None,
    );
}
#[test]
fn top_alias_sigint_restores_terminal() {
    support::cleanup_case(
        env!("CARGO_BIN_EXE_m8s"),
        &["top"],
        "cli-int",
        Some(Signal::SIGINT),
        None,
    );
}
