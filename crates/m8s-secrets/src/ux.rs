//! Secret command library for CLI integration. No get/show or argv value option.
//! Callers must not log raw argv/input. Output contains metadata only.
use crate::{Error, SecretInfo, SecretName, SecretStore, SecretValue, codec::MAX_VALUE};
use std::io::Read;
use zeroize::Zeroizing;

#[derive(Debug, PartialEq, Eq)]
pub enum InputMode {
    Terminal,
    Stdin,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Set {
        name: SecretName,
        input: InputMode,
        expires_at: Option<i64>,
    },
    List,
    Remove {
        name: SecretName,
    },
    Check,
}
/// Syntax errors are opaque: never interpolate rejected arguments.
pub fn parse(args: &[&str]) -> Result<Command, Error> {
    match args {
        ["list"] => Ok(Command::List),
        ["check"] => Ok(Command::Check),
        ["rm", name] => Ok(Command::Remove {
            name: SecretName::new(name)?,
        }),
        ["set", name, options @ ..] => {
            let name = SecretName::new(name)?;
            let mut input = InputMode::Terminal;
            let mut expiry = None;
            let mut rest = options;
            while !rest.is_empty() {
                match rest {
                    ["--stdin", tail @ ..] if input == InputMode::Terminal => {
                        input = InputMode::Stdin;
                        rest = tail;
                    }
                    ["--expires-at", seconds, tail @ ..] if expiry.is_none() => {
                        expiry = Some(seconds.parse::<i64>().map_err(|_| Error::InvalidData)?);
                        rest = tail;
                    }
                    _ => return Err(Error::InvalidData),
                }
            }
            Ok(Command::Set {
                name,
                input,
                expires_at: expiry,
            })
        }
        _ => Err(Error::InvalidData),
    }
}

/// Injected input boundary; tests never need ambient stdin or an actual terminal.
pub trait Input {
    fn read(&mut self, mode: InputMode) -> Result<SecretValue, Error>;
}
#[derive(Debug, PartialEq, Eq)]
pub enum Output {
    Stored,
    Listed(Vec<SecretInfo>),
    Removed(bool),
    /// Local storage readability/schema/expiry check only, NOT provider validation.
    Checked {
        backend: &'static str,
        active_entries: usize,
    },
}
pub fn execute(
    store: &mut dyn SecretStore,
    command: Command,
    input: &mut dyn Input,
    now: i64,
) -> Result<Output, Error> {
    match command {
        Command::Set {
            name,
            input: mode,
            expires_at,
        } => {
            if expires_at.is_some_and(|e| e <= now) {
                return Err(Error::InvalidData);
            }
            if store.backend() == "env" {
                return Err(Error::ReadOnly);
            }
            let value = input.read(mode)?;
            if value.expose().is_empty() || value.expose().len() > MAX_VALUE {
                return Err(Error::InvalidData);
            }
            store.put(name, value, expires_at)?;
            Ok(Output::Stored)
        }
        Command::List => Ok(Output::Listed(store.list(now)?)),
        Command::Remove { name } => Ok(Output::Removed(store.delete(&name)?)),
        Command::Check => Ok(Output::Checked {
            backend: store.backend(),
            active_entries: store.list(now)?.len(),
        }),
    }
}

