#[cfg(all(unix, feature = "prototype-vast"))]
mod benchmark;
#[cfg(all(unix, feature = "prototype-vast"))]
mod launch;
#[cfg(all(unix, feature = "prototype-vast"))]
mod owner;
// Real HTTP request leg for explicitly local qualification, NOT mesh admission.
use std::{io::Write, path::PathBuf};
const HELP: &str = "m8s prototype request --local-qualification --endpoint http://127.0.0.1:PORT --model ID --prompt-file ABS --max-tokens 1..128 --timeout-ms 1..60000 [--wait-model-ms 1..TIMEOUT-1] [--json]\nOptional wait polls /v1/models for the exact model within the total request deadline. This is model discovery, not Ready.\nOne non-streaming /v1/chat/completions request, no retries/download/launch. Explicit bounded UTF-8 prompt file<=8192 bytes; model ASCII<=128. Loopback HTTP only, no DNS/proxy/redirect/auth header. Does NOT implement or downgrade bootstrap/control TLS. A completion is not verified identity, Ready, owner routing or paid qualification. Timeout is outcome-unknown; computation may continue at server.\n";
struct Options {
    port: u16,
    model: String,
    prompt: PathBuf,
    max_tokens: u32,
    timeout_ms: u64,
    wait_model_ms: u64,
    json: bool,
}
fn decimal(s: &str) -> Option<u64> {
    if s.is_empty() || s.len() > 20 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}
