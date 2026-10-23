//! Standalone adapter; shared application logic also powers `m8s top` in-process.
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if let Err(error) = m8s_cli::dashboard::app::Options::parse(&args)
        .map_err(str::to_owned)
        .and_then(m8s_cli::dashboard::app::run)
    {
        eprintln!("m8s-top: {error}");
        std::process::exit(1);
    }
}
