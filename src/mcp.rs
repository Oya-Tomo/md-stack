//! `md-stack mcp`: stdio MCP server that accepts posts from Claude Code (docs/SPEC.md §5).

use std::fmt::Write as _;

use anyhow::Result;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::document;
use crate::math::MathRenderer;
use crate::store::{MathEntry, NewMath, Store};

const INSTRUCTIONS: &str = "\
md-stack shows Markdown in a separate terminal viewer that renders math and lets the user copy \
code blocks. Whenever a response would contain a fenced code block or math ($...$ or $$...$$), \
send the whole response, including the surrounding explanation, with the `post` tool instead of \
writing it in the terminal. Then write only `→ md-stack #N` and a one-line summary in the \
terminal. Math uses KaTeX syntax. If `post` reports errors, fix the listed expressions and post \
again; nothing is saved from a rejected post.";

#[derive(Deserialize, JsonSchema)]
struct PostParams {
    /// The response as Markdown (CommonMark + GFM). Math: `$...$` inline, `$$...$$` display.
    markdown: String,
    /// Short title for the post list. Defaults to the first heading or line.
    title: Option<String>,
}

struct Server {
    store: Store,
    math: MathRenderer,
    /// The Claude Code process that spawned this server.
    claude_pid: u32,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Server {
    fn new(store: Store) -> Self {
        Self {
            store,
            math: MathRenderer::spawn(),
            claude_pid: std::os::unix::process::parent_id(),
            tool_router: Self::tool_router(),
        }
    }

    /// Post a Markdown response to the md-stack viewer. Returns the post number.
    #[tool]
    async fn post(&self, Parameters(params): Parameters<PostParams>) -> CallToolResult {
        match self.try_post(params).await {
            Ok(id) => CallToolResult::success(vec![ContentBlock::text(format!(
                "Posted as md-stack #{id}."
            ))]),
            Err(PostError::Rejected(reason)) => {
                CallToolResult::error(vec![ContentBlock::text(reason)])
            }
            Err(PostError::Internal(e)) => {
                CallToolResult::error(vec![ContentBlock::text(format!("md-stack error: {e:#}"))])
            }
        }
    }
}

/// Why a post was not stored.
enum PostError {
    /// The post itself is at fault; Claude should fix it and post again.
    Rejected(String),
    /// md-stack failed.
    Internal(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for PostError {
    fn from(error: E) -> Self {
        Self::Internal(error.into())
    }
}

impl Server {
    async fn try_post(&self, params: PostParams) -> Result<u32, PostError> {
        // Resolved on every call: `/clear` switches the conversation without restarting us.
        let Some(process) = self.store.live_process(self.claude_pid)? else {
            return Err(PostError::Rejected(format!(
                "md-stack does not know the current conversation of Claude Code process {}. \
                 The `md-stack hook` SessionStart hook must be installed; it takes effect after \
                 Claude Code restarts.",
                self.claude_pid
            )));
        };

        let sources = document::math_sources(&params.markdown);
        let outcomes = self
            .math
            .render(sources.iter().map(|s| (s.tex.clone(), s.display)).collect())
            .await?;

        let mut math = Vec::with_capacity(sources.len());
        let mut failures = String::new();
        let mut failure_count = 0;
        for (source, outcome) in sources.into_iter().zip(outcomes) {
            match outcome {
                Ok(rendered) => math.push(NewMath {
                    entry: MathEntry {
                        display: source.display,
                        tex: source.tex,
                        metrics: rendered.metrics,
                    },
                    svg: rendered.svg,
                }),
                Err(message) => {
                    failure_count += 1;
                    let kind = if source.display { "display" } else { "inline" };
                    write!(
                        failures,
                        "\n[{failure_count}] {kind} math, line {}\n    source: {}\n    error:  {message}\n",
                        source.line,
                        source.tex.trim()
                    )?;
                }
            }
        }
        if failure_count > 0 {
            return Err(PostError::Rejected(format!(
                "Post rejected: {failure_count} math expression(s) failed to render. \
                 Nothing was saved; fix and post again.\n{failures}"
            )));
        }

        let title = params
            .title
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| document::default_title(&params.markdown));
        Ok(self
            .store
            .add_post(&process, title, &params.markdown, math)?)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("md-stack", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}

pub fn run() -> Result<()> {
    let store = Store::open()?;
    store.collect_garbage()?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let service = Server::new(store).serve(rmcp::transport::stdio()).await?;
            service.waiting().await?;
            Ok(())
        })
}
