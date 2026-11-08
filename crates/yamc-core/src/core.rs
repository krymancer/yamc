//! Everything the UI and the MCP server can do.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use tokio::runtime::Handle;
use tokio::sync::broadcast;

use crate::mcp::{McpInfo, McpServer};
use crate::net::{imap_connect, is_no_greeting};
use crate::secrets::SecretBox;
use crate::smtp;
use crate::store::{AccountRow, INBOX, Op, SENT, Store};
use crate::sync::{AccountSync, Emit};
use crate::types::*;

/// Sends core events; thread-list changes are coalesced to one per 150ms because sync can
/// touch thousands of rows a second.
#[derive(Clone)]
struct Emitter {
    tx: broadcast::Sender<CoreEvent>,
    pending: Arc<AtomicBool>,
    rt: Handle,
}

impl Emitter {
    fn emit(&self, e: CoreEvent) {
        if e != CoreEvent::Threads {
            let _ = self.tx.send(e);
            return;
        }
        if self.pending.swap(true, Ordering::SeqCst) {
            return;
        }
        let this = self.clone();
        self.rt.spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            this.pending.store(false, Ordering::SeqCst);
            let _ = this.tx.send(CoreEvent::Threads);
        });
    }
}

const MAX_MESSAGE_BYTES: usize = 25 * 1024 * 1024;

pub struct Core {
    pub store: Arc<Store>,
    pub data_dir: PathBuf,
    pub secret_source: &'static str,
    rt: Handle,
    emitter: Emitter,
    syncs: Mutex<HashMap<i64, AccountSync>>,
    mcp: Mutex<Option<McpServer>>,
    mcp_error: Mutex<Option<String>>,
}

pub fn split_addresses(list: &str) -> Vec<String> {
    list.split([',', ';']).map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

/// A file name from a sender can't climb out of the folder or be hidden.
fn safe_filename(name: &str) -> String {
    let clean: String = name.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '\0') || c.is_control() { '_' } else { c }).collect();
    let clean = clean.trim().trim_start_matches('.').trim();
    if clean.is_empty() { "attachment".into() } else { clean.chars().take(200).collect() }
}

/// `dir/name`, or `dir/name (2).ext` and so on if that's taken.
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    (2..).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|p| !p.exists()).unwrap()
}

pub fn default_data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("YAMC_DATA_DIR") {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| {
                let h = PathBuf::from(h);
                if cfg!(target_os = "macos") { h.join("Library/Application Support") } else { h.join(".local/share") }
            })
        })
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("yamc")
}

impl Core {
    /// Open the store. `rt` runs sync, SMTP and the MCP server.
    pub fn open(data_dir: &Path, rt: Handle) -> Result<Arc<Self>> {
        std::fs::create_dir_all(data_dir).with_context(|| format!("create {}", data_dir.display()))?;
        let secrets = SecretBox::open(data_dir)?;
        let secret_source = secrets.source;
        let store = Arc::new(Store::open(&data_dir.join("mail.db"), secrets)?);
        let (tx, _) = broadcast::channel(256);
        Ok(Arc::new(Self {
            store,
            data_dir: data_dir.to_path_buf(),
            secret_source,
            emitter: Emitter { tx, pending: Arc::new(AtomicBool::new(false)), rt: rt.clone() },
            rt,
            syncs: Mutex::default(),
            mcp: Mutex::new(None),
            mcp_error: Mutex::new(None),
        }))
    }

    /// Start syncing every account and the MCP server (if enabled).
    pub fn start(self: &Arc<Self>) {
        for a in self.store.list_accounts() {
            self.start_sync(a);
        }
        self.apply_mcp();
    }

    pub fn stop(&self) {
        for (_, s) in self.syncs.lock().unwrap().drain() {
            s.stop();
        }
        if let Some(m) = self.mcp.lock().unwrap().take() {
            m.stop();
        }
    }

