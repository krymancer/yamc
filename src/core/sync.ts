import { ImapFlow, type FetchMessageObject, type ListResponse } from 'imapflow'
import type { Account, AccountConfig, Address, ServerConfig, SyncState } from '@shared/types'
import type { NewMessage, Store } from './db'
import { parseSource } from './parse'

const HEADER_BATCH = 250
const BODY_BATCH = 20
const BODY_PREFETCH_MAX = 2 * 1024 * 1024
const POLL_MS = 5 * 60 * 1000
const DAY = 24 * 60 * 60 * 1000

/** Folders that only duplicate others (Gmail) or that nobody wants indexed. */
const SKIP_SPECIAL = new Set(['\\Junk', '\\Trash', '\\All', '\\Flagged', '\\Important'])

export interface SyncHooks {
  changed(): void
  state(accountId: number, state: SyncState, error?: string): void
  log(...args: unknown[]): void
}

const LOOPBACK = new Set(['127.0.0.1', 'localhost', '::1'])

export function tlsOptions(cfg: AccountConfig, server: ServerConfig) {
  return { rejectUnauthorized: !(cfg.allowSelfSigned && LOOPBACK.has(server.host)) }
}

export function imapClient(cfg: AccountConfig, password: string): ImapFlow {
  const s = cfg.imap
  return new ImapFlow({
    host: s.host,
    port: s.port,
    secure: s.tls === 'tls',
    doSTARTTLS: s.tls === 'starttls' ? true : s.tls === 'none' ? false : undefined,
    auth: { user: cfg.username, pass: password },
    tls: tlsOptions(cfg, s),
    logger: false,
    clientInfo: { name: 'yamc' },
  } as ConstructorParameters<typeof ImapFlow>[0])
}

function parseIds(raw: string | undefined | null): string[] {
  return raw ? (raw.match(/<[^<>\s]+>/g) ?? []) : []
}

function headerValue(headers: Buffer | undefined, name: string): string | null {
  if (!headers) return null
  const text = headers.toString('utf8').replace(/\r?\n[ \t]+/g, ' ')
  const m = text.match(new RegExp(`^${name}:\\s*(.*)$`, 'im'))
  return m ? m[1].trim() : null
}

function addr(a: { name?: string; address?: string } | undefined): Address | null {
  return a?.address ? { name: a.name || undefined, address: a.address } : null
}

export function toNewMessage(accountId: number, mailboxId: number, msg: FetchMessageObject): NewMessage {
  const env = msg.envelope
  const date = env?.date ?? msg.internalDate
  return {
    accountId,
    mailboxId,
    uid: msg.uid,
    messageId: env?.messageId?.trim() || null,
    gmailThreadId: msg.threadId ?? null,
    inReplyTo: parseIds(env?.inReplyTo)[0] ?? null,
    references: parseIds(headerValue(msg.headers, 'references')),
    subject: env?.subject ?? '',
    from: addr(env?.from?.[0]),
    to: (env?.to ?? []).map(addr).filter(Boolean) as Address[],
    cc: (env?.cc ?? []).map(addr).filter(Boolean) as Address[],
    date: date ? new Date(date).getTime() || Date.now() : Date.now(),
    seen: msg.flags?.has('\\Seen') ?? false,
    size: msg.size ?? 0,
  }
}

type Task = () => Promise<void>

/** One IMAP session per account. Urgent work (opening a message, user ops) jumps the queue. */
export class AccountSync {
  private client: ImapFlow | null = null
  private urgent: Task[] = []
  private background: Task[] = []
  private running = false
  private stopped = false
  private pollTimer: NodeJS.Timeout | null = null
  private retryMs = 5000
  private failedBodies = new Set<number>()
  private bodyWaiters = new Map<number, ((ok: boolean) => void)[]>()
  state: SyncState = 'idle'
  error?: string

  constructor(
    private store: Store,
    readonly account: Omit<Account, 'state' | 'error'>,
    private hooks: SyncHooks,
    private bodyDays: () => number,
  ) {}

  start(): void {
    this.stopped = false
    this.schedule('background', () => this.fullSync())
  }

  async stop(): Promise<void> {
    this.stopped = true
    if (this.pollTimer) clearInterval(this.pollTimer)
    this.urgent = []
    this.background = []
    const c = this.client
    this.client = null
    await c?.logout().catch(() => c.close())
  }

  /** Push local changes (mark read, archive) to the server soon. */
  kick(): void {
    this.schedule('urgent', () => this.flushOps())
  }

  /** Fetch one message body now. Resolves true when it's in the database. */
  fetchBody(messageId: number): Promise<boolean> {
    return new Promise((resolve) => {
      const waiters = this.bodyWaiters.get(messageId)
      if (waiters) return waiters.push(resolve)
      this.bodyWaiters.set(messageId, [resolve])
      this.schedule('urgent', async () => {
        const loc = this.store.messageLocation(messageId)
        let ok = !!loc?.hasBody
        if (loc && !ok) ok = (await this.fetchBodies(loc.path, [{ id: messageId, uid: loc.uid }])) > 0
        this.resolveBody(messageId, ok)
      })
    })
  }

