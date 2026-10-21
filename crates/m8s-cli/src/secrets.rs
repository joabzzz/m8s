//! Explicit local secrets commands. Never log rejected arguments or values.
use m8s_secrets::{
    Error, SecretStore, SecretValue,
    ux::{self, Input, InputMode, Output},
};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

struct Options {
    file: Option<PathBuf>,
    json: bool,
    command: ux::Command,
}
fn parse(args: &[String]) -> Result<Options, Error> {
    let mut file = None;
    let mut json = false;
    let mut command = Vec::new();
    let mut rest = args;
    while let Some((arg, tail)) = rest.split_first() {
        match arg.as_str() {
            "--file" if file.is_none() => {
                let (path, tail) = tail.split_first().ok_or(Error::InvalidData)?;
                if path.starts_with('-') {
                    return Err(Error::InvalidData);
                }
                file = Some(PathBuf::from(path));
                rest = tail;
            }
            "--json" if !json => {
                json = true;
                rest = tail;
            }
            "--file" | "--json" => return Err(Error::InvalidData),
            _ => {
                command.push(arg.as_str());
                rest = tail;
            }
        }
    }
    Ok(Options {
        file,
        json,
        command: ux::parse(&command)?,
    })
}
struct PromptInput {
    terminal_reads: usize,
}
impl Input for PromptInput {
    fn read(&mut self, mode: InputMode) -> Result<SecretValue, Error> {
        let mut prompted = false;
        let result = ux::SystemInput.read_with_prompt(mode, || {
            // The library invokes this only inside guarded no-echo mode.
            let mut stderr = std::io::stderr().lock();
            stderr
                .write_all(if self.terminal_reads == 0 {
                    b"Secret (no echo): "
                } else {
                    b"Confirm secret (no echo): "
                })
                .map_err(|_| Error::Io)?;
            stderr.flush().map_err(|_| Error::Io)?;
            self.terminal_reads += 1;
            prompted = true;
            Ok(())
        });
        if prompted {
            writeln!(std::io::stderr().lock()).map_err(|_| Error::Io)?;
        }
        result
    }
}
fn metadata(output: Output, backend: &str) -> Value {
    match output {
        Output::Stored => json!({"ok":true,"operation":"set","backend":backend}),
        Output::Removed(removed) => {
            json!({"ok":true,"operation":"rm","backend":backend,"removed":removed})
        }
        Output::Listed(entries) => {
            json!({"ok":true,"operation":"list","backend":backend,"entries":entries.into_iter().map(|entry| json!({"name":entry.name.as_str(),"expires_at":entry.expires_at})).collect::<Vec<_>>()})
        }
        Output::Checked {
            backend,
            active_entries,
        } => {
            json!({"ok":true,"operation":"check","backend":backend,"scope":"local-storage-only","active_entries":active_entries,"owner_keystore":"not-checked","provider_permissions":"not-checked","binary_signing":"not-checked","os_keychain_qualification":"not-performed"})
        }
    }
}
fn execute(
    store: &mut dyn SecretStore,
    command: ux::Command,
    input: &mut dyn Input,
    now: i64,
) -> Result<Value, Error> {
    let backend = store.backend();
    ux::execute(store, command, input, now).map(|output| metadata(output, backend))
}
fn run_inner(args: &[String]) -> Result<(bool, Value), Error> {
    let options = parse(args)?;
    // No Keychain construction/OS access and no fallback. The default is not
    // safely qualified for independent CLI processes, so it remains unavailable.
    let path = options.file.ok_or(Error::Unavailable)?;
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(Error::Unavailable)
    }
    #[cfg(unix)]
    {
        let mut store = m8s_secrets::FileStore::open(&path)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::Io)?
            .as_secs()
            .try_into()
            .map_err(|_| Error::Io)?;
        let output = execute(
            &mut store,
            options.command,
            &mut PromptInput { terminal_reads: 0 },
            now,
        )?;
        Ok((options.json, output))
    }
}
const HELP: &str = "usage: m8s secrets --file ABSOLUTE_PATH <set NAME [--stdin] [--expires-at UNIX_SECONDS] | list | rm NAME | check> [--json]
File backend selection is explicit. Default Keychain is unavailable in this CLI build; no fallback is attempted.
Set uses a local no-echo terminal with confirmation, or explicit --stdin (exact bytes through EOF).
No argv values, get or show. Check covers local storage only, not owner/provider/signing qualification.";
fn text(out: &mut dyn Write, value: &Value) -> std::io::Result<()> {
    let backend = value["backend"].as_str().unwrap_or("unavailable");
    match value["operation"].as_str() {
        Some("set") => writeln!(out, "stored · backend {backend}"),
        Some("rm") => writeln!(out, "removed: {} · backend {backend}", value["removed"]),
        Some("list") => {
            writeln!(out, "backend: {backend} · active entries only")?;
            if let Some(entries) = value["entries"].as_array() {
                for entry in entries {
                    writeln!(
                        out,
                        "{} · expires_at: {}",
                        entry["name"].as_str().unwrap_or(""),
                        entry["expires_at"]
                    )?;
                }
            }
            Ok(())
        }
        Some("check") => writeln!(
            out,
            "local storage readable · backend {backend} · active entries {}\nowner keystore, provider permissions, binary signing and OS Keychain qualification: NOT CHECKED",
            value["active_entries"]
        ),
        _ => Err(std::io::Error::other("secret output failed")),
    }
}
pub(crate) fn run(args: &[String]) -> Result<(), &'static str> {
    if args == ["--help"] || args == ["help"] {
        writeln!(std::io::stdout().lock(), "{HELP}").map_err(|_| "secret output failed")?;
        return Ok(());
    }
    // Inspect only the presence of the fixed flag; never render raw argv.
    let json_requested = args.iter().any(|arg| arg == "--json");
    match run_inner(args) {
        Ok((json, value)) => {
            let mut out = std::io::stdout().lock();
            if json {
                serde_json::to_writer(&mut out, &value).map_err(|_| "secret output failed")?;
                writeln!(out).map_err(|_| "secret output failed")?;
            } else {
                text(&mut out, &value).map_err(|_| "secret output failed")?;
            }
            Ok(())
        }
        Err(_) => {
            if json_requested {
                // Generic, schema-stable failure, with no path/arguments/values.
                writeln!(
                    std::io::stdout().lock(),
                    "{{\"ok\":false,\"error\":\"secret-command-failed\"}}"
                )
                .map_err(|_| "secret output failed")?;
            }
            Err(
                "secret command failed; use explicit --file PATH and set/list/rm/check (set uses local terminal or explicit --stdin); no fallback attempted",
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use m8s_secrets::{KeyringDriver, KeyringStore};
    use std::sync::Mutex;
    #[derive(Default)]
    struct Mock(Mutex<Option<SecretValue>>);
    impl KeyringDriver for Mock {
        fn load(&self) -> Result<Option<SecretValue>, Error> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .as_ref()
                .map(|value| SecretValue::new(value.expose().to_vec())))
        }
        fn save(&self, value: &SecretValue) -> Result<(), Error> {
            *self.0.lock().unwrap() = Some(SecretValue::new(value.expose().to_vec()));
            Ok(())
        }
    }
    struct Synthetic;
    impl Input for Synthetic {
        fn read(&mut self, _: InputMode) -> Result<SecretValue, Error> {
            Ok(SecretValue::new(b"synthetic-sensitive-value".to_vec()))
        }
    }
    #[test]
    fn injected_backend_json_never_exposes_value() {
        let mut store = KeyringStore::injected(Mock::default());
        let mut input = Synthetic;
        for args in [
            &["set", "test/session"][..],
            &["list"][..],
            &["check"][..],
            &["rm", "test/session"][..],
        ] {
            let value = execute(&mut store, ux::parse(args).unwrap(), &mut input, 0).unwrap();
            assert!(!value.to_string().contains("synthetic-sensitive-value"));
        }
    }
    #[test]
    fn duplicate_options_and_unrecognized_arguments_rejected_opaquely() {
        for args in [
            vec!["--file", "/synthetic/a", "--file", "/synthetic/b", "check"],
            vec!["--json", "--json", "list"],
            vec!["set", "test/session", "synthetic-sensitive-value"],
            vec!["--file"],
            vec!["--file", "--json", "check"],
        ] {
            let args: Vec<_> = args.into_iter().map(str::to_owned).collect();
            assert!(parse(&args).is_err());
        }
    }
}
