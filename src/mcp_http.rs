//! The desktop's MCP server over streamable HTTP (`docs/agents.md`), started by `--mcp-http` or by the
//! setting `agent.mcp_http`. The app owns the lifecycle; the server itself is `drafftink_bridge::mcp::http`.

use drafftink_app::{McpHttpControl, McpHttpInfo};
use drafftink_bridge::host::JobSender;
use drafftink_bridge::mcp::http::{DEFAULT_PORT, McpHttpConfig, McpHttpServer};
use drafftink_bridge::server::BridgeError;

/// Starts and stops the server on the shell's job queue.
pub(crate) struct McpHttp {
    jobs: JobSender,
    port: u16,
    server: Option<McpHttpServer>,
}

impl McpHttp {
    /// A stopped server that will listen on `port` (0: a free one), or on [`DEFAULT_PORT`].
    pub(crate) fn new(jobs: JobSender, port: Option<u16>) -> Self {
        Self { jobs, port: port.unwrap_or(DEFAULT_PORT), server: None }
    }

    /// Starts listening and tells the user, on stderr, how to connect.
    pub(crate) fn start_announced(&mut self) -> Result<McpHttpInfo, BridgeError> {
        let server = McpHttpServer::start(McpHttpConfig::on_port(self.port), self.jobs.clone())?;
        server.announce_to_stderr();
        let info = info_of(&server);
        self.server = Some(server);
        Ok(info)
    }
}

fn info_of(server: &McpHttpServer) -> McpHttpInfo {
    McpHttpInfo { endpoint: server.url(), token_file: server.token_file().map(|p| p.display().to_string()) }
}

/// The words for the preferences when the server could not start.
fn problem(error: &BridgeError) -> String {
    match error {
        BridgeError::Bind(addr, _) => format!(
            "Could not listen on {addr}: another program may use that port. Start drafftink with --mcp-http <port> to choose another."
        ),
        other => format!("The MCP server could not start: {other}"),
    }
}

impl McpHttpControl for McpHttp {
    fn start(&mut self) -> Result<McpHttpInfo, String> {
        match &self.server {
            Some(server) => Ok(info_of(server)),
            None => self.start_announced().map_err(|e| {
                eprintln!("drafftink: {e}");
                problem(&e)
            }),
        }
    }

    fn stop(&mut self) {
        if let Some(server) = self.server.take() {
            server.stop();
            eprintln!("drafftink mcp http: stopped");
        }
    }

    fn running(&self) -> Option<McpHttpInfo> {
        self.server.as_ref().map(info_of)
    }
}