impl Options {
    fn parse(args: &[String]) -> Result<Self, &'static str> {
        let invalid = "invalid-prototype-request-arguments";
        if args.first().is_none_or(|s| s != "request") {
            return Err(invalid);
        }
        let (mut endpoint, mut model, mut prompt, mut max_tokens, mut timeout_ms) =
            (None, None, None, None, None);
        let (mut local, mut json) = (false, false);
        let mut wait_model_ms = None;
        let mut rest = &args[1..];
        while let Some((arg, tail)) = rest.split_first() {
            rest = tail;
            match arg.as_str() {
                "--local-qualification" if !local => local = true,
                "--json" if !json => json = true,
                "--endpoint" | "--model" | "--prompt-file" | "--max-tokens" | "--timeout-ms"
                | "--wait-model-ms" => {
                    let slot = match arg.as_str() {
                        "--endpoint" => &mut endpoint,
                        "--model" => &mut model,
                        "--prompt-file" => &mut prompt,
                        "--max-tokens" => &mut max_tokens,
                        "--wait-model-ms" => &mut wait_model_ms,
                        _ => &mut timeout_ms,
                    };
                    if slot.is_some() {
                        return Err(invalid);
                    }
                    let (value, tail) = rest.split_first().ok_or(invalid)?;
                    *slot = Some(value.as_str());
                    rest = tail;
                }
                _ => return Err(invalid),
            }
        }
        if !local {
            return Err(invalid);
        }
        let endpoint = endpoint.ok_or(invalid)?;
        let port = endpoint
            .strip_prefix("http://127.0.0.1:")
            .and_then(decimal)
            .filter(|n| (1..=65535).contains(n))
            .ok_or(invalid)? as u16;
        let model = model.ok_or(invalid)?;
        if model.is_empty()
            || model.len() > 128
            || !model
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.:/".contains(&b))
        {
            return Err(invalid);
        }
        let raw = prompt.ok_or(invalid)?;
        let prompt = PathBuf::from(raw);
        if !prompt.is_absolute() || raw.len() > 4096 || raw.contains('\0') {
            return Err(invalid);
        }
        let timeout_ms = timeout_ms
            .and_then(decimal)
            .filter(|n| (1..=60000).contains(n))
            .ok_or(invalid)?;
        let wait_model_ms = match wait_model_ms {
            None => 0,
            Some(value) => decimal(value)
                .filter(|n| *n > 0 && *n < timeout_ms)
                .ok_or(invalid)?,
        };
        Ok(Self {
            port,
            model: model.to_owned(),
            prompt,
            max_tokens: max_tokens
                .and_then(decimal)
                .filter(|n| (1..=128).contains(n))
                .ok_or(invalid)? as u32,
            timeout_ms,
            wait_model_ms,
            json,
        })
    }
}
#[derive(serde::Deserialize)]
struct Completion {
    model: String,
    choices: Vec<Choice>,
    error: Option<serde_json::Value>,
}
#[derive(serde::Deserialize)]
struct Choice {
    index: u32,
    message: Message,
    finish_reason: String,
}
#[derive(serde::Deserialize)]
struct Message {
    role: String,
    content: String,
}
fn project(bytes: &[u8], model: &str) -> Result<serde_json::Value, &'static str> {
    let invalid = "prototype-response-unavailable";
    let c: Completion = serde_json::from_slice(bytes).map_err(|_| invalid)?;
    if c.error.is_some() || c.model != model || c.choices.len() != 1 {
        return Err(invalid);
    }
    let choice = &c.choices[0];
    if choice.index != 0
        || choice.message.role != "assistant"
        || choice.message.content.is_empty()
        || choice.message.content.len() > 32768
        || !matches!(choice.finish_reason.as_str(), "stop" | "length")
    {
        return Err(invalid);
    }
    Ok(
        serde_json::json!({"schema":"m8s.prototype-completion/v1","scope":"local-http-qualification-only","model":model,"content":choice.message.content,"finish_reason":choice.finish_reason,"identity":"not-verified","readiness":"not-assessed","owner_routing":"not-verified","provider_qualification":"not-performed","launch_performed":false}),
    )
}
#[cfg(unix)]
mod http;
#[cfg(unix)]
mod model_wait;
#[cfg(unix)]
fn execute(o: &Options) -> Result<Vec<u8>, &'static str> {
    let prompt =
        super::models::load_bytes(&o.prompt).map_err(|_| "prototype-prompt-unavailable")?;
    if prompt.is_empty() || prompt.len() > 8192 {
        return Err("prototype-prompt-unavailable");
    }
    let prompt = std::str::from_utf8(&prompt).map_err(|_| "prototype-prompt-unavailable")?;
    let deadline = std::time::Instant::now()
        .checked_add(std::time::Duration::from_millis(o.timeout_ms))
        .ok_or("prototype-request-unavailable")?;
    if o.wait_model_ms != 0 {
        model_wait::wait(
            o.port,
            &o.model,
            std::time::Instant::now()
                .checked_add(std::time::Duration::from_millis(o.wait_model_ms))
                .ok_or("prototype-model-unavailable")?
                .min(deadline),
        )?;
    }
    let body=serde_json::to_vec(&serde_json::json!({"model":o.model,"messages":[{"role":"user","content":prompt}],"stream":false,"max_tokens":o.max_tokens})).map_err(|_|"prototype-request-unavailable")?;
    let reply =
        http::exchange(o.port, &body, deadline).map_err(|_| "prototype-request-outcome-unknown")?;
    let projection = project(&reply, &o.model)?;
    let encoded = if o.json {
        serde_json::to_string(&projection)
    } else {
        serde_json::to_string_pretty(&projection)
    }
    .map_err(|_| "prototype-output-unavailable")?;
    let mut out =
        super::safe_output::ascii(&encoded).map_err(|_| "prototype-output-unavailable")?;
    out.push(b'\n');
    Ok(out)
}
#[cfg(not(unix))]
fn execute(_: &Options) -> Result<Vec<u8>, &'static str> {
    Err("prototype-request-platform-unavailable")
}
pub(super) fn run(args: &[String]) -> Result<(), &'static str> {
    if args == ["launch", "--help"] {
        return std::io::stdout().write_all(b"m8s prototype launch --plan ABSOLUTE_JSON
Requires prototype-vast. One durable paid attempt, fresh account/price checks, pinned SSH and iroh TLS, owner-routed inference, then provider-confirmed removal. See crates/m8s-cli/README.md for the plan and retained accounting evidence.
").map_err(|_| "prototype-output-unavailable");
    }

    #[cfg(all(unix, feature = "prototype-vast"))]
    if args.first().is_some_and(|a| a == "owner-check") {
        return owner::check(&args[1..]);
    }

    #[cfg(all(unix, feature = "prototype-vast"))]
    if args.first().is_some_and(|a| a == "audit") {
        return launch::audit(&args[1..]);
    }
    #[cfg(all(unix, feature = "prototype-vast"))]
    if args.first().is_some_and(|a| a == "vast-preflight") {
        return launch::preflight(&args[1..]);
    }
    #[cfg(all(unix, feature = "prototype-vast"))]
    if args.first().is_some_and(|a| a == "launch") {
        return launch::run(&args[1..]);
    }
    if args.first().is_some_and(|a| a == "vast-cleanup-worker") {
        if args.len() != 3 || args[1] != "--plan" || !std::path::Path::new(&args[2]).is_absolute() {
            return Err("invalid-vast-cleanup-worker-arguments");
        }
        #[cfg(all(unix, feature = "prototype-vast"))]
        return m8s_provider::vast::watchdog::run(std::path::Path::new(&args[2]))
            .map_err(|_| "vast-cleanup-unresolved-retain-liability");
        #[cfg(not(all(unix, feature = "prototype-vast")))]
        return Err("vast-cleanup-worker-unavailable");
    }
    if args.first().is_some_and(|a| a == "stop") {
        return crate::prototype_stop::run(&args[1..]);
    }

    let mut out = std::io::stdout().lock();
    if args == ["request", "--help"] {
        return out
            .write_all(HELP.as_bytes())
            .map_err(|_| "prototype-output-unavailable");
    }
    let bytes = execute(&Options::parse(args)?)?;
    out.write_all(&bytes)
        .and_then(|()| out.flush())
        .map_err(|_| "prototype-output-unavailable")
}