/// Explicit --stdin reads exact bytes through EOF (including any newline), never
/// implicitly selected for redirected input. Does not close the caller's reader.
pub fn read_stdin(reader: &mut dyn Read) -> Result<SecretValue, Error> {
    let mut bytes = Zeroizing::new(Vec::with_capacity(MAX_VALUE + 1));
    reader
        .take((MAX_VALUE + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Io)?;
    if bytes.is_empty() || bytes.len() > MAX_VALUE {
        return Err(Error::InvalidData);
    }
    Ok(SecretValue::new(std::mem::take(&mut *bytes)))
}
/// No OS access until Input::read is explicitly invoked. Terminal mode uses
/// /dev/tty, never falls back to stdin, and restores attributes on ordinary exits.
pub struct SystemInput;
impl Input for SystemInput {
    fn read(&mut self, mode: InputMode) -> Result<SecretValue, Error> {
        match mode {
            InputMode::Stdin => read_stdin(&mut std::io::stdin().lock()),
            InputMode::Terminal => {
                if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                    return Err(Error::Unavailable);
                }
                read_terminal()
            }
        }
    }
}
#[cfg(not(unix))]
fn read_terminal() -> Result<SecretValue, Error> {
    Err(Error::Unavailable)
}
#[cfg(unix)]
fn read_terminal() -> Result<SecretValue, Error> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOCTTY)
        .open("/dev/tty")
        .map_err(|_| Error::Unavailable)?;
    terminal_from(&mut tty)
}
#[cfg(unix)]
fn terminal_from(tty: &mut std::fs::File) -> Result<SecretValue, Error> {
    use std::os::fd::{AsRawFd, RawFd};
    struct Restore {
        fd: RawFd,
        original: libc::termios,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            // SAFETY: descriptor is borrowed from the still-live tty file.
            unsafe {
                libc::tcsetattr(self.fd, libc::TCSANOW, &self.original);
            }
        }
    }
    let fd = tty.as_raw_fd();
    let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
    // SAFETY: tcgetattr initializes termios on success.
    if unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } != 0 {
        return Err(Error::Unavailable);
    }
    // SAFETY: successful tcgetattr initialized the value.
    let original = unsafe { original.assume_init() };
    let guard = Restore { fd, original };
    let mut hidden = original;
    // Handle cancellation bytes ourselves so Ctrl-C cannot skip restoration.
    hidden.c_lflag &= !(libc::ECHO | libc::ECHONL | libc::ICANON | libc::ISIG);
    hidden.c_cc[libc::VMIN] = 1;
    hidden.c_cc[libc::VTIME] = 0;
    // SAFETY: live descriptor and initialized termios; discard previously queued input.
    if unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &hidden) } != 0 {
        return Err(Error::Io);
    }
    let result = (|| {
        let mut bytes = Zeroizing::new(Vec::with_capacity(MAX_VALUE));
        let mut byte = Zeroizing::new([0u8; 1]);
        loop {
            if tty.read(&mut *byte).map_err(|_| Error::Io)? != 1 {
                return Err(Error::Io);
            }
            match byte[0] {
                b'\n' | b'\r' => break,
                3 | 4 | 26 => return Err(Error::Io),
                8 | 127 => {
                    bytes.pop();
                }
                b => {
                    if bytes.len() == MAX_VALUE {
                        return Err(Error::InvalidData);
                    }
                    bytes.push(b);
                }
            }
        }
        if bytes.is_empty() {
            return Err(Error::InvalidData);
        }
        Ok(SecretValue::new(std::mem::take(&mut *bytes)))
    })();
    // Report restoration failure rather than claiming successful input.
    // SAFETY: descriptor and original attributes remain live.
    if unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &guard.original) } != 0 {
        return Err(Error::Io);
    }
    drop(guard);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KeyringDriver, KeyringStore};
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
                .map(|v| SecretValue::new(v.expose().to_vec())))
        }
        fn save(&self, value: &SecretValue) -> Result<(), Error> {
            *self.0.lock().unwrap() = Some(SecretValue::new(value.expose().to_vec()));
            Ok(())
        }
    }
    struct Synthetic;
    impl Input for Synthetic {
        fn read(&mut self, _: InputMode) -> Result<SecretValue, Error> {
            Ok(SecretValue::new(b"synthetic-input".to_vec()))
        }
    }
    #[test]
    fn harness_set_list_check_expire_remove() {
        let mut store = KeyringStore::injected(Mock::default());
        let mut input = Synthetic;
        assert_eq!(
            execute(
                &mut store,
                parse(&["set", "test/session", "--stdin", "--expires-at", "10"]).unwrap(),
                &mut input,
                0
            ),
            Ok(Output::Stored)
        );
        let out = execute(&mut store, parse(&["list"]).unwrap(), &mut input, 9).unwrap();
        assert!(!format!("{out:?}").contains("synthetic-input"));
        assert_eq!(
            execute(&mut store, Command::Check, &mut input, 10),
            Ok(Output::Checked {
                backend: "keychain",
                active_entries: 0
            })
        );
        assert_eq!(
            execute(
                &mut store,
                parse(&["rm", "test/session"]).unwrap(),
                &mut input,
                10
            ),
            Ok(Output::Removed(true))
        );
    }
    #[test]
    fn no_argv_values_or_get_show_and_stdin_is_explicit_bounded_exact() {
        for args in [
            vec!["get", "test/session"],
            vec!["show"],
            vec!["set", "test/session", "synthetic-input"],
            vec!["set", "test/session", "--value", "synthetic-input"],
            vec!["set", "test/session", "--stdin", "--stdin"],
        ] {
            assert!(parse(&args).is_err());
        }
        assert!(matches!(
            parse(&["set", "test/session"]).unwrap(),
            Command::Set {
                input: InputMode::Terminal,
                ..
            }
        ));
        assert_eq!(
            read_stdin(&mut &b"synthetic\n"[..]).unwrap().expose(),
            b"synthetic\n"
        );
        assert!(read_stdin(&mut &b""[..]).is_err());
        assert!(read_stdin(&mut &vec![b'x'; MAX_VALUE + 1][..]).is_err());
        assert_eq!(
            read_stdin(&mut &vec![b'x'; MAX_VALUE][..])
                .unwrap()
                .expose()
                .len(),
            MAX_VALUE
        );
    }
    #[test]
    fn invalid_expiry_and_read_only_fail_before_reading_input() {
        struct NeverRead;
        impl Input for NeverRead {
            fn read(&mut self, _: InputMode) -> Result<SecretValue, Error> {
                panic!("must not read input")
            }
        }
        let mut input = NeverRead;
        let mut store = crate::EnvStore::injected([]);
        let command = parse(&["set", "test/session", "--expires-at", "10"]).unwrap();
        assert_eq!(
            execute(&mut store, command, &mut input, 10),
            Err(Error::InvalidData)
        );
        let command = parse(&["set", "test/session", "--stdin"]).unwrap();
        assert_eq!(
            execute(&mut store, command, &mut input, 0),
            Err(Error::ReadOnly)
        );
    }
    #[cfg(unix)]
    #[test]
    fn dedicated_synthetic_pty_noecho_restores_on_success_and_cancel() {
        use std::{
            fs::File,
            io::Write,
            os::fd::{AsRawFd, FromRawFd},
        };
        for cancel in [false, true] {
            let (mut master, mut slave) = (-1, -1);
            // SAFETY: valid out pointers, null optional name/termios/winsize.
            assert_eq!(
                unsafe {
                    libc::openpty(
                        &mut master,
                        &mut slave,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                },
                0
            );
            // SAFETY: openpty returned owned descriptors.
            let mut master = unsafe { File::from_raw_fd(master) };
            // SAFETY: distinct owned slave descriptor from openpty.
            let mut slave = unsafe { File::from_raw_fd(slave) };
            let inspect = slave.try_clone().unwrap();
            fn attrs(f: &File) -> libc::termios {
                let mut t = std::mem::MaybeUninit::uninit();
                // SAFETY: live descriptor, valid output buffer.
                assert_eq!(unsafe { libc::tcgetattr(f.as_raw_fd(), t.as_mut_ptr()) }, 0);
                // SAFETY: tcgetattr succeeded.
                unsafe { t.assume_init() }
            }
            let before = attrs(&inspect);
            let thread = std::thread::spawn(move || terminal_from(&mut slave));
            let mut hidden = false;
            for _ in 0..1000 {
                let t = attrs(&inspect);
                if t.c_lflag & (libc::ECHO | libc::ICANON | libc::ISIG) == 0 {
                    hidden = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert!(hidden);
            if cancel {
                master.write_all(&[3]).unwrap();
            } else {
                master.write_all(b"synthetic-ptyx\x7f\n").unwrap();
            }
            let result = thread.join().unwrap();
            if cancel {
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap().expose(), b"synthetic-pty");
            }
            let after = attrs(&inspect);
            assert_eq!(before.c_lflag, after.c_lflag);
            assert_eq!(before.c_cc, after.c_cc);
            // No echo or other value output was written to the synthetic terminal.
            // SAFETY: valid fd and fcntl flags.
            assert_ne!(
                unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) },
                -1
            );
            let mut byte = [0];
            assert!(
                master
                    .read(&mut byte)
                    .is_err_and(|e| e.kind() == std::io::ErrorKind::WouldBlock)
            );
        }
    }
}
