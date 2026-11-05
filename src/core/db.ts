import Database from 'better-sqlite3'
import type {
  Account,
  AccountConfig,
  AccountKind,
  Address,
  Attachment,
  Draft,
  DraftInput,
  Mailbox,
  Message,
  Settings,
  ThreadRow,
  View,
} from '@shared/types'
import { randomBytes } from 'node:crypto'

const SCHEMA = `
CREATE TABLE IF NOT EXISTS accounts (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  email TEXT NOT NULL,
  kind TEXT NOT NULL,
  color TEXT NOT NULL,
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
  updated_at INTEGER NOT NULL
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
`

export interface SecretBox {
  encrypt(plain: string): Buffer
  decrypt(blob: Buffer): string
}

export interface NewMessage {
  accountId: number
  mailboxId: number
  uid: number
  messageId: string | null
  gmailThreadId: string | null
  inReplyTo: string | null
  references: string[]
  subject: string
  from: Address | null
  to: Address[]
  cc: Address[]
  date: number
  seen: boolean
  size: number
}

export interface Body {
  html: string | null
  text: string | null
  attachments: Attachment[]
  snippet: string
}

export type Op =
  | { kind: 'seen'; mailboxPath: string; uids: number[]; seen: boolean }
  | { kind: 'move'; mailboxPath: string; uids: number[]; target: string }

const DEFAULT_SETTINGS: Settings = {
  theme: 'zinc-dark',
  accent: 'mauve',
  bodyDays: 90,
  mcpEnabled: true,
  mcpPort: 47823,
  mcpToken: '',
  originalColorSenders: [],
}

const INBOX = '\\Inbox'
const PAGE_MAX = 1000

function addrText(a: Address[]): string {
  return a.map((x) => `${x.name ?? ''} ${x.address}`).join(' ')
}

