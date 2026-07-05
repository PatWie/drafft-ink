//! Main application entry point (native).

#[cfg(feature = "native")]
fn main() {
    env_logger::init();
    log::info!("Starting DrafftInk");

    let (server, room) = parse_args();
    pollster::block_on(drafftink_app::App::run(server, room));
}

/// Parse `--server <url>` and `--room <id>` from the command line so the
/// desktop app can auto-connect and auto-join a collaboration room on startup.
#[cfg(feature = "native")]
fn parse_args() -> (Option<String>, Option<String>) {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut server = None;
    let mut room = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--server" if i + 1 < args.len() => {
                server = Some(args[i + 1].clone());
                i += 2;
            }
            "--room" if i + 1 < args.len() => {
                room = Some(args[i + 1].clone());
                i += 2;
            }
            _ => i += 1,
        }
    }
    (server, room)
}

#[cfg(not(feature = "native"))]
fn main() {
    panic!("Native feature not enabled. Use `cargo run --features native`");
}
