// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
pub mod de;
pub mod tools;
pub mod types;

use std::sync::Arc;

use anyhow::Result;
use base64::Engine;
use rmcp::handler::server::router::prompt::PromptRouter;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{Icon, Implementation, PromptMessage, Role, ServerCapabilities, ServerConfig};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ServerHandler, ServiceExt, prompt, prompt_handler, prompt_router, tool_handler};
use tokio_util::sync::CancellationToken;

use crate::app::App;

pub const INSTRUCTIONS: &str = include_str!("../../assets/instructions.md");
pub const INIT_PROMPT: &str = include_str!("../../assets/prompt_init.md");
pub const INIT_PROMPT_DESCRIPTION: &str =
    "Set up MPriMCP: supermarkets, products, price targets, notifications, schedule";
const AVATAR_SVG: &[u8] = include_bytes!("../../assets/static/avatar.svg");

pub fn instructions() -> String {
    INSTRUCTIONS.replace("\r\n", "\n").trim_end().to_string()
}

#[derive(Clone)]
pub struct MprServer {
    pub app: App,
    tool_router: ToolRouter<MprServer>,
    prompt_router: PromptRouter<MprServer>,
}

impl MprServer {
    pub fn new(app: App) -> Self {
        Self { app, tool_router: Self::tool_router(), prompt_router: Self::prompt_router() }
    }

    fn icons(&self) -> Vec<Icon> {
        let svg = base64::engine::general_purpose::STANDARD.encode(AVATAR_SVG);
        let mut icons = Vec::new();
        if self.app.mode == crate::config::Mode::Serve {
            let base = &self.app.config.public_url;
            icons.push(
                Icon::new(format!("{base}/static/avatar-512.png"))
                    .with_mime_type("image/png")
                    .with_sizes(vec!["512x512".into()]),
            );
            icons.push(
                Icon::new(format!("{base}/static/avatar-192.png"))
                    .with_mime_type("image/png")
                    .with_sizes(vec!["192x192".into()]),
            );
        }
        icons.push(
            Icon::new(format!("data:image/svg+xml;base64,{svg}"))
                .with_mime_type("image/svg+xml")
                .with_sizes(vec!["any".into()]),
        );
        icons
    }
}

// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
#[prompt_router]
impl MprServer {
    #[prompt(
        name = "init",
        description = "Set up MPriMCP: supermarkets, products, price targets, notifications, schedule"
    )]
    async fn init(&self) -> Vec<PromptMessage> {
        vec![PromptMessage::new_text(Role::User, INIT_PROMPT.replace("\r\n", "\n").trim_end())]
    }
}

#[tool_handler(router = self.tool_router)]
#[prompt_handler(router = self.prompt_router)]
impl ServerHandler for MprServer {
    fn get_info(&self) -> ServerConfig {
        let mut info = Implementation::new("mprimcp", env!("CARGO_PKG_VERSION"))
            .with_title("MPriMCP price tracker")
            .with_icons(self.icons());
        if self.app.mode == crate::config::Mode::Serve {
            info = info.with_website_url(self.app.config.public_url.clone());
        }
        ServerConfig::new(ServerCapabilities::builder().enable_tools().enable_prompts().build())
            .with_server_info(info)
            .with_instructions(instructions())
    }
}

pub async fn run_stdio(app: App) -> Result<()> {
    let server = MprServer::new(app.clone());
    let service = server.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    app.notifier.shutdown(std::time::Duration::from_secs(5)).await;
    Ok(())
}

pub fn http_service(app: App, cancel: CancellationToken) -> StreamableHttpService<MprServer, LocalSessionManager> {
    let config = &app.config;
    let authority = config.public_authority();
    let host = config.public_host();
    let mut hosts = vec!["localhost".to_string(), "127.0.0.1".into(), "::1".into(), host.clone()];
    if !authority.is_empty() && authority != host {
        hosts.push(authority);
    }
    let origin = config.public_url.clone();
    let http = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_allowed_hosts(hosts)
        .with_allowed_origins([origin, "http://localhost:*".into(), "http://127.0.0.1:*".into()])
        .with_cancellation_token(cancel);
    let prototype = MprServer::new(app);
    StreamableHttpService::new(move || Ok(prototype.clone()), Arc::new(LocalSessionManager::default()), http)
}
