//! MCP server for coding agents (Claude Code, Codex): Streamable HTTP on 127.0.0.1, bearer token.
//! Agents can read, search, mark, archive and write drafts. They can't send.

use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::core::Core;
use crate::parse::html_to_text;
use crate::types::{DraftInput, ThreadRow, View};

const BODY_MAX: usize = 20_000;

#[derive(Clone, Debug, Serialize)]
pub struct McpInfo {
    pub enabled: bool,
    pub running: bool,
    pub error: Option<String>,
    pub url: String,
    pub token: String,
    pub claude_command: String,
    pub codex_config: String,
}

impl McpInfo {
    pub fn new(enabled: bool, running: bool, error: Option<String>, port: u16, token: &str) -> Self {
        let url = format!("http://127.0.0.1:{port}/mcp");
        Self {
            enabled,
            running,
            error,
            claude_command: format!("claude mcp add --transport http yamc {url} --header \"Authorization: Bearer {token}\""),
            codex_config: format!(
                "[mcp_servers.yamc]\nurl = \"{url}\"\nhttp_headers = {{ Authorization = \"Bearer {token}\" }}"
            ),
            url,
            token: token.to_string(),
        }
    }
}

pub struct McpServer {
    pub port: u16,
    pub token: String,
    cancel: CancellationToken,
}

impl McpServer {
    pub fn start(core: Arc<Core>, port: u16, token: String) -> Result<Self> {
        let rt = core.runtime().clone();
        let listener = std::net::TcpListener::bind(("127.0.0.1", port)).with_context(|| format!("port {port} is taken"))?;
        listener.set_nonblocking(true)?;
        let cancel = CancellationToken::new();
        let service = {
            let core = core.clone();
            StreamableHttpService::new(
                move || Ok(Tools::new(core.clone())),
                Arc::new(LocalSessionManager::default()),
                StreamableHttpServerConfig::default()
                    .with_legacy_session_mode(false)
                    .with_json_response(true)
                    .with_sse_keep_alive(None)
                    .with_cancellation_token(cancel.child_token()),
            )
        };
        let app = axum::Router::new()
            .nest_service("/mcp", service)
            .layer(axum::middleware::from_fn_with_state(Arc::<str>::from(token.as_str()), auth));
        let stop = cancel.clone();
        rt.spawn(async move {
            let listener = match tokio::net::TcpListener::from_std(listener) {
                Ok(l) => l,
                Err(e) => return eprintln!("MCP listener: {e}"),
            };
            if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(async move { stop.cancelled_owned().await }).await {
                eprintln!("MCP server: {e}");
            }
        });
        Ok(Self { port, token, cancel })
    }

    pub fn stop(&self) {
        self.cancel.cancel();
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

fn token_ok(header: Option<&str>, token: &str) -> bool {
    let got = header.unwrap_or("");
    let got = got.strip_prefix("Bearer ").or_else(|| got.strip_prefix("bearer ")).unwrap_or("").trim().as_bytes();
    let want = token.as_bytes();
    // Constant time over the token length.
    got.len() == want.len() && got.iter().zip(want).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

async fn auth(State(token): State<Arc<str>>, req: Request, next: Next) -> Response {
    let header = req.headers().get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok());
    if !token_ok(header, &token) {
        return (StatusCode::UNAUTHORIZED, axum::Json(json!({ "error": "unauthorized" }))).into_response();
    }
    next.run(req).await
}

fn ok(v: serde_json::Value) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::success(vec![ContentBlock::text(serde_json::to_string_pretty(&v).unwrap_or_default())]))
}

fn err(e: impl std::fmt::Display) -> ErrorData {
    ErrorData::invalid_params(e.to_string(), None)
}

fn iso(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).map(|d| d.to_rfc3339()).unwrap_or_default()
}

