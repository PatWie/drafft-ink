//! Skipped tests are loud, counted and fatal when the machine must not skip.
//!
//! A test that needs a GPU, a display or the old app cannot run everywhere. It must not pass silently:
//! it prints a line that starts with [`MARKER`], and `cargo xtask test` counts those lines and prints the
//! total. When the environment says the machine has the thing (`DRAFFTINK_REQUIRE_GPU=1`), the same
//! situation fails the test instead.

/// Every skip prints a line that starts with this word. `cargo xtask` counts them.
pub const MARKER: &str = "SKIPPED:";

/// The environment variable that says the machine has a GPU adapter, so that a skip is a failure.
pub const REQUIRE_GPU_ENV: &str = "DRAFFTINK_REQUIRE_GPU";

/// True when the variable `name` is set to something other than the empty string and `0`.
/// `1`, `true` and `yes` are on; `0` and the empty string are off. One rule for every crate and tool.
#[cfg(not(target_arch = "wasm32"))]
pub fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| flag_value(&v))
}

/// The environment is not readable in the browser.
#[cfg(target_arch = "wasm32")]
pub fn env_flag(_name: &str) -> bool {
    false
}

/// The rule of [`env_flag`] on a value.
pub fn flag_value(value: &str) -> bool {
    !value.is_empty() && value != "0"
}

/// True when `DRAFFTINK_REQUIRE_GPU` says that a missing GPU adapter fails tests instead of skipping them.
pub fn gpu_required() -> bool {
    env_flag(REQUIRE_GPU_ENV)
}

/// The line a skip prints, for the test called `test`.
pub fn skip_line(test: &str, reason: &str) -> String {
    format!("{MARKER} {test}: {reason}")
}

/// Prints the skip line for the running test (named after the current thread, which is the test name under
/// the default harness and under nextest).
pub fn report_skip(reason: &str) {
    let thread = std::thread::current();
    eprintln!("{}", skip_line(thread.name().unwrap_or("(unnamed test)"), reason));
}

/// A test that needs a GPU adapter found none. Prints the skip line and returns `Ok`, or returns the failure
/// message when `DRAFFTINK_REQUIRE_GPU` says the machine must have one.
///
/// # Errors
/// The message to fail the test with, when a GPU is required.
pub fn skip_without_gpu(why: &str) -> Result<(), String> {
    if gpu_required() {
        return Err(format!("no GPU adapter and {REQUIRE_GPU_ENV} is set: {why}"));
    }
    report_skip(&format!("no GPU adapter on this machine ({why}); set {REQUIRE_GPU_ENV}=1 to fail instead"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_rule_is_the_same_everywhere() {
        for on in ["1", "true", "yes", "2"] {
            assert!(flag_value(on), "{on}");
        }
        for off in ["", "0"] {
            assert!(!flag_value(off), "{off:?}");
        }
    }

    #[test]
    fn a_skip_line_starts_with_the_marker_and_names_the_test() {
        let line = skip_line("gpu::draws", "no adapter");
        assert_eq!(line, "SKIPPED: gpu::draws: no adapter");
        assert!(line.starts_with(MARKER));
    }
}
