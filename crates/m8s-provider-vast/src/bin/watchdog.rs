//! Separate cleanup process. No create, credential discovery, or Ready grant.
//! The caller must durably reserve its original liability before arming this
//! process, and must not create until the `armed` receipt has been received.
#[cfg(unix)]
#[path = "watchdog/unix.rs"]
mod unix;

fn main() {
    #[cfg(unix)]
    let result = unix::run();
    #[cfg(not(unix))]
    let result: Result<(), ()> = Err(());
    if result.is_err() {
        // Never print provider responses, paths, arguments, or credentials.
        eprintln!("Vast watchdog incomplete; retain original cleanup liability");
        std::process::exit(1);
    }
}