    pub fn runtime(&self) -> &Handle {
        &self.rt
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.emitter.tx.subscribe()
    }

    fn emit(&self, e: CoreEvent) {
        self.emitter.emit(e);
    }

    fn start_sync(&self, a: AccountRow) {
        let emitter = self.emitter.clone();
        let emit: Emit = Arc::new(move |e| emitter.emit(e));
        let store = self.store.clone();
        let days = Box::new(move || store.get_settings().body_days);
        let id = a.id;
        let _guard = self.rt.enter();
        let s = AccountSync::start(self.store.clone(), a, emit, days);
        self.syncs.lock().unwrap().insert(id, s);
    }

    fn kick(&self, account_id: i64) {
        if let Some(s) = self.syncs.lock().unwrap().get(&account_id) {
            s.kick();
        }
    }

    // ---------- accounts ----------

    pub fn list_accounts(&self) -> Vec<Account> {
        let syncs = self.syncs.lock().unwrap();
        self.store
            .list_accounts()
            .into_iter()
            .map(|a| {
                let (state, error) = syncs.get(&a.id).map(|s| s.state()).unwrap_or((SyncState::Idle, None));
                let sender = self.sender_of(&a);
                Account { id: a.id, name: a.name, sender_name: a.sender_name, sender, email: a.email, kind: a.kind, config: a.config, state, error }
            })
            .collect()
    }

    /// Log in to both servers. Returns the config that worked: a server that stays silent on
    /// STARTTLS is retried with TLS from the start (Proton Bridge can be set either way).
    pub async fn test_account(input: &AccountInput) -> Result<AccountConfig> {
        let pw = &input.password;
        let mut cfg = input.config.clone();
        if let Err(e) = Self::test_imap(&cfg, pw).await {
            if !(cfg.imap.tls == TlsMode::Starttls && is_no_greeting(&e)) {
                return Err(anyhow!("IMAP: {e:#}"));
            }
            cfg.imap.tls = TlsMode::Tls;
            // TLS got further than STARTTLS did, so its error is the one worth showing.
            Self::test_imap(&cfg, pw).await.map_err(|e| anyhow!("IMAP: {e:#}"))?;
        }
        if let Err(e) = smtp::verify(&cfg, pw).await {
            if !(cfg.smtp.tls == TlsMode::Starttls && is_no_greeting(&e)) {
                return Err(anyhow!("SMTP: {e:#}"));
            }
            cfg.smtp.tls = TlsMode::Tls;
            smtp::verify(&cfg, pw).await.map_err(|e| anyhow!("SMTP: {e:#}"))?;
        }
        Ok(cfg)
    }

    async fn test_imap(cfg: &AccountConfig, password: &str) -> Result<()> {
        let mut s = imap_connect(cfg, password).await?;
        let _ = s.logout().await;
        Ok(())
    }

    pub async fn add_account(self: &Arc<Self>, mut input: AccountInput) -> Result<i64> {
        input.config = Self::test_account(&input).await?;
        let id = self.store.add_account(&input)?;
        let row = self.store.account(id).ok_or_else(|| anyhow!("account vanished"))?;
        self.start_sync(row);
        self.emit(CoreEvent::Accounts);
        Ok(id)
    }

    /// The From name for an account: set by hand, else what its sent mail already says.
    fn sender_of(&self, a: &AccountRow) -> String {
        a.sender_name.clone().or_else(|| self.store.inferred_sender_name(a.id, &a.email)).unwrap_or_default()
    }

    /// Change the label and the From name. An empty sender name means infer it again.
    pub fn rename_account(&self, id: i64, label: &str, sender_name: &str) -> Result<()> {
        self.store.rename_account(id, label.trim(), sender_name)?;
        self.emit(CoreEvent::Accounts);
        Ok(())
    }

