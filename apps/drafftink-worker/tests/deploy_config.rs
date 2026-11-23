//! The deployment template sends the routes of the relay to the Worker. A path that is missing from
//! `run_worker_first` is answered by the static assets instead (the app's page), and the app then waits for a
//! handshake that never comes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[test]
fn every_environment_runs_the_worker_first_for_the_relay_routes() {
    let template =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../deploy/cloudflare/wrangler.toml.in"))
            .unwrap();
    let lines: Vec<&str> = template.lines().filter(|l| l.trim_start().starts_with("run_worker_first")).collect();
    assert_eq!(lines.len(), 3, "the default, demo and private environments: {lines:?}");
    for line in lines {
        for path in ["/r/*", "/api/*", "/ws"] {
            assert!(line.contains(&format!("\"{path}\"")), "{path} is missing from: {line}");
        }
    }
}
