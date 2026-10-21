//! Synthetic PTY only; helpers shared with the in-process CLI alias checks.
#![cfg(unix)]
#[path = "support/terminal_cleanup.rs"]
mod support;
use nix::sys::signal::Signal;
use support::cleanup_case;
#[test]
fn quit_key_restores_synthetic_terminal() {
    cleanup_case(env!("CARGO_BIN_EXE_m8s-top"), &[], "quit", None, Some(b"q"));
}
#[test]
fn ctrl_c_key_restores_synthetic_terminal() {
    cleanup_case(
        env!("CARGO_BIN_EXE_m8s-top"),
        &[],
        "ctrl-c",
        None,
        Some(b"\x03"),
    );
}
#[test]
fn sigterm_restores_synthetic_terminal() {
    cleanup_case(
        env!("CARGO_BIN_EXE_m8s-top"),
        &[],
        "term",
        Some(Signal::SIGTERM),
        None,
    );
}
#[test]
fn sigint_restores_synthetic_terminal() {
    cleanup_case(
        env!("CARGO_BIN_EXE_m8s-top"),
        &[],
        "int",
        Some(Signal::SIGINT),
        None,
    );
}