    pub fn remove_account(&self, id: i64) -> Result<()> {
        if let Some(s) = self.syncs.lock().unwrap().remove(&id) {
            s.stop();
        }
        self.store.remove_account(id)?;
        self.emit(CoreEvent::Accounts);
        self.emit(CoreEvent::Threads);
        Ok(())
    }

    // ---------- reading ----------

    pub fn list_mailboxes(&self) -> Vec<Mailbox> {
        self.store.list_mailboxes()
    }

    pub fn list_threads(&self, view: &View, offset: usize, limit: usize) -> Vec<ThreadRow> {
        self.store.list_threads(view, offset, limit)
    }

    pub fn get_thread(&self, account_id: i64, thread_id: &str) -> Thread {
        let messages = self.store.get_thread_messages(account_id, thread_id);
        let subject = messages.last().map(|m| m.subject.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| "(no subject)".into());
        Thread { account_id, thread_id: thread_id.into(), subject, messages }
    }

    /// The message with its body, downloading it first if needed.
    pub async fn load_message(&self, id: i64) -> Option<Message> {
        let m = self.store.get_message(id)?;
        if m.has_body {
            return Some(m);
        }
        let rx = self.syncs.lock().unwrap().get(&m.account_id)?.request_body(id);
        let _ = rx.await;
        self.store.get_message(id)
    }

    /// Download attachment `index` of a message (as listed in `Message::attachments`) into
    /// `dir`, under its own name made unique there. Returns the saved path.
    pub async fn save_attachment(&self, message_id: i64, index: usize, dir: &Path) -> Result<PathBuf> {
        let (name, data) = self.attachment(message_id, index).await?;
        std::fs::create_dir_all(dir)?;
        let path = unique_path(dir, &safe_filename(&name));
        std::fs::write(&path, data).with_context(|| format!("can't write {}", path.display()))?;
        Ok(path)
    }

    /// Download attachment `index` to exactly `path` (a Save As… choice).
    pub async fn save_attachment_as(&self, message_id: i64, index: usize, path: &Path) -> Result<()> {
        let (_, data) = self.attachment(message_id, index).await?;
        std::fs::write(path, data).with_context(|| format!("can't write {}", path.display()))
    }

    async fn attachment(&self, message_id: i64, index: usize) -> Result<(String, Vec<u8>)> {
        let m = self.store.get_message(message_id).ok_or_else(|| anyhow!("message is gone"))?;
        let src = {
            let syncs = self.syncs.lock().unwrap();
            let s = syncs.get(&m.account_id).ok_or_else(|| anyhow!("account isn't connected"))?;
            s.fetch_source(message_id)
        };
        let src = src.await.map_err(|_| anyhow!("yamc stopped syncing this account"))??;
        crate::parse::attachment_data(&src, index).ok_or_else(|| anyhow!("attachment not found in the message"))
    }

    // ---------- actions ----------

    pub fn mark_thread(&self, account_id: i64, thread_id: &str, seen: bool) -> Result<()> {
        let changed: Vec<_> = self.store.thread_copies(account_id, thread_id).into_iter().filter(|c| c.seen != seen).collect();
        if changed.is_empty() {
            return Ok(());
        }
        let mut by_mailbox: HashMap<i64, (String, Vec<u32>)> = HashMap::new();
        for c in changed {
            by_mailbox.entry(c.mailbox_id).or_insert_with(|| (c.path.clone(), Vec::new())).1.push(c.uid);
        }
        self.store.local_change(|tx| {
            for (mailbox_id, (path, uids)) in &by_mailbox {
                for u in uids {
                    tx.execute("UPDATE messages SET seen = ? WHERE mailbox_id = ? AND uid = ?", rusqlite::params![seen, mailbox_id, u])?;
                }
                Store::enqueue_op_tx(tx, account_id, &Op::Seen { mailbox_path: path.clone(), uids: uids.clone(), seen })?;
            }
            Ok(())
        })?;
        self.kick(account_id);
        self.emit(CoreEvent::Threads);
        Ok(())
    }

