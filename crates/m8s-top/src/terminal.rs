//! Private terminal scope used only by the explicit app runner. Rendering is pure.
use std::io;
pub(crate) trait Modes {
    fn raw_on(&mut self) -> io::Result<()>;
    fn screen_on(&mut self) -> io::Result<()>;
    fn screen_off(&mut self) -> io::Result<()>;
    fn raw_off(&mut self) -> io::Result<()>;
}
pub(crate) struct TerminalGuard<M: Modes> {
    modes: M,
    raw: bool,
    screen: bool,
}
impl<M: Modes> TerminalGuard<M> {
    pub(crate) fn enter(modes: M) -> io::Result<Self> {
        let mut guard = Self {
            modes,
            raw: false,
            screen: false,
        };
        // Mark before attempting: an I/O failure can occur after partial changes.
        guard.raw = true;
        guard.modes.raw_on()?;
        guard.screen = true;
        guard.modes.screen_on()?;
        Ok(guard)
    }
}
impl<M: Modes> Drop for TerminalGuard<M> {
    fn drop(&mut self) {
        if self.screen {
            let _ = self.modes.screen_off();
        }
        if self.raw {
            let _ = self.modes.raw_off();
        }
    }
}
pub(crate) struct RealModes;
impl Modes for RealModes {
    fn raw_on(&mut self) -> io::Result<()> {
        crossterm::terminal::enable_raw_mode()
    }
    fn screen_on(&mut self) -> io::Result<()> {
        crossterm::execute!(
            io::stdout(),
            crossterm::terminal::EnterAlternateScreen,
            crossterm::cursor::Hide
        )
    }
    fn screen_off(&mut self) -> io::Result<()> {
        use io::Write;
        // Try both restoration operations even if the first write fails.
        let mut out = io::stdout();
        let show = crossterm::execute!(out, crossterm::cursor::Show);
        let leave = crossterm::execute!(out, crossterm::terminal::LeaveAlternateScreen);
        let flush = out.flush();
        show.and(leave).and(flush)
    }
    fn raw_off(&mut self) -> io::Result<()> {
        crossterm::terminal::disable_raw_mode()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    struct Fake {
        calls: Arc<Mutex<Vec<&'static str>>>,
        fail: &'static str,
    }
    impl Fake {
        fn operation(&self, name: &'static str) -> io::Result<()> {
            self.calls.lock().unwrap().push(name);
            if self.fail == name {
                Err(io::Error::other("synthetic terminal error"))
            } else {
                Ok(())
            }
        }
    }
    impl Modes for Fake {
        fn raw_on(&mut self) -> io::Result<()> {
            self.operation("raw_on")
        }
        fn screen_on(&mut self) -> io::Result<()> {
            self.operation("screen_on")
        }
        fn screen_off(&mut self) -> io::Result<()> {
            self.operation("screen_off")
        }
        fn raw_off(&mut self) -> io::Result<()> {
            self.operation("raw_off")
        }
    }
    #[test]
    fn terminal_restored_on_normal_error_and_partial_entry_paths() {
        for (fail, expected) in [
            ("", vec!["raw_on", "screen_on", "screen_off", "raw_off"]),
            (
                "screen_off",
                vec!["raw_on", "screen_on", "screen_off", "raw_off"],
            ),
            (
                "screen_on",
                vec!["raw_on", "screen_on", "screen_off", "raw_off"],
            ),
            ("raw_on", vec!["raw_on", "raw_off"]),
        ] {
            let calls = Arc::new(Mutex::new(vec![]));
            let scope = TerminalGuard::enter(Fake {
                calls: calls.clone(),
                fail,
            });
            drop(scope); // Includes simulated application error/unwind scope exit.
            assert_eq!(*calls.lock().unwrap(), expected);
        }
    }
}
