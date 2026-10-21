//! Read-only status projection. No resource commands, credentials, transport
//! admission or implicit terminal/signal initialization. Explicit `app::run` opts
//! into executable I/O; importing, parsing and rendering remain effect-free.
pub mod app;
pub mod client;
mod terminal;
pub mod view;

use m8s_core::{Phase, Timestamp};
use serde::Serialize;

#[derive(Clone, Serialize)]
pub struct NodeStatus {
    pub uid: String,
    pub pool: String,
    /// The daemon's reported phase, not independently verified readiness.
    pub reported_phase: Phase,
    pub instance_id: Option<String>,
    pub last_observation: Option<Timestamp>,
    pub model_ready: Option<bool>,
    pub destroy_pending: Option<bool>,
}
#[derive(Clone, Serialize)]
pub struct StatusSnapshot {
    pub schema_version: u32,
    pub reported_mode: String,
    pub pools: Vec<(String, u32)>,
    pub nodes: Vec<NodeStatus>,
    pub bootstrap_verified: Option<bool>,
    pub heartbeat_age_seconds: Option<u64>,
    pub hourly_micro_usd: Option<u64>,
}

/// Display only printable ASCII. Control/bidi/non-ASCII characters become
/// visible escapes, never terminal commands. This is not secret-value redaction:
/// the protocol must remain secret-free; raw events/errors are never displayed.
pub fn display_text(input: &str) -> String {
    use std::fmt::Write;
    let mut output = String::new();
    for ch in input.chars() {
        if ch.is_ascii_graphic() || ch == ' ' {
            output.push(ch);
        } else {
            // Writing to a String cannot fail; no source values enter errors.
            let _ = write!(output, "{}", ch.escape_unicode());
        }
    }
    output
}