    /// Move the conversation's Inbox copies to Archive (or All Mail).
    pub fn archive_thread(&self, account_id: i64, thread_id: &str) -> Result<()> {
        let (_, target) = self
            .store
            .find_mailbox(account_id, "\\Archive")
            .or_else(|| self.store.find_mailbox(account_id, "\\All"))
            .ok_or_else(|| anyhow!("This account has no Archive folder"))?;
        let copies: Vec<_> =
            self.store.thread_copies(account_id, thread_id).into_iter().filter(|c| c.special_use.as_deref() == Some(INBOX)).collect();
        self.move_copies(account_id, copies, &target)
    }

    /// Move every copy of the conversation to Trash.
    pub fn trash_thread(&self, account_id: i64, thread_id: &str) -> Result<()> {
        let (_, target) = self.store.find_mailbox(account_id, "\\Trash").ok_or_else(|| anyhow!("This account has no Trash folder"))?;
        let gmail = self.store.account(account_id).is_some_and(|a| a.kind == AccountKind::Gmail);
        let mut copies: Vec<_> = self.store.thread_copies(account_id, thread_id).into_iter().filter(|c| c.path != target).collect();
        if gmail {
            // On Gmail folders are labels: trashing one copy trashes the message, so moving the
            // other labels' copies too would only fail. Prefer the Inbox copy.
            copies.sort_by_key(|c| c.special_use.as_deref() != Some(INBOX));
            let mut seen = std::collections::HashSet::new();
            copies.retain(|c| seen.insert(c.message_key.clone()));
        }
        self.move_copies(account_id, copies, &target)
    }

    fn move_copies(&self, account_id: i64, copies: Vec<crate::store::Copy>, target: &str) -> Result<()> {
        if copies.is_empty() {
            return Ok(());
        }
        let mut by_path: HashMap<String, Vec<u32>> = HashMap::new();
        for c in &copies {
            by_path.entry(c.path.clone()).or_default().push(c.uid);
        }
        self.store.local_change(|tx| {
            for c in &copies {
                tx.execute("DELETE FROM messages WHERE id = ?", [c.id])?;
            }
            for (path, uids) in &by_path {
                Store::enqueue_op_tx(tx, account_id, &Op::Move { mailbox_path: path.clone(), uids: uids.clone(), target: target.to_string() })?;
            }
            Ok(())
        })?;
        self.kick(account_id);
        self.emit(CoreEvent::Threads);
        Ok(())
    }

    // ---------- drafts & sending ----------

    pub fn list_drafts(&self) -> Vec<Draft> {
        self.store.list_drafts()
    }

    pub fn get_draft(&self, id: i64) -> Option<Draft> {
        self.store.get_draft(id)
    }

    pub fn save_draft(&self, d: &DraftInput) -> Result<i64> {
        let id = self.store.save_draft(d)?;
        self.emit(CoreEvent::Drafts);
        Ok(id)
    }

    pub fn delete_draft(&self, id: i64) -> Result<()> {
        self.store.delete_draft(id)?;
        self.emit(CoreEvent::Drafts);
        Ok(())
    }

