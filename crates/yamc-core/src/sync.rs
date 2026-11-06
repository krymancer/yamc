//! One worker per account: a command connection that runs queued tasks (urgent first), plus an
//! IDLE connection that watches INBOX and pokes the worker when something changes.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use async_imap::imap_proto::{AttributeValue, Response, Status};
use async_imap::types::{Flag, NameAttribute};
use futures::TryStreamExt;
use tokio::sync::{Notify, oneshot};
use tokio::task::JoinHandle;

use crate::net::{Session, imap_connect};
use crate::parse::{parse_headers, parse_source};
use crate::store::{AccountRow, INBOX, NewMessage, Op, Store, now_ms};
use crate::types::{CoreEvent, SyncState};

const HEADER_BATCH: usize = 250;
const BODY_BATCH: usize = 20;
const BODY_PREFETCH_MAX: u32 = 2 * 1024 * 1024;
const POLL: Duration = Duration::from_secs(5 * 60);
const IDLE_RENEW: Duration = Duration::from_secs(25 * 60);
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const HEADER_FIELDS: &str = "FROM TO CC SUBJECT DATE MESSAGE-ID IN-REPLY-TO REFERENCES";

/// Folders that only duplicate others (Gmail) or that nobody wants indexed.
const SKIP_SPECIAL: [&str; 5] = ["\\Junk", "\\Trash", "\\All", "\\Flagged", "\\Important"];

pub type Emit = Arc<dyn Fn(CoreEvent) + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Task {
    FullSync,
    SyncMailbox(i64),
    FlushOps,
    FetchBody(i64),
    Backfill,
}

#[derive(Default)]
struct Queues {
    urgent: VecDeque<Task>,
    background: VecDeque<Task>,
}

struct Inner {
    store: Arc<Store>,
    account: AccountRow,
    emit: Emit,
    body_days: Box<dyn Fn() -> i64 + Send + Sync>,
    queues: Mutex<Queues>,
    notify: Notify,
    stopped: AtomicBool,
    state: Mutex<(SyncState, Option<String>)>,
    waiters: Mutex<HashMap<i64, Vec<oneshot::Sender<bool>>>>,
    failed_bodies: Mutex<HashSet<i64>>,
}

pub struct AccountSync {
    inner: Arc<Inner>,
    tasks: Vec<JoinHandle<()>>,
}

struct Caps {
    gmail: bool,
    condstore: bool,
    mv: bool,
}

struct Conn {
    session: Session,
    caps: Caps,
}

fn log(account: &AccountRow, msg: impl std::fmt::Display) {
    eprintln!("[{}] {msg}", account.email);
}

