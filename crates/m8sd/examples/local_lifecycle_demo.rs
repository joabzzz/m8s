//! Explicit harmless LOCAL walkthrough. Synthetic accounting is NOT approval.
#[cfg(unix)]
fn main() {
    let args = std::env::args_os().skip(1).take(17).collect();
    let mut out = std::io::stdout().lock();
    std::process::exit(m8sd::local_demo::run_cli(args, &mut out));
}
#[cfg(not(unix))]
fn main() {
    eprintln!("local lifecycle demo unavailable on this platform");
    std::process::exit(1);
}