    /// A reply draft (not saved) for a message.
    pub fn reply_draft(&self, message_id: i64, all: bool) -> Result<DraftInput> {
        let m = self.store.get_message(message_id).ok_or_else(|| anyhow!("no message {message_id}"))?;
        let me = self.store.account(m.account_id).map(|a| a.email.to_lowercase()).unwrap_or_default();
        let from_me = m.from.as_ref().is_some_and(|f| f.address.to_lowercase() == me);
        let to: Vec<Address> = if from_me { m.to.clone() } else { m.from.clone().into_iter().collect() };
        let to_set: std::collections::HashSet<String> = to.iter().map(|a| a.address.to_lowercase()).collect();
        let cc: Vec<Address> = if all {
            m.to.iter()
                .chain(m.cc.iter())
                .filter(|a| {
                    let l = a.address.to_lowercase();
                    l != me && !to_set.contains(&l)
                })
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        let when = chrono::DateTime::from_timestamp_millis(m.date)
            .map(|d| d.with_timezone(&chrono::Local).format("%a, %b %-d, %Y at %H:%M").to_string())
            .unwrap_or_default();
        let quoted = m.text.as_deref().unwrap_or("").lines().map(|l| format!("> {l}")).collect::<Vec<_>>().join("\n");
        let who = m.from.as_ref().map(Address::formatted).unwrap_or_else(|| "someone".into());
        let subject = if m.subject.to_lowercase().starts_with("re:") { m.subject.clone() } else { format!("Re: {}", m.subject) };
        Ok(DraftInput {
            id: None,
            account_id: m.account_id,
            to: to.iter().map(Address::formatted).collect::<Vec<_>>().join(", "),
            cc: cc.iter().map(Address::formatted).collect::<Vec<_>>().join(", "),
            subject,
            body: format!("\n\nOn {when}, {who} wrote:\n{quoted}"),
            reply_to: Some(m.id),
            created_by: None,
            attachments: Vec::new(),
        })
    }

    pub async fn send(&self, d: &DraftInput) -> Result<()> {
        let account = self.store.account(d.account_id).ok_or_else(|| anyhow!("unknown account"))?;
        let to = split_addresses(&d.to);
        if to.is_empty() {
            return Err(anyhow!("No recipients"));
        }
        let cc = split_addresses(&d.cc);
        let parent = d.reply_to.and_then(|id| self.store.get_message(id));
        let mut refs = parent.as_ref().map(|p| p.references.clone()).unwrap_or_default();
        if let Some(id) = parent.as_ref().and_then(|p| p.message_id.clone()) {
            refs.push(id);
        }
        let files = Self::read_attachments(&d.attachments)?;
        let from_name = self.sender_of(&account);
        let msg = smtp::build(&smtp::Outgoing {
            from_name: &from_name,
            from_addr: &account.email,
            to: &to,
            cc: &cc,
            subject: &d.subject,
            body: &d.body,
            in_reply_to: parent.as_ref().and_then(|p| p.message_id.as_deref()),
            references: &refs,
            attachments: &files,
        })?;
        let password = self.store.get_password(account.id)?;
        let raw = msg.formatted();
        smtp::send(&account.config, &password, msg).await?;
        if !account.config.server_saves_sent
            && let Some((_, sent)) = self.store.find_mailbox(account.id, SENT)
        {
            let r: Result<()> = async {
                let mut s = imap_connect(&account.config, &password).await?;
                s.append(&sent, Some("(\\Seen)"), None, &raw).await?;
                let _ = s.logout().await;
                Ok(())
            }
            .await;
            if let Err(e) = r {
                eprintln!("append to Sent failed: {e:#}");
            }
        }
        if let Some(id) = d.id {
            self.delete_draft(id)?;
        }
        // Pasted images were saved by us just for this message.
        let ours = self.pasted_dir();
        for p in &d.attachments {
            if Path::new(p).starts_with(&ours) {
                let _ = std::fs::remove_file(p);
            }
        }
        // Only INBOX is watched live; fetch Sent now so the message shows up there. Servers that
        // file sent mail themselves can take a moment.
        if let Some((sent_id, _)) = self.store.find_mailbox(account.id, SENT)
            && let Some(s) = self.syncs.lock().unwrap().get(&account.id)
        {
            let delay = if account.config.server_saves_sent { 2 } else { 0 };
            s.sync_mailbox_after(sent_id, Duration::from_secs(delay));
        }
        Ok(())
    }

    /// Where pasted images are written until the message is sent.
    fn pasted_dir(&self) -> PathBuf {
        self.data_dir.join("pasted")
    }

    /// Save a pasted image so it can be attached like any file. Returns its path.
    pub fn save_pasted(&self, bytes: &[u8], ext: &str) -> Result<String> {
        let dir = self.pasted_dir();
        std::fs::create_dir_all(&dir)?;
        let stamp = chrono::Local::now().format("%Y-%m-%d at %H.%M.%S");
        let mut path = dir.join(format!("Image {stamp}.{ext}"));
        let mut n = 2;
        while path.exists() {
            path = dir.join(format!("Image {stamp} ({n}).{ext}"));
            n += 1;
        }
        std::fs::write(&path, bytes)?;
        Ok(path.to_string_lossy().into_owned())
    }

    fn read_attachments(paths: &[String]) -> Result<Vec<smtp::OutgoingFile>> {
        let mut total = 0usize;
        let mut out = Vec::new();
        for p in paths {
            let path = Path::new(p);
            let filename = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "attachment".into());
            let data = std::fs::read(path).with_context(|| format!("can't read attachment {filename}"))?;
            total += data.len();
            out.push(smtp::OutgoingFile { content_type: smtp::content_type_for(&filename).into(), filename, data });
        }
        // Base64 grows mail by a third; Gmail and most servers stop at 25 MB.
        if total * 4 / 3 > MAX_MESSAGE_BYTES {
            return Err(anyhow!(
                "Attachments are {:.1} MB; most mail servers reject messages over 25 MB",
                total as f64 / 1_048_576.0
            ));
        }
        Ok(out)
    }