/// IMAP modified UTF-7 (RFC 3501 §5.1.3) to a display string.
pub fn decode_mutf7(s: &str) -> String {
    use base64::Engine;
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(end) = after.find('-') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let chunk = &after[..end];
        if chunk.is_empty() {
            out.push('&');
        } else {
            let b64 = chunk.replace(',', "/");
            match base64::engine::general_purpose::STANDARD_NO_PAD.decode(b64.trim_end_matches('=')) {
                Ok(bytes) => {
                    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                    out.push_str(&String::from_utf16_lossy(&units));
                }
                Err(_) => out.push_str(&rest[i..i + 2 + end]),
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Servers without SPECIAL-USE (RFC 6154) still name these folders predictably.
fn special_by_name(path: &str, delimiter: Option<&str>) -> Option<&'static str> {
    if delimiter.is_some_and(|d| path.contains(d)) {
        return None;
    }
    Some(match path.to_lowercase().as_str() {
        "archive" | "archives" => "\\Archive",
        "sent" | "sent items" | "sent messages" | "sent mail" => "\\Sent",
        "drafts" | "draft" => "\\Drafts",
        "trash" | "deleted items" | "deleted messages" | "bin" => "\\Trash",
        "junk" | "spam" | "junk e-mail" | "junk email" | "bulk mail" => "\\Junk",
        _ => return None,
    })
}

fn special_use(attrs: &[NameAttribute<'_>], path: &str, delimiter: Option<&str>) -> Option<String> {
    if path.eq_ignore_ascii_case("INBOX") {
        return Some(INBOX.into());
    }
    special_from_attrs(attrs).or_else(|| special_by_name(path, delimiter).map(String::from))
}

fn special_from_attrs(attrs: &[NameAttribute<'_>]) -> Option<String> {
    attrs.iter().find_map(|a| {
        Some(
            match a {
                NameAttribute::All => "\\All",
                NameAttribute::Archive => "\\Archive",
                NameAttribute::Drafts => "\\Drafts",
                NameAttribute::Flagged => "\\Flagged",
                NameAttribute::Junk => "\\Junk",
                NameAttribute::Sent => "\\Sent",
                NameAttribute::Trash => "\\Trash",
                NameAttribute::Extension(e) if e.eq_ignore_ascii_case("\\Important") => "\\Important",
                _ => return None,
            }
            .to_string(),
        )
    })
}

fn is_connection_error(e: &anyhow::Error) -> bool {
    use async_imap::error::Error as E;
    e.chain().any(|c| {
        c.downcast_ref::<E>().is_some_and(|e| matches!(e, E::Io(_) | E::ConnectionLost))
            || c.downcast_ref::<std::io::Error>().is_some()
    })
}

fn uid_set(uids: &[u32]) -> String {
    uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",")
}

impl AccountSync {
    pub fn start(store: Arc<Store>, account: AccountRow, emit: Emit, body_days: Box<dyn Fn() -> i64 + Send + Sync>) -> Self {
        let inner = Arc::new(Inner {
            store,
            account,
            emit,
            body_days,
            queues: Mutex::default(),
            notify: Notify::new(),
            stopped: AtomicBool::new(false),
            state: Mutex::new((SyncState::Idle, None)),
            waiters: Mutex::default(),
            failed_bodies: Mutex::default(),
        });
        inner.push(false, Task::FullSync);
        let worker = tokio::spawn(inner.clone().worker());
        let idle = tokio::spawn(inner.clone().idle_watch());
        Self { inner, tasks: vec![worker, idle] }
    }

    pub fn stop(&self) {
        self.inner.stopped.store(true, Ordering::SeqCst);
        self.inner.notify.notify_one();
        for t in &self.tasks {
            t.abort();
        }
    }

    pub fn state(&self) -> (SyncState, Option<String>) {
        self.inner.state.lock().unwrap().clone()
    }

    /// Push local changes (mark read, archive) to the server soon.
    pub fn kick(&self) {
        self.inner.push(true, Task::FlushOps);
    }

    /// Fetch one body now. The receiver gets true once it's in the database.
    pub fn request_body(&self, message_id: i64) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        let first = {
            let mut w = self.inner.waiters.lock().unwrap();
            let list = w.entry(message_id).or_default();
            list.push(tx);
            list.len() == 1
        };
        if first {
            self.inner.push(true, Task::FetchBody(message_id));
        }
        rx
    }
}

impl Drop for AccountSync {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Inner {
    fn push(&self, urgent: bool, task: Task) {
        if self.stopped.load(Ordering::SeqCst) {
            return;
        }
        let mut q = self.queues.lock().unwrap();
        let lane = if urgent { &mut q.urgent } else { &mut q.background };
        if !lane.contains(&task) {
            lane.push_back(task);
        }
        drop(q);
        self.notify.notify_one();
    }

    fn pop(&self) -> Option<Task> {
        let mut q = self.queues.lock().unwrap();
        q.urgent.pop_front().or_else(|| q.background.pop_front())
    }

    fn set_state(&self, state: SyncState, error: Option<String>) {
        *self.state.lock().unwrap() = (state, error);
        (self.emit)(CoreEvent::Accounts);
    }

    fn changed(&self) {
        (self.emit)(CoreEvent::Threads);
    }

    fn resolve(&self, id: i64, ok: bool) {
        if let Some(list) = self.waiters.lock().unwrap().remove(&id) {
            for tx in list {
                let _ = tx.send(ok);
            }
        }
    }

    fn fail_all_waiters(&self) {
        let ids: Vec<i64> = self.waiters.lock().unwrap().keys().copied().collect();
        for id in ids {
            self.resolve(id, false);
        }
    }

    async fn connect(&self) -> Result<Conn> {
        let password = self.store.get_password(self.account.id)?;
        let mut session = imap_connect(&self.account.config, &password).await?;
        let caps = session.capabilities().await?;
        let caps = Caps { gmail: caps.has_str("X-GM-EXT-1"), condstore: caps.has_str("CONDSTORE"), mv: caps.has_str("MOVE") };
        Ok(Conn { session, caps })
    }

    async fn worker(self: Arc<Self>) {
        let mut conn: Option<Conn> = None;
        let mut retry = Duration::from_secs(5);
        let mut retry_at: Option<Instant> = None;
        let mut poll_at = Instant::now() + POLL;
        while !self.stopped.load(Ordering::SeqCst) {
            let Some(task) = self.pop() else {
                let now = Instant::now();
                let until = retry_at.unwrap_or(poll_at).max(now);
                tokio::select! {
                    _ = self.notify.notified() => {}
                    _ = tokio::time::sleep_until(until.into()) => {
                        retry_at = None;
                        poll_at = Instant::now() + POLL;
                        self.push(false, Task::FullSync);
                    }
                }
                continue;
            };
            let reused = conn.is_some();
            let mut result = async {
                if conn.is_none() {
                    conn = Some(self.connect().await?);
                }
                self.run(conn.as_mut().unwrap(), task.clone()).await
            }
            .await;
            // Servers drop idle connections; a stale one gets one immediate retry on a fresh one.
            if reused && result.as_ref().is_err_and(is_connection_error) {
                conn = None;
                result = async {
                    conn = Some(self.connect().await?);
                    self.run(conn.as_mut().unwrap(), task.clone()).await
                }
                .await;
            }
            match result {
                Ok(()) => {
                    retry = Duration::from_secs(5);
                    if let Some(c) = conn.as_mut() {
                        while c.session.unsolicited_responses.try_recv().is_ok() {}
                    }
                }
                Err(e) => {
                    log(&self.account, format!("{task:?} failed: {e:#}"));
                    conn = None;
                    self.set_state(SyncState::Error, Some(format!("{e:#}")));
                    self.fail_all_waiters();
                    *self.queues.lock().unwrap() = Queues::default();
                    retry_at = Some(Instant::now() + retry);
                    retry = (retry * 2).min(Duration::from_secs(300));
                }
            }
        }
        if let Some(mut c) = conn {
            let _ = c.session.logout().await;
        }
    }

    async fn run(&self, c: &mut Conn, task: Task) -> Result<()> {
        match task {
            Task::FullSync => self.full_sync(c).await,
            Task::SyncMailbox(id) => self.sync_mailbox(c, id).await,
            Task::FlushOps => self.flush_ops(c).await,
            Task::Backfill => self.backfill(c).await,
            Task::FetchBody(id) => {
                let ok = match self.store.message_location(id) {
                    Some(loc) if loc.has_body => true,
                    Some(loc) => self.fetch_bodies(c, &loc.path, &[(loc.id, loc.uid)]).await? > 0,
                    None => false,
                };
                self.resolve(id, ok);
                Ok(())
            }
        }
    }

    async fn full_sync(&self, c: &mut Conn) -> Result<()> {
        self.set_state(SyncState::Syncing, None);
        self.flush_ops(c).await?;
        self.sync_mailbox_list(c).await?;
        let mut rows: Vec<_> = self.store.mailbox_rows(self.account.id).into_iter().filter(|m| m.synced).collect();
        rows.sort_by_key(|m| m.special_use.as_deref() != Some(INBOX));
        for r in rows {
            if self.stopped.load(Ordering::SeqCst) {
                return Ok(());
            }
            // Opening a message mid-sync shouldn't wait for the whole account.
            while let Some(t) = self.take_urgent() {
                Box::pin(self.run(c, t)).await?;
            }
            self.sync_mailbox(c, r.id).await?;
        }
        self.set_state(SyncState::Idle, None);
        self.push(false, Task::Backfill);
        Ok(())
    }

    fn take_urgent(&self) -> Option<Task> {
        self.queues.lock().unwrap().urgent.pop_front()
    }

    async fn sync_mailbox_list(&self, c: &mut Conn) -> Result<()> {
        let names: Vec<_> = c.session.list(Some(""), Some("*")).await?.try_collect().await?;
        let mut kept = HashSet::new();
        for n in &names {
            let attrs = n.attributes();
            let skip = attrs.iter().any(|a| match a {
                NameAttribute::NoSelect => true,
                NameAttribute::Extension(e) => e.eq_ignore_ascii_case("\\NonExistent"),
                _ => false,
            });
            if skip {
                continue;
            }
            let path = n.name();
            let delim = n.delimiter();
            let special = special_use(attrs, path, delim);
            let depth = delim.map(|d| path.matches(d).count() as i64).unwrap_or(0);
            let leaf = delim.and_then(|d| path.rsplit(d).next()).unwrap_or(path);
            let name = if special.as_deref() == Some(INBOX) { "Inbox".to_string() } else { decode_mutf7(leaf) };
            let synced = special.as_deref().is_none_or(|s| !SKIP_SPECIAL.contains(&s));
            self.store.upsert_mailbox(self.account.id, path, &name, delim, special.as_deref(), depth, synced)?;
            kept.insert(path.to_string());
        }
        self.store.remove_mailboxes_except(self.account.id, &kept)?;
        self.changed();
        Ok(())
    }

    async fn sync_mailbox(&self, c: &mut Conn, mailbox_id: i64) -> Result<()> {
        let Some(row) = self.store.mailbox_rows(self.account.id).into_iter().find(|m| m.id == mailbox_id) else { return Ok(()) };
        let mb = if c.caps.condstore { c.session.select_condstore(&row.path).await? } else { c.session.select(&row.path).await? };
        // A missing UIDVALIDITY is "unknown", never "changed": only a real change wipes local copies.
        let uidvalidity = mb.uid_validity.map(|v| v.to_string()).or(row.uidvalidity.clone());
        if let (Some(old), Some(new)) = (row.uidvalidity.as_deref(), mb.uid_validity)
            && old != new.to_string()
        {
            self.store.clear_mailbox(mailbox_id)?;
        }

        let known = self.store.known_uids(mailbox_id);
        let on_server: HashSet<u32> = if mb.exists > 0 { c.session.uid_search("ALL").await? } else { HashSet::new() };
        let removed: Vec<i64> = known.iter().filter(|(uid, _)| !on_server.contains(uid)).map(|(_, (id, _))| *id).collect();
        if !removed.is_empty() {
            self.store.delete_messages(&removed)?;
            self.changed();
        }

        // Newest first, so the top of the inbox fills in before old history.
        let mut missing: Vec<u32> = on_server.iter().copied().filter(|u| !known.contains_key(u)).collect();
        missing.sort_unstable_by(|a, b| b.cmp(a));
        for batch in missing.chunks(HEADER_BATCH) {
            if self.stopped.load(Ordering::SeqCst) {
                return Ok(());
            }
            let msgs = self.fetch_headers(c, mailbox_id, batch).await?;
            self.store.insert_messages(&msgs)?;
            self.changed();
        }
        if !missing.is_empty() {
            self.push(false, Task::Backfill);
        }

        // Flag changes on messages we already had.
        if !known.is_empty() {
            let prev: Option<u64> = row.modseq.as_deref().and_then(|m| m.parse().ok());
            let can_diff = c.caps.condstore && prev.is_some() && mb.highest_modseq.is_some();
            if !(can_diff && mb.highest_modseq == prev) {
                let query = match (can_diff, prev) {
                    (true, Some(p)) => format!("(UID FLAGS) (CHANGEDSINCE {p})"),
                    _ => "(UID FLAGS)".to_string(),
                };
                let fetched: Vec<_> = c.session.uid_fetch("1:*", &query).await?.try_collect().await?;
                let (mut seen, mut unseen) = (Vec::new(), Vec::new());
                for f in &fetched {
                    let Some(uid) = f.uid else { continue };
                    let Some((_, was)) = known.get(&uid) else { continue };
                    let now = f.flags().any(|fl| matches!(fl, Flag::Seen));
                    if now != *was {
                        if now { seen.push(uid) } else { unseen.push(uid) }
                    }
                }
                if !seen.is_empty() {
                    self.store.set_seen_by_uid(mailbox_id, &seen, true)?;
                }
                if !unseen.is_empty() {
                    self.store.set_seen_by_uid(mailbox_id, &unseen, false)?;
                }
                if !seen.is_empty() || !unseen.is_empty() {
                    self.changed();
                }
            }
        }
        if let Some(v) = uidvalidity {
            self.store.set_mailbox_state(mailbox_id, &v, mb.highest_modseq)?;
        }
        Ok(())
    }

    /// Raw UID FETCH so Gmail's X-GM-THRID comes through (async-imap's Fetch doesn't expose it).
    async fn fetch_headers(&self, c: &mut Conn, mailbox_id: i64, uids: &[u32]) -> Result<Vec<NewMessage>> {
        let cmd = format!(
            "UID FETCH {} (UID FLAGS RFC822.SIZE INTERNALDATE{} BODY.PEEK[HEADER.FIELDS ({HEADER_FIELDS})])",
            uid_set(uids),
            if c.caps.gmail { " X-GM-THRID" } else { "" }
        );
        let tag = c.session.run_command(&cmd).await?;
        let mut out = Vec::new();
        loop {
            let resp = c.session.read_response().await?.ok_or_else(|| anyhow!("connection lost"))?;
            match resp.parsed() {
                Response::Fetch(_, attrs) => {
                    let mut m = NewMessage { account_id: self.account.id, mailbox_id, ..Default::default() };
                    let mut header: Option<&[u8]> = None;
                    let mut internal: Option<i64> = None;
                    for a in attrs {
                        match a {
                            AttributeValue::Uid(u) => m.uid = *u,
                            AttributeValue::Flags(f) => m.seen = f.iter().any(|x| x.eq_ignore_ascii_case("\\Seen")),
                            AttributeValue::Rfc822Size(s) => m.size = *s,
                            AttributeValue::GmailThrId(t) => m.gmail_thread_id = Some(*t),
                            AttributeValue::InternalDate(d) => {
                                internal = chrono::DateTime::parse_from_str(d, "%d-%b-%Y %H:%M:%S %z").ok().map(|d| d.timestamp_millis())
                            }
                            AttributeValue::BodySection { data: Some(d), .. } => header = Some(d.as_ref()),
                            _ => {}
                        }
                    }
                    if m.uid == 0 {
                        continue;
                    }
                    let h = parse_headers(header.unwrap_or(b""));
                    m.message_id = h.message_id;
                    m.in_reply_to = h.in_reply_to;
                    m.references = h.references;
                    m.subject = h.subject;
                    m.from = h.from;
                    m.to = h.to;
                    m.cc = h.cc;
                    m.date = h.date.or(internal).unwrap_or_else(now_ms);
                    out.push(m);
                }
                Response::Done { tag: t, status, outcome } if *t == tag => {
                    if *status != Status::Ok {
                        bail!("FETCH failed: {}", outcome.information.as_deref().unwrap_or("?"));
                    }
                    break;
                }
                _ => {}
            }
        }
        Ok(out)
    }

    /// Download bodies for recent mail in small chunks; urgent work gets a turn between chunks.
    async fn backfill(&self, c: &mut Conn) -> Result<()> {
        let since = now_ms() - (self.body_days)() * DAY_MS;
        let failed = self.failed_bodies.lock().unwrap().clone();
        let todo: Vec<_> = self
            .store
            .bodyless(self.account.id, since, BODY_PREFETCH_MAX, BODY_BATCH * 4)
            .into_iter()
            .filter(|m| !failed.contains(&m.id))
            .take(BODY_BATCH)
            .collect();
        if todo.is_empty() {
            return Ok(());
        }
        let mut by_path: HashMap<String, Vec<(i64, u32)>> = HashMap::new();
        for m in todo {
            by_path.entry(m.path).or_default().push((m.id, m.uid));
        }
        for (path, items) in by_path {
            self.fetch_bodies(c, &path, &items).await?;
        }
        self.push(false, Task::Backfill);
        Ok(())
    }

    async fn fetch_bodies(&self, c: &mut Conn, path: &str, items: &[(i64, u32)]) -> Result<usize> {
        c.session.select(path).await?;
        let mut by_uid: HashMap<u32, i64> = items.iter().map(|(id, uid)| (*uid, *id)).collect();
        let uids: Vec<u32> = items.iter().map(|(_, u)| *u).collect();
        let fetched: Vec<_> = c.session.uid_fetch(uid_set(&uids), "(UID BODY.PEEK[])").await?.try_collect().await?;
        let mut done = 0;
        for f in &fetched {
            let (Some(uid), Some(src)) = (f.uid, f.body()) else { continue };
            let Some(&id) = by_uid.get(&uid) else { continue };
            match parse_source(src) {
                Some(body) => {
                    self.store.save_body(id, &body)?;
                    by_uid.remove(&uid);
                    done += 1;
                }
                None => log(&self.account, format!("parse failed for message {id}")),
            }
        }
        self.failed_bodies.lock().unwrap().extend(by_uid.values());
        if done > 0 {
            self.changed();
        }
        Ok(done)
    }

    async fn flush_ops(&self, c: &mut Conn) -> Result<()> {
        let mut touched = HashSet::new();
        for (id, op, attempts) in self.store.pending_ops(self.account.id) {
            let r: Result<()> = async {
                match &op {
                    Op::Seen { mailbox_path, uids, seen } => {
                        c.session.select(mailbox_path).await?;
                        let q = if *seen { "+FLAGS.SILENT (\\Seen)" } else { "-FLAGS.SILENT (\\Seen)" };
                        let _: Vec<_> = c.session.uid_store(uid_set(uids), q).await?.try_collect().await?;
                    }
                    Op::Move { mailbox_path, uids, target } => {
                        c.session.select(mailbox_path).await?;
                        let set = uid_set(uids);
                        if c.caps.mv {
                            c.session.uid_mv(&set, target).await?;
                        } else {
                            c.session.uid_copy(&set, target).await?;
                            let _: Vec<_> = c.session.uid_store(&set, "+FLAGS.SILENT (\\Deleted)").await?.try_collect().await?;
                            let _: Vec<_> = c.session.expunge().await?.try_collect().await?;
                        }
                        touched.insert(target.clone());
                    }
                }
                Ok(())
            }
            .await;
            match r {
                Ok(()) => self.store.finish_op(id)?,
                Err(e) => {
                    log(&self.account, format!("op {op:?} failed: {e:#}"));
                    if attempts >= 5 { self.store.finish_op(id)? } else { self.store.fail_op(id)? }
                    // A broken connection fails every op; let the worker reconnect.
                    if is_connection_error(&e) {
                        return Err(e);
                    }
                }
            }
        }
        // Moved mail shows up in its new folder now, not at the next poll.
        for row in self.store.mailbox_rows(self.account.id) {
            if row.synced && touched.contains(&row.path) {
                self.push(true, Task::SyncMailbox(row.id));
            }
        }
        Ok(())
    }

    /// A second connection parked in IDLE on INBOX.
    async fn idle_watch(self: Arc<Self>) {
        let mut wait = Duration::from_secs(10);
        while !self.stopped.load(Ordering::SeqCst) {
            match self.idle_once().await {
                Ok(()) => return,
                Err(e) => {
                    log(&self.account, format!("idle: {e:#}"));
                    tokio::time::sleep(wait).await;
                    wait = (wait * 2).min(Duration::from_secs(300));
                }
            }
        }
    }

    async fn idle_once(&self) -> Result<()> {
        let mut c = self.connect().await?;
        if !c.session.capabilities().await?.has_str("IDLE") {
            return Ok(()); // The five-minute poll covers it.
        }
        c.session.select("INBOX").await?;
        let mut session = c.session;
        loop {
            let mut idle = session.idle();
            idle.init().await?;
            let (wait, _stop) = idle.wait_with_timeout(IDLE_RENEW);
            let resp = wait.await?;
            session = idle.done().await?;
            if matches!(resp, async_imap::extensions::idle::IdleResponse::NewData(_)) {
                // Wait for the burst (EXISTS + FETCH + …) to settle.
                tokio::time::sleep(Duration::from_millis(300)).await;
                if let Some(inbox) = self.store.find_mailbox(self.account.id, INBOX) {
                    self.push(true, Task::SyncMailbox(inbox.0));
                }
            }
            if self.stopped.load(Ordering::SeqCst) {
                let _ = session.logout().await;
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::decode_mutf7;

    #[test]
    fn mutf7() {
        assert_eq!(decode_mutf7("Inbox"), "Inbox");
        assert_eq!(decode_mutf7("[Gmail]/Lixeira"), "[Gmail]/Lixeira");
        assert_eq!(decode_mutf7("Entw&APw-rfe"), "Entwürfe");
        assert_eq!(decode_mutf7("&ZeVnLIqe-"), "日本語");
        assert_eq!(decode_mutf7("A&-B"), "A&B");
    }
}