  private resolveBody(id: number, ok: boolean) {
    this.bodyWaiters.get(id)?.forEach((r) => r(ok))
    this.bodyWaiters.delete(id)
  }

  private setState(state: SyncState, error?: string) {
    this.state = state
    this.error = error
    this.hooks.state(this.account.id, state, error)
  }

  private schedule(lane: 'urgent' | 'background', task: Task) {
    if (this.stopped) return
    ;(lane === 'urgent' ? this.urgent : this.background).push(task)
    void this.pump()
  }

  private async pump() {
    if (this.running) return
    this.running = true
    try {
      while (!this.stopped) {
        const task = this.urgent.shift() ?? this.background.shift()
        if (!task) break
        try {
          await this.ensureClient()
          await task()
          this.retryMs = 5000
        } catch (err) {
          this.hooks.log(`[${this.account.email}]`, err)
          this.dropClient()
          this.setState('error', err instanceof Error ? err.message : String(err))
          for (const id of [...this.bodyWaiters.keys()]) this.resolveBody(id, false)
          this.urgent = []
          this.background = []
          const wait = this.retryMs
          this.retryMs = Math.min(this.retryMs * 2, 5 * 60 * 1000)
          setTimeout(() => this.schedule('background', () => this.fullSync()), wait)
          break
        }
      }
    } finally {
      this.running = false
    }
    if (!this.stopped && (this.urgent.length || this.background.length)) void this.pump()
  }

  private dropClient() {
    const c = this.client
    this.client = null
    c?.close()
  }

  private async ensureClient(): Promise<ImapFlow> {
    if (this.client?.usable) return this.client
    this.dropClient()
    const c = imapClient(this.account.config, this.store.getPassword(this.account.id))
    c.on('error', (err: Error) => this.hooks.log(`[${this.account.email}] imap error`, err.message))
    c.on('close', () => {
      if (this.client !== c || this.stopped) return
      this.client = null
      this.setState('offline')
      setTimeout(() => this.schedule('background', () => this.fullSync()), this.retryMs)
    })
    c.on('exists', (ev: { path: string }) => {
      const row = this.store.mailboxRows(this.account.id).find((m) => m.path === ev.path)
      if (row) this.schedule('urgent', () => this.syncMailbox(row.id, row.path))
    })
    c.on('flags', (ev: { path: string; uid?: number; flags: Set<string> }) => {
      const row = this.store.mailboxRows(this.account.id).find((m) => m.path === ev.path)
      if (!row || !ev.uid) return
      this.store.setSeenByUid(row.id, [ev.uid], ev.flags.has('\\Seen'))
      this.hooks.changed()
    })
    await c.connect()
    this.client = c
    return c
  }

  private async fullSync(): Promise<void> {
    this.setState('syncing')
    await this.flushOps()
    await this.syncMailboxList()
    const rows = this.store.mailboxRows(this.account.id).filter((m) => m.synced)
    rows.sort((a, b) => (a.special_use === '\\Inbox' ? -1 : b.special_use === '\\Inbox' ? 1 : 0))
    for (const r of rows) await this.syncMailbox(r.id, r.path)
    this.setState('idle')
    await this.watchInbox()
    if (!this.pollTimer) this.pollTimer = setInterval(() => this.schedule('background', () => this.fullSync()), POLL_MS)
    this.schedule('background', () => this.backfillBodies())
  }

  /** Select INBOX and leave it selected so imapflow's auto-IDLE watches it. */
  private async watchInbox() {
    const c = this.client!
    const lock = await c.getMailboxLock('INBOX')
    lock.release()
  }

  private async syncMailboxList() {
    const list: ListResponse[] = await this.client!.list()
    const kept: string[] = []
    for (const m of list) {
      if (m.flags.has('\\Noselect') || m.flags.has('\\NonExistent')) continue
      const special = m.path.toUpperCase() === 'INBOX' ? '\\Inbox' : (m.specialUse ?? null)
      kept.push(m.path)
      this.store.upsertMailbox({
        accountId: this.account.id,
        path: m.path,
        name: m.path.toUpperCase() === 'INBOX' ? 'Inbox' : m.name,
        delimiter: m.delimiter,
        specialUse: special,
        depth: m.parent.length,
        synced: !special || !SKIP_SPECIAL.has(special),
      })
    }
    this.store.removeMailboxesExcept(this.account.id, kept)
    this.hooks.changed()
  }

