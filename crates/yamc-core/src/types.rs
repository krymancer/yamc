//! Data shared by the store, sync engine, MCP server and UI.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountKind {
    Gmail,
    Proton,
    Outlook,
    Imap,
}

impl AccountKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gmail => "gmail",
            Self::Proton => "proton",
            Self::Outlook => "outlook",
            Self::Imap => "imap",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "gmail" => Self::Gmail,
            "proton" => Self::Proton,
            "outlook" => Self::Outlook,
            _ => Self::Imap,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TlsMode {
    Tls,
    Starttls,
    None,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub tls: TlsMode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountConfig {
    pub imap: ServerConfig,
    pub smtp: ServerConfig,
    pub username: String,
    /// Accept self-signed certificates. Only honoured for loopback hosts (Proton Bridge).
    #[serde(default)]
    pub allow_self_signed: bool,
    /// Whether the server stores sent mail itself (Gmail, Outlook, Proton do).
    #[serde(default)]
    pub server_saves_sent: bool,
}

#[derive(Clone, Debug)]
pub struct AccountInput {
    pub name: String,
    pub email: String,
    pub kind: AccountKind,
    pub config: AccountConfig,
    pub password: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncState {
    Idle,
    Syncing,
    Error,
    Offline,
}

#[derive(Clone, Debug)]
pub struct Account {
    pub id: i64,
    pub name: String,
    pub email: String,
    pub kind: AccountKind,
    pub config: AccountConfig,
    pub state: SyncState,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mailbox {
    pub id: i64,
    pub account_id: i64,
    pub path: String,
    pub name: String,
    pub special_use: Option<String>,
    pub depth: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub address: String,
}

impl Address {
    /// `Name <addr>` or just `addr`.
    pub fn formatted(&self) -> String {
        match &self.name {
            Some(n) if !n.is_empty() => format!("{n} <{}>", self.address),
            _ => self.address.clone(),
        }
    }

    pub fn display(&self) -> &str {
        match &self.name {
            Some(n) if !n.is_empty() => n,
            _ => &self.address,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum View {
    Inbox,
    Mailbox(i64),
    Search(String),
    Drafts,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadRow {
    pub account_id: i64,
    pub thread_id: String,
    pub subject: String,
    /// Display names of senders, latest first.
    pub from: String,
    pub snippet: String,
    pub date: i64,
    pub unread: bool,
    pub count: i64,
}

impl ThreadRow {
    pub fn key(&self) -> ThreadKey {
        ThreadKey(self.account_id, self.thread_id.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ThreadKey(pub i64, pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
}

#[derive(Clone, Debug)]
pub struct Message {
    pub id: i64,
    pub account_id: i64,
    pub mailbox_id: i64,
    pub message_id: Option<String>,
    pub subject: String,
    pub from: Option<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub date: i64,
    pub seen: bool,
    pub has_body: bool,
    pub html: Option<String>,
    pub text: Option<String>,
    pub attachments: Vec<Attachment>,
    pub snippet: String,
    pub references: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Thread {
    pub account_id: i64,
    pub thread_id: String,
    pub subject: String,
    pub messages: Vec<Message>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub id: i64,
    pub account_id: i64,
    pub to: String,
    pub cc: String,
    pub subject: String,
    pub body: String,
    /// Local message id being replied to, if any.
    pub reply_to: Option<i64>,
    /// `user` or `agent`.
    pub created_by: String,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DraftInput {
    pub id: Option<i64>,
    pub account_id: i64,
    pub to: String,
    pub cc: String,
    pub subject: String,
    pub body: String,
    pub reply_to: Option<i64>,
    pub created_by: Option<String>,
}

impl From<&Draft> for DraftInput {
    fn from(d: &Draft) -> Self {
        Self {
            id: Some(d.id),
            account_id: d.account_id,
            to: d.to.clone(),
            cc: d.cc.clone(),
            subject: d.subject.clone(),
            body: d.body.clone(),
            reply_to: d.reply_to,
            created_by: Some(d.created_by.clone()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Theme id (`zinc-dark`, `mocha`, …) or `system`.
    pub theme: String,
    /// Accent for Catppuccin themes.
    pub accent: String,
    pub body_days: i64,
    pub mcp_enabled: bool,
    pub mcp_port: u16,
    pub mcp_token: String,
    /// Senders whose remote images always load.
    pub image_senders: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "zinc-dark".into(),
            accent: "mauve".into(),
            body_days: 90,
            mcp_enabled: true,
            mcp_port: 47823,
            mcp_token: String::new(),
            image_senders: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreEvent {
    Threads,
    Accounts,
    Drafts,
    Settings,
}
