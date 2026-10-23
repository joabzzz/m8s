//! Separate cleanup process. No create, credential discovery, or Ready grant.
//! The caller must durably reserve its original liability before arming this
//! process, and must not create until the `armed` receipt has been received.
fn main() {
    #[cfg(unix)]
    let result = {
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        if args.len() != 2 || args[0] != "--plan" {
            Err(())
        } else {
            m8s_provider::vast::watchdog::run(std::path::Path::new(&args[1])).map_err(|_| ())
        }
    };
    #[cfg(not(unix))]
    let result: Result<(), ()> = Err(());
    if result.is_err() {
        // Never print provider responses, paths, arguments, or credentials.
        eprintln!("Vast watchdog incomplete; retain original cleanup liability");
        std::process::exit(1);
    }
}