fn rows(list: &[ThreadRow]) -> serde_json::Value {
    list.iter()
        .map(|t| {
            json!({
                "account_id": t.account_id, "thread_id": t.thread_id, "subject": t.subject, "from": t.from,
                "snippet": t.snippet, "date": iso(t.date), "unread": t.unread, "messages": t.count,
            })
        })
        .collect()
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ListInbox {
    /// Folder to list instead of the shared inbox (from list_accounts).
    mailbox_id: Option<i64>,
    unread_only: Option<bool>,
    /// 1–500, default 50.
    limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct Search {
    query: String,
    /// 1–200, default 25.
    limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ThreadRef {
    account_id: i64,
    thread_id: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct MarkRead {
    account_id: i64,
    thread_id: String,
    /// false marks it unread. Default true.
    read: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct CreateDraft {
    /// Required unless replying.
    account_id: Option<i64>,
    reply_to_message_id: Option<i64>,
    reply_all: Option<bool>,
    /// Comma-separated addresses.
    to: Option<String>,
    cc: Option<String>,
    subject: Option<String>,
    /// Plain text.
    body: String,
}

#[derive(Clone)]
pub struct Tools {
    core: Arc<Core>,
}

#[tool_router]
impl Tools {
    pub fn new(core: Arc<Core>) -> Self {
        Self { core }
    }

    #[tool(description = "List mail accounts and their folders.")]
    async fn list_accounts(&self) -> Result<CallToolResult, ErrorData> {
        let boxes = self.core.list_mailboxes();
        let list: Vec<_> = self
            .core
            .list_accounts()
            .into_iter()
            .map(|a| {
                let folders: Vec<_> = boxes
                    .iter()
                    .filter(|b| b.account_id == a.id)
                    .map(|b| json!({ "mailbox_id": b.id, "path": b.path, "special_use": b.special_use }))
                    .collect();
                json!({ "id": a.id, "name": a.name, "sender_name": a.sender, "email": a.email, "kind": a.kind, "state": a.state, "folders": folders })
            })
            .collect();
        ok(json!(list))
    }

    #[tool(description = "List conversations in the shared inbox (all accounts), unread first then newest. Optionally a single folder.")]
    async fn list_inbox(&self, Parameters(p): Parameters<ListInbox>) -> Result<CallToolResult, ErrorData> {
        let view = p.mailbox_id.map(View::Mailbox).unwrap_or(View::Inbox);
        let mut list = self.core.list_threads(&view, 0, p.limit.unwrap_or(50).clamp(1, 500));
        if p.unread_only.unwrap_or(false) {
            list.retain(|t| t.unread);
        }
        ok(rows(&list))
    }

    #[tool(description = "Full-text search across all accounts (subject, sender, recipients, downloaded bodies). Newest first.")]
    async fn search(&self, Parameters(p): Parameters<Search>) -> Result<CallToolResult, ErrorData> {
        ok(rows(&self.core.list_threads(&View::Search(p.query), 0, p.limit.unwrap_or(25).clamp(1, 200))))
    }

    #[tool(description = "Read a whole conversation as plain text. Downloads bodies that are not cached yet. Does not mark it read.")]
    async fn get_thread(&self, Parameters(p): Parameters<ThreadRef>) -> Result<CallToolResult, ErrorData> {
        let t = self.core.get_thread(p.account_id, &p.thread_id);
        let mut messages = Vec::new();
        for m in &t.messages {
            let full = self.core.load_message(m.id).await.unwrap_or_else(|| m.clone());
            let mut text = full.text.clone().unwrap_or_else(|| full.html.as_deref().map(html_to_text).unwrap_or_default());
            if text.len() > BODY_MAX {
                let cut = (0..=BODY_MAX).rev().find(|i| text.is_char_boundary(*i)).unwrap_or(0);
                text.truncate(cut);
                text.push_str("\n[truncated]");
            }
            messages.push(json!({
                "message_id": full.id, "from": full.from, "to": full.to, "cc": full.cc, "date": iso(full.date),
                "subject": full.subject, "unread": !full.seen, "attachments": full.attachments, "body": text,
            }));
        }
        ok(json!({ "account_id": t.account_id, "thread_id": t.thread_id, "subject": t.subject, "messages": messages }))
    }

    #[tool(
        description = "Create a draft for the user to review and send in yamc. Plain text body. For a reply, pass reply_to_message_id (from get_thread) and the recipients/subject are filled in unless given."
    )]
    async fn create_draft(&self, Parameters(a): Parameters<CreateDraft>) -> Result<CallToolResult, ErrorData> {
        let draft = if let Some(mid) = a.reply_to_message_id {
            let base = self.core.reply_draft(mid, a.reply_all.unwrap_or(false)).map_err(err)?;
            DraftInput {
                to: a.to.unwrap_or(base.to),
                cc: a.cc.unwrap_or(base.cc),
                subject: a.subject.unwrap_or(base.subject),
                body: format!("{}{}", a.body, base.body),
                ..base
            }
        } else {
            let account_id = a.account_id.ok_or_else(|| err("account_id is required for a new message"))?;
            DraftInput {
                id: None,
                account_id,
                to: a.to.unwrap_or_default(),
                cc: a.cc.unwrap_or_default(),
                subject: a.subject.unwrap_or_default(),
                body: a.body,
                reply_to: None,
                created_by: None,
                attachments: Vec::new(),
            }
        };
        // Agents never attach files: a draft must not be able to carry your files out.
        let id = self.core.save_draft(&DraftInput { created_by: Some("agent".into()), attachments: Vec::new(), ..draft }).map_err(err)?;
        ok(json!({ "draft_id": id, "note": "Draft saved. The user will review and send it from yamc." }))
    }

    #[tool(description = "List drafts waiting in yamc.")]
    async fn list_drafts(&self) -> Result<CallToolResult, ErrorData> {
        ok(json!(self.core.list_drafts()))
    }

    #[tool(description = "Mark a conversation read (or unread with read=false).")]
    async fn mark_read(&self, Parameters(p): Parameters<MarkRead>) -> Result<CallToolResult, ErrorData> {
        self.core.mark_thread(p.account_id, &p.thread_id, p.read.unwrap_or(true)).map_err(err)?;
        ok(json!({ "ok": true }))
    }

    #[tool(description = "Archive a conversation (remove it from the inbox).")]
    async fn archive(&self, Parameters(p): Parameters<ThreadRef>) -> Result<CallToolResult, ErrorData> {
        self.core.archive_thread(p.account_id, &p.thread_id).map_err(err)?;
        ok(json!({ "ok": true }))
    }
}

#[tool_handler]
impl ServerHandler for Tools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new("yamc", env!("CARGO_PKG_VERSION")))
            .with_instructions(
            "yamc is the user's local mail client. Threads are identified by (account_id, thread_id). \
             You cannot send mail: create_draft puts a draft in the app and the user reviews and sends it.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::token_ok;

    #[test]
    fn token() {
        assert!(token_ok(Some("Bearer abc"), "abc"));
        assert!(!token_ok(Some("Bearer abd"), "abc"));
        assert!(!token_ok(Some("abc"), "abc"));
        assert!(!token_ok(None, "abc"));
    }
}
