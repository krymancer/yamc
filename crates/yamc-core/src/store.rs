//! SQLite store: accounts, mailboxes, message headers, bodies, FTS, drafts and the ops outbox.
//!
//! Two connections: sync writes on one while the UI reads on the other (WAL lets them overlap).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use anyhow::{Context, Result, anyhow};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use crate::secrets::SecretBox;
use crate::types::*;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS accounts (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  email TEXT NOT NULL,
  kind TEXT NOT NULL,
  color TEXT NOT NULL DEFAULT '',
  sender_name TEXT,
  config TEXT NOT NULL,
  secret BLOB NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS mailboxes (
  id INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  name TEXT NOT NULL,
  delimiter TEXT,
  special_use TEXT,
  depth INTEGER NOT NULL DEFAULT 0,
  synced INTEGER NOT NULL DEFAULT 1,
  uidvalidity TEXT,
  modseq TEXT,
  UNIQUE(account_id, path)
);
CREATE TABLE IF NOT EXISTS messages (
  id INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  mailbox_id INTEGER NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
  uid INTEGER NOT NULL,
  message_id TEXT,
  thread_id TEXT NOT NULL,
  in_reply_to TEXT,
  refs TEXT,
  subject TEXT NOT NULL DEFAULT '',
  from_name TEXT,
  from_addr TEXT,
  to_json TEXT NOT NULL DEFAULT '[]',
  cc_json TEXT NOT NULL DEFAULT '[]',
  date INTEGER NOT NULL,
  seen INTEGER NOT NULL DEFAULT 0,
  snippet TEXT NOT NULL DEFAULT '',
  has_body INTEGER NOT NULL DEFAULT 0,
  size INTEGER NOT NULL DEFAULT 0,
  UNIQUE(mailbox_id, uid)
);
CREATE INDEX IF NOT EXISTS messages_mailbox_thread ON messages(mailbox_id, account_id, thread_id, date, seen);
CREATE INDEX IF NOT EXISTS messages_thread ON messages(account_id, thread_id, date);
CREATE INDEX IF NOT EXISTS messages_msgid ON messages(account_id, message_id);
CREATE INDEX IF NOT EXISTS messages_reply ON messages(account_id, in_reply_to);
CREATE INDEX IF NOT EXISTS messages_bodyless ON messages(account_id, has_body, date);
CREATE TABLE IF NOT EXISTS bodies (
  message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
  html TEXT,
  text TEXT,
  attachments TEXT NOT NULL DEFAULT '[]'
);
CREATE VIRTUAL TABLE IF NOT EXISTS fts USING fts5(
  subject, sender, recipients, body,
  content='', contentless_delete=1,
  tokenize='unicode61 remove_diacritics 2'
);
CREATE TRIGGER IF NOT EXISTS messages_fts_delete AFTER DELETE ON messages BEGIN
  DELETE FROM fts WHERE rowid = old.id;
END;
CREATE TABLE IF NOT EXISTS drafts (
  id INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  to_list TEXT NOT NULL DEFAULT '',
  cc_list TEXT NOT NULL DEFAULT '',
  subject TEXT NOT NULL DEFAULT '',
  body TEXT NOT NULL DEFAULT '',
  reply_to INTEGER,
  created_by TEXT NOT NULL DEFAULT 'user',
  updated_at INTEGER NOT NULL,
  attachments TEXT NOT NULL DEFAULT '[]'
);
CREATE TABLE IF NOT EXISTS ops (
  id INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  payload TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
-- One row per conversation per folder, kept current by triggers, so listing a folder is an
-- index range scan instead of a GROUP BY over every message.
CREATE TABLE IF NOT EXISTS mailbox_threads (
  mailbox_id INTEGER NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
  account_id INTEGER NOT NULL,
  thread_id TEXT NOT NULL,
  last_date INTEGER NOT NULL,
  all_seen INTEGER NOT NULL,
  PRIMARY KEY (mailbox_id, thread_id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS mailbox_threads_order ON mailbox_threads(mailbox_id, all_seen, last_date DESC);
CREATE TRIGGER IF NOT EXISTS mt_insert AFTER INSERT ON messages BEGIN
  INSERT INTO mailbox_threads(mailbox_id, account_id, thread_id, last_date, all_seen)
  VALUES (NEW.mailbox_id, NEW.account_id, NEW.thread_id, NEW.date, NEW.seen)
  ON CONFLICT(mailbox_id, thread_id) DO UPDATE
    SET last_date = MAX(last_date, excluded.last_date), all_seen = MIN(all_seen, excluded.all_seen);
END;
CREATE TRIGGER IF NOT EXISTS mt_update AFTER UPDATE OF seen, date, thread_id ON messages BEGIN
  DELETE FROM mailbox_threads WHERE mailbox_id = OLD.mailbox_id AND thread_id = OLD.thread_id;
  INSERT INTO mailbox_threads(mailbox_id, account_id, thread_id, last_date, all_seen)
    SELECT mailbox_id, account_id, thread_id, MAX(date), MIN(seen) FROM messages
    WHERE mailbox_id = OLD.mailbox_id AND account_id = OLD.account_id AND thread_id = OLD.thread_id
    GROUP BY mailbox_id, account_id, thread_id;
  INSERT OR REPLACE INTO mailbox_threads(mailbox_id, account_id, thread_id, last_date, all_seen)
    SELECT mailbox_id, account_id, thread_id, MAX(date), MIN(seen) FROM messages
    WHERE mailbox_id = NEW.mailbox_id AND account_id = NEW.account_id AND thread_id = NEW.thread_id
      AND (NEW.thread_id != OLD.thread_id OR NEW.mailbox_id != OLD.mailbox_id)
    GROUP BY mailbox_id, account_id, thread_id;
END;
CREATE TRIGGER IF NOT EXISTS mt_delete AFTER DELETE ON messages BEGIN
  DELETE FROM mailbox_threads WHERE mailbox_id = OLD.mailbox_id AND thread_id = OLD.thread_id;
  INSERT INTO mailbox_threads(mailbox_id, account_id, thread_id, last_date, all_seen)
    SELECT mailbox_id, account_id, thread_id, MAX(date), MIN(seen) FROM messages
    WHERE mailbox_id = OLD.mailbox_id AND account_id = OLD.account_id AND thread_id = OLD.thread_id
    GROUP BY mailbox_id, account_id, thread_id;
END;
"#;

/// Fill the summary table for databases created before it existed.
const BACKFILL_SUMMARY: &str = "
INSERT INTO mailbox_threads(mailbox_id, account_id, thread_id, last_date, all_seen)
  SELECT mailbox_id, account_id, thread_id, MAX(date), MIN(seen) FROM messages GROUP BY mailbox_id, account_id, thread_id;";

pub const INBOX: &str = "\\Inbox";
pub const SENT: &str = "\\Sent";
const PAGE_MAX: usize = 1000;

/// A message header as it comes off the wire, before it gets a thread.
#[derive(Clone, Debug, Default)]
pub struct NewMessage {
    pub account_id: i64,
    pub mailbox_id: i64,
    pub uid: u32,
    pub message_id: Option<String>,
    pub gmail_thread_id: Option<u64>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub subject: String,
    pub from: Option<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub date: i64,
    pub seen: bool,
    pub size: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Body {
    pub html: Option<String>,
    pub text: Option<String>,
    pub attachments: Vec<Attachment>,
    pub snippet: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Op {
    Seen { #[serde(rename = "mailboxPath")] mailbox_path: String, uids: Vec<u32>, seen: bool },
    Move { #[serde(rename = "mailboxPath")] mailbox_path: String, uids: Vec<u32>, target: String },
}

impl Op {
    fn kind(&self) -> &'static str {
        match self {
            Op::Seen { .. } => "seen",
            Op::Move { .. } => "move",
        }
    }
}

#[derive(Clone, Debug)]
pub struct MailboxRow {
    pub id: i64,
    pub path: String,
    pub special_use: Option<String>,
    pub synced: bool,
    pub uidvalidity: Option<String>,
    pub modseq: Option<String>,
}

#[derive(Clone, Debug)]
pub struct MessageLoc {
    pub id: i64,
    pub account_id: i64,
    pub mailbox_id: i64,
    pub path: String,
    pub uid: u32,
    pub has_body: bool,
}

#[derive(Clone, Debug)]
pub struct Copy {
    pub id: i64,
    pub mailbox_id: i64,
    pub path: String,
    pub special_use: Option<String>,
    pub uid: u32,
    pub seen: bool,
    /// Message-ID, or the row id when there is none: the same mail in several folders shares it.
    pub message_key: String,
}

#[derive(Clone, Debug)]
pub struct AccountRow {
    pub id: i64,
    pub name: String,
    pub email: String,
    pub kind: AccountKind,
    pub config: AccountConfig,
    /// The From name set by hand; `None` means infer it from mail already sent.
    pub sender_name: Option<String>,
}

fn non_empty(s: &str) -> Option<&str> {
    Some(s.trim()).filter(|s| !s.is_empty())
}

fn addr_text(list: &[Address]) -> String {
    list.iter()
        .map(|a| format!("{} {}", a.name.as_deref().unwrap_or(""), a.address))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Turn free text into a safe FTS5 query: every word becomes a quoted prefix term, and
/// `from:` / `to:` / `cc:` / `subject:` / `body:` scope a word to that field.
pub fn fts_query(input: &str) -> String {
    input
        .split_whitespace()
        .filter_map(|t| {
            let (col, word) = match t.split_once(':') {
                Some((k, w)) => match k.to_lowercase().as_str() {
                    "from" => (Some("sender"), w),
                    "to" | "cc" => (Some("recipients"), w),
                    "subject" | "s" => (Some("subject"), w),
                    "body" => (Some("body"), w),
                    _ => (None, t),
                },
                None => (None, t),
            };
            let word = word.replace('"', "");
            if word.is_empty() {
                return None;
            }
            Some(match col {
                Some(c) => format!("{c}:\"{word}\"*"),
                None => format!("\"{word}\"*"),
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn json<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".into())
}

fn parse_json<T: for<'de> Deserialize<'de> + Default>(s: Option<String>) -> T {
    s.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn open(path: &Path) -> Result<Connection> {
    let c = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
    c.execute_batch(
        "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;
         PRAGMA cache_size = -65536; PRAGMA temp_store = MEMORY; PRAGMA mmap_size = 268435456;
         PRAGMA busy_timeout = 5000;",
    )?;
    Ok(c)
}

pub struct Store {
    w: Mutex<Connection>,
    r: Mutex<Connection>,
    secrets: SecretBox,
}

impl Store {
    pub fn open(path: &Path, secrets: SecretBox) -> Result<Self> {
        let w = open(path)?;
        w.execute_batch(SCHEMA)?;
        // Columns added after the first release.
        let has_attachments: bool =
            w.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('drafts') WHERE name = 'attachments')", [], |r| r.get(0))?;
        if !has_attachments {
            w.execute_batch("ALTER TABLE drafts ADD COLUMN attachments TEXT NOT NULL DEFAULT '[]'")?;
        }
        let has_sender: bool =
            w.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('accounts') WHERE name = 'sender_name')", [], |r| r.get(0))?;
        if !has_sender {
            w.execute_batch("ALTER TABLE accounts ADD COLUMN sender_name TEXT")?;
        }
        let summary_empty: bool = w.query_row("SELECT NOT EXISTS(SELECT 1 FROM mailbox_threads)", [], |r| r.get(0))?;
        let has_messages: bool = w.query_row("SELECT EXISTS(SELECT 1 FROM messages)", [], |r| r.get(0))?;
        if summary_empty && has_messages {
            w.execute_batch(BACKFILL_SUMMARY)?;
        }
        let r = open(path)?;
        Ok(Self { w: Mutex::new(w), r: Mutex::new(r), secrets })
    }

    fn w(&self) -> MutexGuard<'_, Connection> {
        self.w.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn r(&self) -> MutexGuard<'_, Connection> {
        self.r.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---------- settings ----------

    pub fn get_settings(&self) -> Settings {
        let mut map = match serde_json::to_value(Settings::default()) {
            Ok(serde_json::Value::Object(m)) => m,
            _ => Default::default(),
        };
        {
            let c = self.r();
            let mut st = c.prepare_cached("SELECT key, value FROM settings").expect("settings");
            let rows = st
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .expect("settings");
            for (k, v) in rows.flatten() {
                if let Ok(v) = serde_json::from_str(&v) {
                    map.insert(k, v);
                }
            }
        }
        let mut s: Settings = serde_json::from_value(serde_json::Value::Object(map)).unwrap_or_default();
        if s.mcp_token.is_empty() {
            s.mcp_token = crate::secrets::random_token();
            let _ = self.set_settings(&serde_json::json!({ "mcpToken": s.mcp_token }));
        }
        s
    }

    /// Merge a JSON object of camelCase settings keys.
    pub fn set_settings(&self, patch: &serde_json::Value) -> Result<()> {
        let obj = patch.as_object().ok_or_else(|| anyhow!("settings patch must be an object"))?;
        let mut c = self.w();
        let tx = c.transaction()?;
        for (k, v) in obj {
            tx.execute(
                "INSERT INTO settings(key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![k, v.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    // ---------- accounts ----------

    pub fn add_account(&self, a: &AccountInput) -> Result<i64> {
        let secret = self.secrets.encrypt(&a.password)?;
        let c = self.w();
        c.execute(
            "INSERT INTO accounts(name, email, kind, config, secret, created_at, sender_name) VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![a.name, a.email, a.kind.as_str(), json(&a.config), secret, now_ms(), non_empty(&a.sender_name)],
        )?;
        Ok(c.last_insert_rowid())
    }

    /// Set the label and the From name; an empty sender name goes back to inferring it.
    pub fn rename_account(&self, id: i64, label: &str, sender_name: &str) -> Result<()> {
        self.w().execute(
            "UPDATE accounts SET name = ?, sender_name = ? WHERE id = ?",
            params![label, non_empty(sender_name), id],
        )?;
        Ok(())
    }

    /// The name this address usually sends under, taken from synced mail (mostly the Sent
    /// folder). The most frequent one wins, so one odd message doesn't change it.
    pub fn inferred_sender_name(&self, account_id: i64, email: &str) -> Option<String> {
        self.r()
            .query_row(
                "SELECT from_name FROM (
                   SELECT from_name, date FROM messages
                   WHERE account_id = ? AND from_addr = ? COLLATE NOCASE AND from_name IS NOT NULL AND from_name != ''
                   ORDER BY date DESC LIMIT 200)
                 GROUP BY from_name ORDER BY COUNT(*) DESC, MAX(date) DESC LIMIT 1",
                params![account_id, email],
                |r| r.get(0),
            )
            .ok()
    }

    pub fn remove_account(&self, id: i64) -> Result<()> {
        self.w().execute("DELETE FROM accounts WHERE id = ?", [id])?;
        Ok(())
    }

    pub fn list_accounts(&self) -> Vec<AccountRow> {
        let c = self.r();
        let mut st = c.prepare_cached("SELECT id, name, email, kind, config, sender_name FROM accounts ORDER BY id").expect("accounts");
        st.query_map([], |r| {
            Ok(AccountRow {
                id: r.get(0)?,
                name: r.get(1)?,
                email: r.get(2)?,
                kind: AccountKind::parse(&r.get::<_, String>(3)?),
                config: serde_json::from_str(&r.get::<_, String>(4)?).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e))
                })?,
                sender_name: r.get(5)?,
            })
        })
        .expect("accounts")
        .flatten()
        .collect()
    }

    pub fn account(&self, id: i64) -> Option<AccountRow> {
        self.list_accounts().into_iter().find(|a| a.id == id)
    }

    pub fn get_password(&self, account_id: i64) -> Result<String> {
        let blob: Vec<u8> = self
            .r()
            .query_row("SELECT secret FROM accounts WHERE id = ?", [account_id], |r| r.get(0))
            .optional()?
            .ok_or_else(|| anyhow!("no account {account_id}"))?;
        self.secrets.decrypt(&blob)
    }

    // ---------- mailboxes ----------

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_mailbox(
        &self,
        account_id: i64,
        path: &str,
        name: &str,
        delimiter: Option<&str>,
        special_use: Option<&str>,
        depth: i64,
        synced: bool,
    ) -> Result<i64> {
        let c = self.w();
        c.execute(
            "INSERT INTO mailboxes(account_id, path, name, delimiter, special_use, depth, synced) VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(account_id, path) DO UPDATE SET name = excluded.name, delimiter = excluded.delimiter,
               special_use = excluded.special_use, depth = excluded.depth, synced = excluded.synced",
            params![account_id, path, name, delimiter, special_use, depth, synced],
        )?;
        Ok(c.query_row("SELECT id FROM mailboxes WHERE account_id = ? AND path = ?", params![account_id, path], |r| r.get(0))?)
    }

    pub fn remove_mailboxes_except(&self, account_id: i64, keep: &HashSet<String>) -> Result<()> {
        let gone: Vec<i64> = self.mailbox_rows(account_id).into_iter().filter(|m| !keep.contains(&m.path)).map(|m| m.id).collect();
        let c = self.w();
        for id in gone {
            c.execute("DELETE FROM mailboxes WHERE id = ?", [id])?;
        }
        Ok(())
    }

    pub fn mailbox_rows(&self, account_id: i64) -> Vec<MailboxRow> {
        let c = self.r();
        let mut st = c
            .prepare_cached("SELECT id, path, special_use, synced, uidvalidity, modseq FROM mailboxes WHERE account_id = ?")
            .expect("mailboxes");
        st.query_map([account_id], |r| {
            Ok(MailboxRow {
                id: r.get(0)?,
                path: r.get(1)?,
                special_use: r.get(2)?,
                synced: r.get(3)?,
                uidvalidity: r.get(4)?,
                modseq: r.get(5)?,
            })
        })
        .expect("mailboxes")
        .flatten()
        .collect()
    }

    pub fn list_mailboxes(&self) -> Vec<Mailbox> {
        let c = self.r();
        let mut st = c
            .prepare_cached(
                r"SELECT id, account_id, path, name, special_use, depth FROM mailboxes WHERE synced = 1
                  ORDER BY account_id, CASE special_use WHEN '\Inbox' THEN 0 WHEN '\Drafts' THEN 1 WHEN '\Sent' THEN 2
                  WHEN '\Archive' THEN 3 WHEN '\All' THEN 4 WHEN '\Junk' THEN 8 WHEN '\Trash' THEN 9 ELSE 5 END, path",
            )
            .expect("mailboxes");
        st.query_map([], |r| {
            Ok(Mailbox {
                id: r.get(0)?,
                account_id: r.get(1)?,
                path: r.get(2)?,
                name: r.get(3)?,
                special_use: r.get(4)?,
                depth: r.get(5)?,
            })
        })
        .expect("mailboxes")
        .flatten()
        .collect()
    }

    pub fn find_mailbox(&self, account_id: i64, special_use: &str) -> Option<(i64, String)> {
        self.r()
            .query_row(
                "SELECT id, path FROM mailboxes WHERE account_id = ? AND special_use = ? LIMIT 1",
                params![account_id, special_use],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn set_mailbox_state(&self, id: i64, uidvalidity: &str, modseq: Option<u64>) -> Result<()> {
        self.w().execute(
            "UPDATE mailboxes SET uidvalidity = ?, modseq = ? WHERE id = ?",
            params![uidvalidity, modseq.map(|m| m.to_string()), id],
        )?;
        Ok(())
    }

    pub fn clear_mailbox(&self, id: i64) -> Result<()> {
        self.w().execute("DELETE FROM messages WHERE mailbox_id = ?", [id])?;
        Ok(())
    }

    // ---------- messages ----------

    /// uid → (message row id, seen)
    pub fn known_uids(&self, mailbox_id: i64) -> HashMap<u32, (i64, bool)> {
        let c = self.r();
        let mut st = c.prepare_cached("SELECT id, uid, seen FROM messages WHERE mailbox_id = ?").expect("uids");
        st.query_map([mailbox_id], |r| Ok((r.get::<_, u32>(1)?, (r.get::<_, i64>(0)?, r.get::<_, bool>(2)?))))
            .expect("uids")
            .flatten()
            .collect()
    }

    /// Thread assignment: Gmail thread id, else the parent's thread, else the root of References.
    fn thread_for(c: &Connection, m: &NewMessage) -> rusqlite::Result<String> {
        if let Some(t) = m.gmail_thread_id {
            return Ok(format!("g:{t}"));
        }
        let mut find = c.prepare_cached("SELECT thread_id FROM messages WHERE account_id = ? AND message_id = ? LIMIT 1")?;
        let parents = m.in_reply_to.iter().chain(m.references.iter().rev());
        for p in parents {
            if let Some(t) = find.query_row(params![m.account_id, p], |r| r.get::<_, String>(0)).optional()? {
                return Ok(t);
            }
        }
        if let Some(id) = &m.message_id {
            // A reply may have arrived before its parent.
            let mut child = c.prepare_cached("SELECT thread_id FROM messages WHERE account_id = ? AND in_reply_to = ? LIMIT 1")?;
            if let Some(t) = child.query_row(params![m.account_id, id], |r| r.get::<_, String>(0)).optional()? {
                return Ok(t);
            }
        }
        let root = m.references.first().or(m.in_reply_to.as_ref()).or(m.message_id.as_ref());
        Ok(match root {
            Some(r) => format!("r:{r}"),
            None => format!("u:{}:{}", m.mailbox_id, m.uid),
        })
    }

    pub fn insert_messages(&self, list: &[NewMessage]) -> Result<()> {
        let mut c = self.w();
        let tx = c.transaction()?;
        for m in list {
            let thread = Self::thread_for(&tx, m)?;
            let changed = tx.prepare_cached(
                "INSERT OR IGNORE INTO messages(account_id, mailbox_id, uid, message_id, thread_id, in_reply_to, refs, subject,
                   from_name, from_addr, to_json, cc_json, date, seen, size)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )?
            .execute(params![
                m.account_id,
                m.mailbox_id,
                m.uid,
                m.message_id,
                thread,
                m.in_reply_to,
                m.references.join(" "),
                m.subject,
                m.from.as_ref().and_then(|a| a.name.clone()),
                m.from.as_ref().map(|a| a.address.clone()),
                json(&m.to),
                json(&m.cc),
                m.date,
                m.seen,
                m.size,
            ])?;
            if changed == 0 {
                continue;
            }
            let id = tx.last_insert_rowid();
            // Reuse a body already fetched for the same message in another mailbox (Gmail labels).
            let twin = match &m.message_id {
                Some(mid) => tx
                    .prepare_cached(
                        "SELECT b.html, b.text, b.attachments, m.snippet FROM messages m JOIN bodies b ON b.message_id = m.id
                         WHERE m.account_id = ? AND m.message_id = ? AND m.id != ? LIMIT 1",
                    )?
                    .query_row(params![m.account_id, mid, id], |r| {
                        Ok(Body {
                            html: r.get(0)?,
                            text: r.get(1)?,
                            attachments: parse_json(r.get(2)?),
                            snippet: r.get(3)?,
                        })
                    })
                    .optional()?,
                None => None,
            };
            match twin {
                Some(b) => Self::save_body_tx(&tx, id, &b)?,
                None => {
                    tx.prepare_cached("INSERT INTO fts(rowid, subject, sender, recipients, body) VALUES (?, ?, ?, ?, '')")?.execute(
                        params![
                            id,
                            m.subject,
                            m.from.as_ref().map(|a| addr_text(std::slice::from_ref(a))).unwrap_or_default(),
                            addr_text(&[m.to.clone(), m.cc.clone()].concat()),
                        ],
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete_messages(&self, ids: &[i64]) -> Result<()> {
        let mut c = self.w();
        let tx = c.transaction()?;
        for id in ids {
            tx.prepare_cached("DELETE FROM messages WHERE id = ?")?.execute([id])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_seen_by_uid(&self, mailbox_id: i64, uids: &[u32], seen: bool) -> Result<()> {
        let mut c = self.w();
        let tx = c.transaction()?;
        for u in uids {
            tx.prepare_cached("UPDATE messages SET seen = ? WHERE mailbox_id = ? AND uid = ?")?.execute(params![seen, mailbox_id, u])?;
        }
        tx.commit()?;
        Ok(())
    }

    fn save_body_tx(c: &Connection, message_id: i64, b: &Body) -> rusqlite::Result<()> {
        c.prepare_cached("INSERT OR REPLACE INTO bodies(message_id, html, text, attachments) VALUES (?, ?, ?, ?)")?
            .execute(params![message_id, b.html, b.text, json(&b.attachments)])?;
        c.prepare_cached("UPDATE messages SET has_body = 1, snippet = ? WHERE id = ?")?.execute(params![b.snippet, message_id])?;
        let m = c
            .prepare_cached("SELECT subject, from_name, from_addr, to_json, cc_json FROM messages WHERE id = ?")?
            .query_row([message_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            })
            .optional()?;
        let Some((subject, from_name, from_addr, to, cc)) = m else { return Ok(()) };
        let mut recips: Vec<Address> = parse_json(to);
        recips.extend(parse_json::<Vec<Address>>(cc));
        c.prepare_cached("DELETE FROM fts WHERE rowid = ?")?.execute([message_id])?;
        c.prepare_cached("INSERT INTO fts(rowid, subject, sender, recipients, body) VALUES (?, ?, ?, ?, ?)")?.execute(params![
            message_id,
            subject,
            format!("{} {}", from_name.unwrap_or_default(), from_addr.unwrap_or_default()),
            addr_text(&recips),
            b.text.as_deref().unwrap_or(""),
        ])?;
        Ok(())
    }

    pub fn save_body(&self, message_id: i64, b: &Body) -> Result<()> {
        let mut c = self.w();
        let tx = c.transaction()?;
        Self::save_body_tx(&tx, message_id, b)?;
        tx.commit()?;
        Ok(())
    }

    /// Messages that still need a body, newest first.
    pub fn bodyless(&self, account_id: i64, since_ms: i64, max_size: u32, limit: usize) -> Vec<MessageLoc> {
        let c = self.r();
        let mut st = c
            .prepare_cached(
                "SELECT m.id, m.account_id, m.mailbox_id, b.path, m.uid FROM messages m JOIN mailboxes b ON b.id = m.mailbox_id
                 WHERE m.has_body = 0 AND m.account_id = ? AND m.date >= ? AND m.size <= ? ORDER BY m.date DESC LIMIT ?",
            )
            .expect("bodyless");
        st.query_map(params![account_id, since_ms, max_size, limit as i64], |r| {
            Ok(MessageLoc { id: r.get(0)?, account_id: r.get(1)?, mailbox_id: r.get(2)?, path: r.get(3)?, uid: r.get(4)?, has_body: false })
        })
        .expect("bodyless")
        .flatten()
        .collect()
    }

    pub fn message_location(&self, id: i64) -> Option<MessageLoc> {
        self.r()
            .query_row(
                "SELECT m.id, m.account_id, m.mailbox_id, b.path, m.uid, m.has_body FROM messages m
                 JOIN mailboxes b ON b.id = m.mailbox_id WHERE m.id = ?",
                [id],
                |r| {
                    Ok(MessageLoc {
                        id: r.get(0)?,
                        account_id: r.get(1)?,
                        mailbox_id: r.get(2)?,
                        path: r.get(3)?,
                        uid: r.get(4)?,
                        has_body: r.get(5)?,
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
    }

    // ---------- lists ----------

    pub fn list_threads(&self, view: &View, offset: usize, limit: usize) -> Vec<ThreadRow> {
        let limit = limit.min(PAGE_MAX) as i64;
        let offset = offset as i64;
        let c = self.r();
        type G = (i64, String, i64, bool);
        let map = |r: &Row| -> rusqlite::Result<G> { Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)) };
        let groups: Vec<G> = match view {
            View::Inbox | View::Sent | View::Mailbox(_) => {
                let ids: Vec<i64> = match view {
                    View::Mailbox(id) => vec![*id],
                    _ => {
                        let special = if *view == View::Sent { SENT } else { INBOX };
                        let mut st = c.prepare_cached("SELECT id FROM mailboxes WHERE special_use = ?").expect("inbox");
                        st.query_map([special], |r| r.get(0)).expect("inbox").flatten().collect()
                    }
                };
                // Unread first, then newest. Each folder is an index range scan; folders from
                // different accounts are merged here. The UI keeps positions stable while open.
                let want = offset + limit;
                let mut st = c
                    .prepare_cached(
                        "SELECT account_id, thread_id, last_date, all_seen FROM mailbox_threads
                         WHERE mailbox_id = ? ORDER BY all_seen ASC, last_date DESC LIMIT ?",
                    )
                    .expect("threads");
                let mut all: Vec<G> = Vec::new();
                for id in ids {
                    all.extend(st.query_map(params![id, want], map).expect("threads").flatten());
                }
                all.sort_by(|a, b| a.3.cmp(&b.3).then(b.2.cmp(&a.2)));
                all.into_iter().skip(offset as usize).take(limit as usize).collect()
            }
            View::Search(q) => {
                let q = fts_query(q);
                if q.is_empty() {
                    return Vec::new();
                }
                let mut st = c
                    .prepare_cached(
                        "SELECT m.account_id, m.thread_id, MAX(m.date) AS last_date, MIN(m.seen) AS all_seen
                         FROM fts JOIN messages m ON m.id = fts.rowid
                         WHERE fts MATCH ? GROUP BY m.account_id, m.thread_id ORDER BY last_date DESC LIMIT ? OFFSET ?",
                    )
                    .expect("search");
                st.query_map(params![q, limit, offset], map).expect("search").flatten().collect()
            }
            View::Drafts => return Vec::new(),
        };
        let rows = Self::decorate(&c, groups);
        if *view == View::Sent { Self::show_recipients(&c, rows) } else { rows }
    }

    /// In Sent, who you wrote to matters more than who wrote: label rows with the recipients
    /// of your latest sent message in the conversation.
    fn show_recipients(c: &Connection, rows: Vec<ThreadRow>) -> Vec<ThreadRow> {
        let mut st = c
            .prepare_cached(
                "SELECT m.to_json FROM messages m JOIN mailboxes b ON b.id = m.mailbox_id
                 WHERE m.account_id = ? AND m.thread_id = ? AND b.special_use = ? ORDER BY m.date DESC LIMIT 1",
            )
            .expect("sent to");
        rows.into_iter()
            .map(|mut r| {
                let to: Vec<Address> = parse_json(st.query_row(params![r.account_id, r.thread_id, SENT], |x| x.get(0)).ok());
                if !to.is_empty() {
                    let names: Vec<&str> = to.iter().map(|a| a.display()).collect();
                    r.from = format!("To: {}", names.join(", "));
                }
                r
            })
            .collect()
    }

    fn decorate(c: &Connection, groups: Vec<(i64, String, i64, bool)>) -> Vec<ThreadRow> {
        let mut latest = c
            .prepare_cached("SELECT subject, snippet FROM messages WHERE account_id = ? AND thread_id = ? ORDER BY date DESC LIMIT 1")
            .expect("latest");
        let mut senders = c
            .prepare_cached(
                "SELECT COALESCE(NULLIF(from_name, ''), from_addr) AS who, COUNT(DISTINCT COALESCE(message_id, id)) AS n
                 FROM messages WHERE account_id = ? AND thread_id = ? GROUP BY who ORDER BY MAX(date) DESC",
            )
            .expect("senders");
        groups
            .into_iter()
            .map(|(account_id, thread_id, date, all_seen)| {
                let (subject, snippet) = latest
                    .query_row(params![account_id, thread_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                    .unwrap_or_default();
                let s: Vec<(Option<String>, i64)> = senders
                    .query_map(params![account_id, thread_id], |r| Ok((r.get(0)?, r.get(1)?)))
                    .map(|it| it.flatten().collect())
                    .unwrap_or_default();
                let from = if s.len() > 1 {
                    s.iter()
                        .take(3)
                        .map(|(w, _)| w.as_deref().unwrap_or("").split_whitespace().next().unwrap_or("(unknown)").to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                } else {
                    s.first().and_then(|(w, _)| w.clone()).unwrap_or_else(|| "(unknown)".into())
                };
                ThreadRow {
                    account_id,
                    subject: if subject.is_empty() { "(no subject)".into() } else { subject },
                    thread_id,
                    from,
                    snippet,
                    date,
                    unread: !all_seen,
                    count: s.iter().map(|(_, n)| n).sum(),
                }
            })
            .collect()
    }

    fn row_to_message(r: &Row) -> rusqlite::Result<Message> {
        let from_addr: Option<String> = r.get("from_addr")?;
        let from_name: Option<String> = r.get("from_name")?;
        let refs: Option<String> = r.get("refs")?;
        Ok(Message {
            id: r.get("id")?,
            account_id: r.get("account_id")?,
            mailbox_id: r.get("mailbox_id")?,
            message_id: r.get("message_id")?,
            subject: r.get("subject")?,
            from: from_addr.map(|address| Address { name: from_name.filter(|n| !n.is_empty()), address }),
            to: parse_json(r.get("to_json")?),
            cc: parse_json(r.get("cc_json")?),
            date: r.get("date")?,
            seen: r.get("seen")?,
            has_body: r.get("has_body")?,
            html: r.get("html")?,
            text: r.get("text")?,
            attachments: parse_json(r.get("attachments")?),
            snippet: r.get("snippet")?,
            references: refs.map(|s| s.split(' ').filter(|x| !x.is_empty()).map(String::from).collect()).unwrap_or_default(),
        })
    }

    pub fn get_thread_messages(&self, account_id: i64, thread_id: &str) -> Vec<Message> {
        let c = self.r();
        let mut st = c
            .prepare_cached(
                "SELECT m.*, b.html, b.text, b.attachments FROM messages m LEFT JOIN bodies b ON b.message_id = m.id
                 WHERE m.account_id = ? AND m.thread_id = ? ORDER BY m.date ASC",
            )
            .expect("thread");
        let rows: Vec<Message> = st.query_map(params![account_id, thread_id], Self::row_to_message).expect("thread").flatten().collect();
        // The same message can live in several mailboxes (Gmail labels); show it once.
        let mut seen = HashSet::new();
        rows.into_iter().filter(|m| seen.insert(m.message_id.clone().unwrap_or_else(|| format!("#{}", m.id)))).collect()
    }

    pub fn get_message(&self, id: i64) -> Option<Message> {
        self.r()
            .query_row(
                "SELECT m.*, b.html, b.text, b.attachments FROM messages m LEFT JOIN bodies b ON b.message_id = m.id WHERE m.id = ?",
                [id],
                Self::row_to_message,
            )
            .optional()
            .ok()
            .flatten()
    }

    /// All copies of a thread's messages, with their mailbox, for server-side ops.
    pub fn thread_copies(&self, account_id: i64, thread_id: &str) -> Vec<Copy> {
        let c = self.r();
        let mut st = c
            .prepare_cached(
                "SELECT m.id, m.mailbox_id, b.path, b.special_use, m.uid, m.seen, COALESCE(m.message_id, CAST(m.id AS TEXT)) FROM messages m
                 JOIN mailboxes b ON b.id = m.mailbox_id WHERE m.account_id = ? AND m.thread_id = ?",
            )
            .expect("copies");
        st.query_map(params![account_id, thread_id], |r| {
            Ok(Copy { id: r.get(0)?, mailbox_id: r.get(1)?, path: r.get(2)?, special_use: r.get(3)?, uid: r.get(4)?, seen: r.get(5)?, message_key: r.get(6)? })
        })
        .expect("copies")
        .flatten()
        .collect()
    }

    // ---------- drafts ----------

    fn row_to_draft(r: &Row) -> rusqlite::Result<Draft> {
        Ok(Draft {
            id: r.get("id")?,
            account_id: r.get("account_id")?,
            to: r.get("to_list")?,
            cc: r.get("cc_list")?,
            subject: r.get("subject")?,
            body: r.get("body")?,
            reply_to: r.get("reply_to")?,
            created_by: r.get("created_by")?,
            updated_at: r.get("updated_at")?,
            attachments: parse_json(r.get("attachments")?),
        })
    }

    pub fn list_drafts(&self) -> Vec<Draft> {
        let c = self.r();
        let mut st = c.prepare_cached("SELECT * FROM drafts ORDER BY updated_at DESC").expect("drafts");
        st.query_map([], Self::row_to_draft).expect("drafts").flatten().collect()
    }

    pub fn get_draft(&self, id: i64) -> Option<Draft> {
        self.r().query_row("SELECT * FROM drafts WHERE id = ?", [id], Self::row_to_draft).optional().ok().flatten()
    }

    pub fn save_draft(&self, d: &DraftInput) -> Result<i64> {
        let c = self.w();
        let now = now_ms();
        if let Some(id) = d.id {
            let n = c.execute(
                "UPDATE drafts SET account_id = ?, to_list = ?, cc_list = ?, subject = ?, body = ?, reply_to = ?, attachments = ?, updated_at = ? WHERE id = ?",
                params![d.account_id, d.to, d.cc, d.subject, d.body, d.reply_to, json(&d.attachments), now, id],
            )?;
            if n > 0 {
                return Ok(id);
            }
        }
        c.execute(
            "INSERT INTO drafts(account_id, to_list, cc_list, subject, body, reply_to, created_by, attachments, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![d.account_id, d.to, d.cc, d.subject, d.body, d.reply_to, d.created_by.as_deref().unwrap_or("user"), json(&d.attachments), now],
        )?;
        Ok(c.last_insert_rowid())
    }

    pub fn delete_draft(&self, id: i64) -> Result<()> {
        self.w().execute("DELETE FROM drafts WHERE id = ?", [id])?;
        Ok(())
    }

    // ---------- ops (outbox of server changes) ----------

    pub fn enqueue_op(&self, account_id: i64, op: &Op) -> Result<()> {
        self.w().execute(
            "INSERT INTO ops(account_id, kind, payload, created_at) VALUES (?, ?, ?, ?)",
            params![account_id, op.kind(), json(op), now_ms()],
        )?;
        Ok(())
    }

    pub fn pending_ops(&self, account_id: i64) -> Vec<(i64, Op, i64)> {
        let c = self.r();
        let mut st = c.prepare_cached("SELECT id, payload, attempts FROM ops WHERE account_id = ? ORDER BY id").expect("ops");
        st.query_map([account_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))
            .expect("ops")
            .flatten()
            .filter_map(|(id, p, a)| serde_json::from_str(&p).ok().map(|op| (id, op, a)))
            .collect()
    }

    pub fn finish_op(&self, id: i64) -> Result<()> {
        self.w().execute("DELETE FROM ops WHERE id = ?", [id])?;
        Ok(())
    }

    pub fn fail_op(&self, id: i64) -> Result<()> {
        self.w().execute("UPDATE ops SET attempts = attempts + 1 WHERE id = ?", [id])?;
        Ok(())
    }

    /// Run several writes atomically (mark read / archive + their outbox entries).
    pub fn local_change(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<()>) -> Result<()> {
        let mut c = self.w();
        let tx = c.transaction()?;
        f(&tx)?;
        tx.commit()?;
        Ok(())
    }

    pub fn enqueue_op_tx(c: &Connection, account_id: i64, op: &Op) -> rusqlite::Result<()> {
        c.execute(
            "INSERT INTO ops(account_id, kind, payload, created_at) VALUES (?, ?, ?, ?)",
            params![account_id, op.kind(), json(op), now_ms()],
        )?;
        Ok(())
    }
}