  private async syncMailbox(mailboxId: number, path: string) {
    const c = this.client!
    const row = this.store.mailboxRows(this.account.id).find((m) => m.id === mailboxId)
    if (!row) return
    const lock = await c.getMailboxLock(path)
    try {
      const mb = c.mailbox
      if (!mb) return
      const uidValidity = mb.uidValidity.toString()
      if (row.uidvalidity && row.uidvalidity !== uidValidity) this.store.clearMailbox(mailboxId)

      const known = this.store.knownUids(mailboxId)
      const serverUids = mb.exists ? ((await c.search({ all: true }, { uid: true })) || []) : []
      const onServer = new Set(serverUids)
      const removed = [...known].filter(([uid]) => !onServer.has(uid)).map(([, v]) => v.id)
      if (removed.length) this.store.deleteMessages(removed)

      // Newest first, so the top of the inbox fills in before old history.
      const missing = serverUids.filter((u) => !known.has(u)).sort((a, b) => b - a)
      for (let i = 0; i < missing.length; i += HEADER_BATCH) {
        if (this.stopped) return
        const batch = missing.slice(i, i + HEADER_BATCH)
        const msgs: NewMessage[] = []
        for await (const msg of c.fetch(
          batch.join(','),
          { uid: true, envelope: true, flags: true, internalDate: true, size: true, threadId: true, headers: ['references'] },
          { uid: true },
        )) {
          msgs.push(toNewMessage(this.account.id, mailboxId, msg))
        }
        this.store.insertMessages(msgs)
        this.hooks.changed()
      }

      // Flag changes on messages we already had.
      if (known.size) {
        const prev = row.modseq ? BigInt(row.modseq) : undefined
        const canDiff = prev !== undefined && mb.highestModseq !== undefined
        if (!(canDiff && mb.highestModseq === prev)) {
          const seen: number[] = []
          const unseen: number[] = []
          for await (const msg of c.fetch('1:*', { uid: true, flags: true }, { uid: true, changedSince: canDiff ? prev : undefined })) {
            const k = known.get(msg.uid)
            if (!k) continue
            const s = msg.flags?.has('\\Seen') ?? false
            if (s !== k.seen) (s ? seen : unseen).push(msg.uid)
          }
          if (seen.length) this.store.setSeenByUid(mailboxId, seen, true)
          if (unseen.length) this.store.setSeenByUid(mailboxId, unseen, false)
          if (seen.length || unseen.length) this.hooks.changed()
        }
      }
      this.store.setMailboxState(mailboxId, uidValidity, mb.highestModseq?.toString() ?? null)
    } finally {
      lock.release()
    }
  }

  /** Download bodies for recent mail in small chunks, yielding to urgent work between chunks. */
  private async backfillBodies() {
    const since = Date.now() - this.bodyDays() * DAY
    const todo = this.store
      .bodyless(this.account.id, since, BODY_PREFETCH_MAX, BODY_BATCH * 4)
      .filter((m) => !this.failedBodies.has(m.id))
      .slice(0, BODY_BATCH)
    if (!todo.length) return
    const byPath = new Map<string, { id: number; uid: number }[]>()
    for (const m of todo) byPath.set(m.path, [...(byPath.get(m.path) ?? []), { id: m.id, uid: m.uid }])
    for (const [path, items] of byPath) await this.fetchBodies(path, items)
    this.schedule('background', () => this.backfillBodies())
  }

  private async fetchBodies(path: string, items: { id: number; uid: number }[]): Promise<number> {
    const c = this.client!
    const byUid = new Map(items.map((i) => [i.uid, i.id]))
    let done = 0
    const lock = await c.getMailboxLock(path)
    try {
      for await (const msg of c.fetch(items.map((i) => i.uid).join(','), { uid: true, source: true }, { uid: true })) {
        const id = byUid.get(msg.uid)
        if (!id || !msg.source) continue
        try {
          this.store.saveBody(id, await parseSource(msg.source))
          byUid.delete(msg.uid)
          done++
        } catch (err) {
          this.hooks.log('parse failed', id, err)
        }
      }
    } finally {
      lock.release()
    }
    for (const id of byUid.values()) this.failedBodies.add(id)
    if (done) this.hooks.changed()
    return done
  }

  private async flushOps() {
    const c = this.client!
    const touched = new Set<string>()
    for (const { id, op, attempts } of this.store.pendingOps(this.account.id)) {
      try {
        const lock = await c.getMailboxLock(op.mailboxPath)
        try {
          const range = op.uids.join(',')
          if (op.kind === 'seen') {
            if (op.seen) await c.messageFlagsAdd(range, ['\\Seen'], { uid: true })
            else await c.messageFlagsRemove(range, ['\\Seen'], { uid: true })
          } else {
            await c.messageMove(range, op.target, { uid: true })
            touched.add(op.target)
          }
        } finally {
          lock.release()
        }
        this.store.finishOp(id)
      } catch (err) {
        this.hooks.log('op failed', op, err)
        if (attempts >= 5) this.store.finishOp(id)
        else this.store.failOp(id)
      }
    }
    // Moved mail shows up in its new folder now, not at the next poll.
    for (const row of this.store.mailboxRows(this.account.id)) {
      if (row.synced && touched.has(row.path)) this.schedule('urgent', () => this.syncMailbox(row.id, row.path))
    }
  }
}
