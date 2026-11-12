//! MCP for the desktop app (native only): the `rmcp` handler for the generated tools, and a streamable HTTP
//! server on loopback that the app starts when the user asks for it (`drafftink --mcp-http`, or the setting
//! `agent.mcp_http`). The handler is also what `drafftink mcp` and `drafftink-headless mcp` serve over stdio.

pub mod handler;
pub mod http;
pub mod local;
