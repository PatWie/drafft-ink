//! The MCP wire: the official `rmcp` SDK serving a [`Service`] over stdio (`drafftink mcp`, `drafftink-headless mcp`) or streamable HTTP
//! ([`super::http`]).
//!
//! Why the SDK and not a hand-written JSON-RPC loop: the protocol keeps changing (the current schema is the
//! 2026-07-28 revision, with new result types and version negotiation), and the SDK follows it. The tool list
//! is generated from the registry at run time, so the SDK's `#[tool]` macros are not used; the handler below
//! is the small part we write. The logic lives in [`Service`] and is tested without the SDK.

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, GetPromptRequestParams, GetPromptResponse,
    GetPromptResult, Implementation, ListPromptsResult, ListToolsResult, PaginatedRequestParams, Prompt, PromptMessage,
    Role, ServerCapabilities, ServerConfig, Tool as McpTool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::Value;

use crate::tools::catalog::Tool;
use crate::tools::guide::PROMPT_NAME;
use crate::tools::outcome::{Block, Outcome};
use crate::tools::service::Service;

/// The handler that rmcp drives.
#[derive(Clone)]
pub struct DrafftHandler {
    service: Arc<Service>,
}

impl DrafftHandler {
    /// A handler over a service.
    pub fn new(service: Arc<Service>) -> Self {
        Self { service }
    }
}

fn mcp_tool(t: &Tool) -> McpTool {
    let schema = match &t.input_schema {
        Value::Object(m) => m.clone(),
        _ => serde_json::Map::new(),
    };
    let mut annotations = ToolAnnotations::with_title(t.title.clone()).read_only(t.hints.read_only);
    annotations.destructive_hint = Some(t.hints.destructive);
    annotations.idempotent_hint = Some(t.hints.read_only);
    annotations.open_world_hint = Some(t.hints.open_world);
    McpTool::new(t.name.clone(), t.description.clone(), Arc::new(schema))
        .with_title(t.title.clone())
        .with_annotations(annotations)
}

fn result(o: Outcome) -> CallToolResult {
    let content = o
        .blocks
        .into_iter()
        .map(|b| match b {
            Block::Text(t) => ContentBlock::text(t),
            Block::Image { data, mime } => ContentBlock::image(data, mime),
        })
        .collect();
    if o.is_error { CallToolResult::error(content) } else { CallToolResult::success(content) }
}

impl ServerHandler for DrafftHandler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().enable_prompts().build())
            .with_server_info(Implementation::new("drafftink-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(self.service.instructions())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let tools = self.service.catalog().tools().iter().map(mcp_tool).collect();
        Ok(ListToolsResult { tools, ..ListToolsResult::default() })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args = request.arguments.unwrap_or_default();
        match self.service.call(&request.name, args).await {
            Ok(outcome) => Ok(CallToolResponse::from(result(outcome))),
            Err(unknown) => Err(ErrorData::invalid_params(unknown.to_string(), None)),
        }
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        let prompt = Prompt::new(
            PROMPT_NAME,
            Some(
                "A one-page guide to drawing in drafft.ink: look with doc.outline, build with diagram.create, edit in a batch, undo, and read refusals.",
            ),
            None,
        );
        Ok(ListPromptsResult { prompts: vec![prompt], ..ListPromptsResult::default() })
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        if request.name != PROMPT_NAME {
            return Err(ErrorData::invalid_params(
                format!("unknown prompt `{}`. The prompt is `{PROMPT_NAME}`", request.name),
                None,
            ));
        }
        let message = PromptMessage::new(Role::User, ContentBlock::text(self.service.guide()));
        Ok(GetPromptResponse::from(GetPromptResult::new(vec![message]).with_description("How to draw with drafft.ink")))
    }
}
