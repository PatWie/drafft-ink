//! `drafftink-mcp`: an MCP server over stdio. See `docs/agents.md`.

use std::process::ExitCode;

fn main() -> ExitCode {
    drafftink_mcp::cli::main()
}