/** Turn free text into a safe FTS5 query: every word becomes a quoted prefix term. */
export function ftsQuery(input: string): string {
  const terms = input
    .split(/\s+/)
    .map((t) => t.replace(/"/g, '').trim())
    .filter(Boolean)
  return terms.map((t) => `"${t}"*`).join(' ')
}

export class Store {
  readonly db: Database.Database

  constructor(
    path: string,
    private secrets: SecretBox,
  ) {
    this.db = new Database(path)
    this.db.pragma('journal_mode = WAL')
    this.db.pragma('synchronous = NORMAL')
    this.db.pragma('foreign_keys = ON')
    this.db.pragma('cache_size = -65536')
    this.db.pragma('temp_store = MEMORY')
    this.db.pragma('mmap_size = 268435456')
    this.db.exec(SCHEMA)
  }

  close(): void {
    this.db.close()
  }

  tx<T>(fn: () => T): T {
    return this.db.transaction(fn)()
  }

  // ---------- settings ----------

  getSettings(): Settings {
    const rows = this.db.prepare('SELECT key, value FROM settings').all() as { key: string; value: string }[]
    const s: Settings = { ...DEFAULT_SETTINGS }
    for (const r of rows) (s as unknown as Record<string, unknown>)[r.key] = JSON.parse(r.value)
    if (!s.mcpToken) {
      s.mcpToken = randomBytes(24).toString('base64url')
      this.setSettings({ mcpToken: s.mcpToken })
    }
    return s
  }

  setSettings(patch: Partial<Settings>): void {
    const stmt = this.db.prepare('INSERT INTO settings(key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value')
    this.tx(() => {
      for (const [k, v] of Object.entries(patch)) stmt.run(k, JSON.stringify(v))
    })
  }

  // ---------- accounts ----------

  addAccount(a: { name: string; email: string; kind: AccountKind; color: string; config: AccountConfig; password: string }): number {
    const r = this.db
      .prepare('INSERT INTO accounts(name, email, kind, color, config, secret, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)')
      .run(a.name, a.email, a.kind, a.color, JSON.stringify(a.config), this.secrets.encrypt(a.password), Date.now())
    return Number(r.lastInsertRowid)
  }

  removeAccount(id: number): void {
    this.db.prepare('DELETE FROM accounts WHERE id = ?').run(id)
  }

  listAccounts(): Omit<Account, 'state' | 'error'>[] {
    const rows = this.db.prepare('SELECT id, name, email, kind, color, config FROM accounts ORDER BY id').all() as any[]
    return rows.map((r) => ({ ...r, config: JSON.parse(r.config) }))
  }

  getPassword(accountId: number): string {
    const r = this.db.prepare('SELECT secret FROM accounts WHERE id = ?').get(accountId) as { secret: Buffer } | undefined
    if (!r) throw new Error(`No account ${accountId}`)
    return this.secrets.decrypt(r.secret)
  }

  // ---------- mailboxes ----------

  upsertMailbox(m: { accountId: number; path: string; name: string; delimiter: string; specialUse: string | null; depth: number; synced: boolean }): number {
    this.db
      .prepare(
        `INSERT INTO mailboxes(account_id, path, name, delimiter, special_use, depth, synced) VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(account_id, path) DO UPDATE SET name = excluded.name, delimiter = excluded.delimiter,
           special_use = excluded.special_use, depth = excluded.depth, synced = excluded.synced`,
      )
      .run(m.accountId, m.path, m.name, m.delimiter, m.specialUse, m.depth, m.synced ? 1 : 0)
    return (this.db.prepare('SELECT id FROM mailboxes WHERE account_id = ? AND path = ?').get(m.accountId, m.path) as { id: number }).id
  }

  removeMailboxesExcept(accountId: number, paths: string[]): void {
    const keep = new Set(paths)
    for (const m of this.mailboxRows(accountId)) if (!keep.has(m.path)) this.db.prepare('DELETE FROM mailboxes WHERE id = ?').run(m.id)
  }

  mailboxRows(accountId: number): { id: number; path: string; special_use: string | null; synced: number; uidvalidity: string | null; modseq: string | null }[] {
    return this.db.prepare('SELECT id, path, special_use, synced, uidvalidity, modseq FROM mailboxes WHERE account_id = ?').all(accountId) as any[]
  }

  listMailboxes(): Mailbox[] {
    return this.db
      .prepare(
        `SELECT id, account_id AS accountId, path, name, special_use AS specialUse, depth FROM mailboxes
         WHERE synced = 1 ORDER BY account_id, CASE special_use WHEN '\\Inbox' THEN 0 WHEN '\\Drafts' THEN 1 WHEN '\\Sent' THEN 2 WHEN '\\Archive' THEN 3 WHEN '\\All' THEN 4 WHEN '\\Junk' THEN 8 WHEN '\\Trash' THEN 9 ELSE 5 END, path`,
      )
      .all() as Mailbox[]
  }

  findMailbox(accountId: number, specialUse: string): { id: number; path: string } | undefined {
    return this.db.prepare('SELECT id, path FROM mailboxes WHERE account_id = ? AND special_use = ?').get(accountId, specialUse) as any
  }

  setMailboxState(id: number, uidvalidity: string, modseq: string | null): void {
    this.db.prepare('UPDATE mailboxes SET uidvalidity = ?, modseq = ? WHERE id = ?').run(uidvalidity, modseq, id)
  }

  clearMailbox(id: number): void {
    this.db.prepare('DELETE FROM messages WHERE mailbox_id = ?').run(id)
  }

  // ---------- messages ----------

  knownUids(mailboxId: number): Map<number, { id: number; seen: boolean }> {
    const rows = this.db.prepare('SELECT id, uid, seen FROM messages WHERE mailbox_id = ?').all(mailboxId) as { id: number; uid: number; seen: number }[]
    return new Map(rows.map((r) => [r.uid, { id: r.id, seen: !!r.seen }]))
  }

  /** Thread assignment: Gmail thread id, else the parent's thread, else the root of References. */
  private threadFor(m: NewMessage): string {
    if (m.gmailThreadId) return `g:${m.gmailThreadId}`
    const parents = [m.inReplyTo, ...[...m.references].reverse()].filter(Boolean) as string[]
    const find = this.db.prepare('SELECT thread_id FROM messages WHERE account_id = ? AND message_id = ? LIMIT 1')
    for (const p of parents) {
      const r = find.get(m.accountId, p) as { thread_id: string } | undefined
      if (r) return r.thread_id
    }
    if (m.messageId) {
      // A reply may have arrived before its parent.
      const child = this.db
        .prepare('SELECT thread_id FROM messages WHERE account_id = ? AND in_reply_to = ? LIMIT 1')
        .get(m.accountId, m.messageId) as { thread_id: string } | undefined
      if (child) return child.thread_id
    }
    const root = m.references[0] ?? m.inReplyTo ?? m.messageId
    return root ? `r:${root}` : `u:${m.mailboxId}:${m.uid}`
  }

  insertMessages(list: NewMessage[]): void {
    const ins = this.db.prepare(
      `INSERT OR IGNORE INTO messages(account_id, mailbox_id, uid, message_id, thread_id, in_reply_to, refs, subject,
         from_name, from_addr, to_json, cc_json, date, seen, size)
       VALUES (@accountId, @mailboxId, @uid, @messageId, @threadId, @inReplyTo, @refs, @subject,
         @fromName, @fromAddr, @to, @cc, @date, @seen, @size)`,
    )
    const fts = this.db.prepare('INSERT INTO fts(rowid, subject, sender, recipients, body) VALUES (?, ?, ?, ?, ?)')
    // Reuse a body already fetched for the same message in another mailbox (Gmail labels).
    const twin = this.db.prepare(
      `SELECT b.html, b.text, b.attachments, m.snippet FROM messages m JOIN bodies b ON b.message_id = m.id
       WHERE m.account_id = ? AND m.message_id = ? LIMIT 1`,
    )
    this.tx(() => {
      for (const m of list) {
        const r = ins.run({
          accountId: m.accountId,
          mailboxId: m.mailboxId,
          uid: m.uid,
          messageId: m.messageId,
          threadId: this.threadFor(m),
          inReplyTo: m.inReplyTo,
          refs: m.references.join(' '),
          subject: m.subject,
          fromName: m.from?.name ?? null,
          fromAddr: m.from?.address ?? null,
          to: JSON.stringify(m.to),
          cc: JSON.stringify(m.cc),
          date: m.date,
          seen: m.seen ? 1 : 0,
          size: m.size,
        })
        if (!r.changes) continue
        const id = Number(r.lastInsertRowid)
        const t = m.messageId ? (twin.get(m.accountId, m.messageId) as any) : undefined
        if (t) {
          this.saveBody(id, { html: t.html, text: t.text, attachments: JSON.parse(t.attachments), snippet: t.snippet })
        } else {
          fts.run(id, m.subject, m.from ? addrText([m.from]) : '', addrText([...m.to, ...m.cc]), '')
        }
      }
    })
  }

  deleteMessages(ids: number[]): void {
    const del = this.db.prepare('DELETE FROM messages WHERE id = ?')
    this.tx(() => ids.forEach((id) => del.run(id)))
  }

  setSeenByUid(mailboxId: number, uids: number[], seen: boolean): void {
    const st = this.db.prepare('UPDATE messages SET seen = ? WHERE mailbox_id = ? AND uid = ?')
    this.tx(() => uids.forEach((u) => st.run(seen ? 1 : 0, mailboxId, u)))
  }

  saveBody(messageId: number, b: Body): void {
    this.tx(() => {
      this.db
        .prepare('INSERT OR REPLACE INTO bodies(message_id, html, text, attachments) VALUES (?, ?, ?, ?)')
        .run(messageId, b.html, b.text, JSON.stringify(b.attachments))
      this.db.prepare('UPDATE messages SET has_body = 1, snippet = ? WHERE id = ?').run(b.snippet, messageId)
      const m = this.db.prepare('SELECT subject, from_name, from_addr, to_json, cc_json FROM messages WHERE id = ?').get(messageId) as any
      if (!m) return
      const recipients = addrText([...JSON.parse(m.to_json), ...JSON.parse(m.cc_json)])
      this.db
        .prepare('INSERT OR REPLACE INTO fts(rowid, subject, sender, recipients, body) VALUES (?, ?, ?, ?, ?)')
        .run(messageId, m.subject, `${m.from_name ?? ''} ${m.from_addr ?? ''}`, recipients, b.text ?? '')
    })
  }

  /** Messages that still need a body, newest first. */
  bodyless(accountId: number, sinceMs: number, maxSize: number, limit: number): { id: number; accountId: number; mailboxId: number; path: string; uid: number }[] {
    return this.db
      .prepare(
        `SELECT m.id, m.account_id AS accountId, m.mailbox_id AS mailboxId, b.path, m.uid FROM messages m
         JOIN mailboxes b ON b.id = m.mailbox_id
         WHERE m.has_body = 0 AND m.account_id = ? AND m.date >= ? AND m.size <= ? ORDER BY m.date DESC LIMIT ?`,
      )
      .all(accountId, sinceMs, maxSize, limit) as any[]
  }

  messageLocation(id: number): { accountId: number; mailboxId: number; path: string; uid: number; hasBody: boolean } | undefined {
    const r = this.db
      .prepare('SELECT m.account_id AS accountId, m.mailbox_id AS mailboxId, b.path, m.uid, m.has_body AS hasBody FROM messages m JOIN mailboxes b ON b.id = m.mailbox_id WHERE m.id = ?')
      .get(id) as any
    return r ? { ...r, hasBody: !!r.hasBody } : undefined
  }

  // ---------- lists ----------

  private inboxIds(): number[] {
    return (this.db.prepare('SELECT id FROM mailboxes WHERE special_use = ?').all(INBOX) as { id: number }[]).map((r) => r.id)
  }

  listThreads(view: View, offset: number, limit: number): ThreadRow[] {
    limit = Math.min(limit, PAGE_MAX)
    let groups: { account_id: number; thread_id: string; last_date: number; all_seen: number }[]
    if (view.kind === 'inbox' || view.kind === 'mailbox') {
      const ids = view.kind === 'inbox' ? this.inboxIds() : [view.mailboxId]
      if (!ids.length) return []
      // Unread first, then newest. The renderer keeps positions stable while the view is open.
      groups = this.db
        .prepare(
          `SELECT account_id, thread_id, MAX(date) AS last_date, MIN(seen) AS all_seen FROM messages
           WHERE mailbox_id IN (${ids.map(() => '?').join(',')})
           GROUP BY account_id, thread_id ORDER BY all_seen ASC, last_date DESC LIMIT ? OFFSET ?`,
        )
        .all(...ids, limit, offset) as any[]
    } else if (view.kind === 'search') {
      const q = ftsQuery(view.query)
      if (!q) return []
      groups = this.db
        .prepare(
          `SELECT m.account_id, m.thread_id, MAX(m.date) AS last_date, MIN(m.seen) AS all_seen
           FROM fts JOIN messages m ON m.id = fts.rowid
           WHERE fts MATCH ? GROUP BY m.account_id, m.thread_id ORDER BY last_date DESC LIMIT ? OFFSET ?`,
        )
        .all(q, limit, offset) as any[]
    } else {
      return []
    }
    return this.decorate(groups)
  }

  private decorate(groups: { account_id: number; thread_id: string; last_date: number; all_seen: number }[]): ThreadRow[] {
    const latest = this.db.prepare(
      `SELECT subject, snippet FROM messages WHERE account_id = ? AND thread_id = ? ORDER BY date DESC LIMIT 1`,
    )
    const senders = this.db.prepare(
      `SELECT COALESCE(NULLIF(from_name, ''), from_addr) AS who, COUNT(DISTINCT COALESCE(message_id, id)) AS n
       FROM messages WHERE account_id = ? AND thread_id = ? GROUP BY who ORDER BY MAX(date) DESC`,
    )
    return groups.map((g) => {
      const l = latest.get(g.account_id, g.thread_id) as { subject: string; snippet: string }
      const s = senders.all(g.account_id, g.thread_id) as { who: string | null; n: number }[]
      const names = s.map((x) => (x.who ?? '').split(/\s+/)[0] || '(unknown)')
      return {
        key: `${g.account_id}:${g.thread_id}`,
        accountId: g.account_id,
        threadId: g.thread_id,
        subject: l?.subject || '(no subject)',
        from: s.length > 1 ? names.slice(0, 3).join(', ') : (s[0]?.who ?? '(unknown)'),
        snippet: l?.snippet ?? '',
        date: g.last_date,
        unread: g.all_seen === 0,
        count: s.reduce((a, x) => a + x.n, 0),
      }
    })
  }

  getThreadMessages(accountId: number, threadId: string): Message[] {
    const rows = this.db
      .prepare(
        `SELECT m.*, b.html, b.text, b.attachments FROM messages m LEFT JOIN bodies b ON b.message_id = m.id
         WHERE m.account_id = ? AND m.thread_id = ? ORDER BY m.date ASC`,
      )
      .all(accountId, threadId) as any[]
    const seen = new Set<string>()
    const out: Message[] = []
    for (const r of rows) {
      // The same message can live in several mailboxes (Gmail labels); show it once.
      const k = r.message_id ?? `#${r.id}`
      if (seen.has(k)) continue
      seen.add(k)
      out.push(rowToMessage(r))
    }
    return out
  }

  getMessage(id: number): Message | undefined {
    const r = this.db.prepare('SELECT m.*, b.html, b.text, b.attachments FROM messages m LEFT JOIN bodies b ON b.message_id = m.id WHERE m.id = ?').get(id)
    return r ? rowToMessage(r) : undefined
  }

  /** All copies of a thread's messages, with their mailbox, for server-side ops. */
  threadCopies(accountId: number, threadId: string): { id: number; mailboxId: number; path: string; specialUse: string | null; uid: number; seen: boolean }[] {
    return (
      this.db
        .prepare(
          `SELECT m.id, m.mailbox_id AS mailboxId, b.path, b.special_use AS specialUse, m.uid, m.seen FROM messages m
           JOIN mailboxes b ON b.id = m.mailbox_id WHERE m.account_id = ? AND m.thread_id = ?`,
        )
        .all(accountId, threadId) as any[]
    ).map((r) => ({ ...r, seen: !!r.seen }))
  }

  // ---------- drafts ----------

  listDrafts(): Draft[] {
    return (this.db.prepare('SELECT * FROM drafts ORDER BY updated_at DESC').all() as any[]).map(rowToDraft)
  }

  getDraft(id: number): Draft | undefined {
    const r = this.db.prepare('SELECT * FROM drafts WHERE id = ?').get(id)
    return r ? rowToDraft(r) : undefined
  }

  saveDraft(d: DraftInput): number {
    const now = Date.now()
    if (d.id) {
      this.db
        .prepare('UPDATE drafts SET account_id = ?, to_list = ?, cc_list = ?, subject = ?, body = ?, reply_to = ?, updated_at = ? WHERE id = ?')
        .run(d.accountId, d.to, d.cc, d.subject, d.body, d.replyTo, now, d.id)
      return d.id
    }
    const r = this.db
      .prepare('INSERT INTO drafts(account_id, to_list, cc_list, subject, body, reply_to, created_by, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)')
      .run(d.accountId, d.to, d.cc, d.subject, d.body, d.replyTo, d.createdBy ?? 'user', now)
    return Number(r.lastInsertRowid)
  }

  deleteDraft(id: number): void {
    this.db.prepare('DELETE FROM drafts WHERE id = ?').run(id)
  }

  // ---------- ops (outbox of server changes) ----------

  enqueueOp(accountId: number, op: Op): void {
    this.db.prepare('INSERT INTO ops(account_id, kind, payload, created_at) VALUES (?, ?, ?, ?)').run(accountId, op.kind, JSON.stringify(op), Date.now())
  }

  pendingOps(accountId: number): { id: number; op: Op; attempts: number }[] {
    return (this.db.prepare('SELECT id, payload, attempts FROM ops WHERE account_id = ? ORDER BY id').all(accountId) as any[]).map((r) => ({
      id: r.id,
      op: JSON.parse(r.payload),
      attempts: r.attempts,
    }))
  }

  finishOp(id: number): void {
    this.db.prepare('DELETE FROM ops WHERE id = ?').run(id)
  }

  failOp(id: number): void {
    this.db.prepare('UPDATE ops SET attempts = attempts + 1 WHERE id = ?').run(id)
  }
}

function parseJson<T>(s: string | null, fallback: T): T {
  if (!s) return fallback
  try {
    return JSON.parse(s)
  } catch {
    return fallback
  }
}

function rowToMessage(r: any): Message {
  return {
    id: r.id,
    accountId: r.account_id,
    mailboxId: r.mailbox_id,
    messageId: r.message_id,
    subject: r.subject,
    from: r.from_addr ? { name: r.from_name ?? undefined, address: r.from_addr } : null,
    to: parseJson(r.to_json, []),
    cc: parseJson(r.cc_json, []),
    date: r.date,
    seen: !!r.seen,
    hasBody: !!r.has_body,
    html: r.html ?? null,
    text: r.text ?? null,
    attachments: parseJson(r.attachments, []),
    snippet: r.snippet,
    references: r.refs ? String(r.refs).split(' ').filter(Boolean) : [],
  }
}

function rowToDraft(r: any): Draft {
  return {
    id: r.id,
    accountId: r.account_id,
    to: r.to_list,
    cc: r.cc_list,
    subject: r.subject,
    body: r.body,
    replyTo: r.reply_to,
    createdBy: r.created_by,
    updatedAt: r.updated_at,
  }
}