    // ---------- settings ----------

    pub fn get_settings(&self) -> Settings {
        self.store.get_settings()
    }

    /// Merge camelCase keys into the settings.
    pub fn set_settings(self: &Arc<Self>, patch: serde_json::Value) -> Result<Settings> {
        self.store.set_settings(&patch)?;
        let obj = patch.as_object().cloned().unwrap_or_default();
        if obj.keys().any(|k| k.starts_with("mcp")) {
            self.apply_mcp();
        }
        self.emit(CoreEvent::Settings);
        Ok(self.store.get_settings())
    }

    fn apply_mcp(self: &Arc<Self>) {
        let s = self.store.get_settings();
        let mut cur = self.mcp.lock().unwrap();
        if let Some(m) = cur.as_ref()
            && (!s.mcp_enabled || m.port != s.mcp_port || m.token != s.mcp_token)
        {
            cur.take().unwrap().stop();
        }
        if s.mcp_enabled && cur.is_none() {
            match McpServer::start(self.clone(), s.mcp_port, s.mcp_token.clone()) {
                Ok(m) => {
                    *cur = Some(m);
                    *self.mcp_error.lock().unwrap() = None;
                }
                Err(e) => {
                    eprintln!("MCP server: {e:#}");
                    *self.mcp_error.lock().unwrap() = Some(format!("{e:#}"));
                }
            }
        }
    }

    pub fn mcp_info(&self) -> McpInfo {
        let s = self.store.get_settings();
        let running = self.mcp.lock().unwrap().is_some();
        McpInfo::new(s.mcp_enabled, running, self.mcp_error.lock().unwrap().clone(), s.mcp_port, &s.mcp_token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_names_stay_in_the_folder() {
        assert_eq!(safe_filename("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(safe_filename(".hidden"), "hidden");
        assert_eq!(safe_filename("  "), "attachment");
        assert_eq!(safe_filename("Relatório Q3.pdf"), "Relatório Q3.pdf");
        let dir = std::env::temp_dir().join(format!("yamc-unique-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(unique_path(&dir, "a.pdf"), dir.join("a.pdf"));
        std::fs::write(dir.join("a.pdf"), b"").unwrap();
        std::fs::write(dir.join("a (2).pdf"), b"").unwrap();
        assert_eq!(unique_path(&dir, "a.pdf"), dir.join("a (3).pdf"));
        std::fs::write(dir.join("notes"), b"").unwrap();
        assert_eq!(unique_path(&dir, "notes"), dir.join("notes (2)"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
