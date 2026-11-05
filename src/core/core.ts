import nodemailer from 'nodemailer'
import { join } from 'node:path'
import { mkdirSync } from 'node:fs'
import type { Account, AccountConfig, AccountInput, CoreEvent, Draft, DraftInput, Message, Settings, SyncState, Thread, View } from '@shared/types'
import { ACCOUNT_COLORS } from '@shared/presets'
import { Store, type SecretBox } from './db'
import { AccountSync, imapClient, tlsOptions } from './sync'

export interface CoreOptions {
  dataDir: string
  secrets: SecretBox
  log?: (...args: unknown[]) => void
}

function smtpTransport(cfg: AccountConfig, password: string) {
  const s = cfg.smtp
  return nodemailer.createTransport({
    host: s.host,
    port: s.port,
    secure: s.tls === 'tls',
    requireTLS: s.tls === 'starttls',
    ignoreTLS: s.tls === 'none',
    auth: { user: cfg.username, pass: password },
    tls: tlsOptions(cfg, s),
  })
}

function splitAddresses(list: string): string[] {
  return list
    .split(/[,;]/)
    .map((s) => s.trim())
    .filter(Boolean)
}

function fmtAddress(a: { name?: string; address: string }): string {
  return a.name ? `${a.name} <${a.address}>` : a.address
}

/** Everything the UI and MCP server can do. No Electron imports. */
export class Core {
  readonly store: Store
  private syncs = new Map<number, AccountSync>()
  private listeners = new Set<(e: CoreEvent) => void>()
  private threadsTimer: NodeJS.Timeout | null = null
  private log: (...args: unknown[]) => void

  constructor(opts: CoreOptions) {
    mkdirSync(opts.dataDir, { recursive: true })
    this.store = new Store(join(opts.dataDir, 'yamc.db'), opts.secrets)
    this.log = opts.log ?? console.log
  }

  start(): void {
    for (const a of this.store.listAccounts()) this.startSync(a)
  }

  async stop(): Promise<void> {
    await Promise.all([...this.syncs.values()].map((s) => s.stop()))
    this.store.close()
  }

  on(cb: (e: CoreEvent) => void): () => void {
    this.listeners.add(cb)
    return () => this.listeners.delete(cb)
  }

  private emit(e: CoreEvent) {
    if (e.type === 'threads') {
      // Sync can change thousands of rows per second; tell the UI at most every 150ms.
      if (this.threadsTimer) return
      this.threadsTimer = setTimeout(() => {
        this.threadsTimer = null
        this.listeners.forEach((l) => l(e))
      }, 150)
      return
    }
    this.listeners.forEach((l) => l(e))
  }

  private startSync(a: Omit<Account, 'state' | 'error'>) {
    const s = new AccountSync(
      this.store,
      a,
      {
        changed: () => this.emit({ type: 'threads' }),
        state: (_id: number, _st: SyncState) => this.emit({ type: 'accounts' }),
        log: this.log,
      },
      () => this.store.getSettings().bodyDays,
    )
    this.syncs.set(a.id, s)
    s.start()
  }

  private sync(accountId: number): AccountSync {
    const s = this.syncs.get(accountId)
    if (!s) throw new Error(`Unknown account ${accountId}`)
    return s
  }

  // ---------- accounts ----------

  listAccounts(): Account[] {
    return this.store.listAccounts().map((a) => {
      const s = this.syncs.get(a.id)
      return { ...a, state: s?.state ?? 'idle', error: s?.error }
    })
  }

  async testAccount(input: AccountInput): Promise<void> {
    const imap = imapClient(input.config, input.password)
    imap.on('error', () => {})
    try {
      await imap.connect()
    } catch (err) {
      throw new Error(`IMAP: ${err instanceof Error ? err.message : err}`)
    } finally {
      await imap.logout().catch(() => imap.close())
    }
    try {
      await smtpTransport(input.config, input.password).verify()
    } catch (err) {
      throw new Error(`SMTP: ${err instanceof Error ? err.message : err}`)
    }
  }

  async addAccount(input: AccountInput): Promise<number> {
    await this.testAccount(input)
    const color = ACCOUNT_COLORS[this.store.listAccounts().length % ACCOUNT_COLORS.length]
    const id = this.store.addAccount({ ...input, color })
    this.startSync(this.store.listAccounts().find((a) => a.id === id)!)
    this.emit({ type: 'accounts' })
    return id
  }

  async removeAccount(id: number): Promise<void> {
    await this.syncs.get(id)?.stop()
    this.syncs.delete(id)
    this.store.removeAccount(id)
    this.emit({ type: 'accounts' })
    this.emit({ type: 'threads' })
  }

  // ---------- reading ----------

  listMailboxes() {
    return this.store.listMailboxes()
  }

  listThreads(view: View, offset = 0, limit = 200) {
    return this.store.listThreads(view, offset, limit)
  }

  getThread(accountId: number, threadId: string): Thread {
    const messages = this.store.getThreadMessages(accountId, threadId)
    return { accountId, threadId, subject: messages.at(-1)?.subject || '(no subject)', messages }
  }

  /** Returns the message with its body, downloading it if needed. */
  async loadMessage(id: number): Promise<Message | undefined> {
    const m = this.store.getMessage(id)
    if (!m || m.hasBody) return m
    await this.sync(m.accountId).fetchBody(id)
    return this.store.getMessage(id)
  }

  // ---------- actions ----------

  markThread(accountId: number, threadId: string, seen = true): void {
    const changed = this.store.threadCopies(accountId, threadId).filter((c) => c.seen !== seen)
    if (!changed.length) return
    const byMailbox = new Map<number, { path: string; uids: number[] }>()
    for (const c of changed) {
      const g = byMailbox.get(c.mailboxId) ?? { path: c.path, uids: [] }
      g.uids.push(c.uid)
      byMailbox.set(c.mailboxId, g)
    }
    this.store.tx(() => {
      for (const [mailboxId, g] of byMailbox) {
        this.store.setSeenByUid(mailboxId, g.uids, seen)
        this.store.enqueueOp(accountId, { kind: 'seen', mailboxPath: g.path, uids: g.uids, seen })
      }
    })
    this.sync(accountId).kick()
    this.emit({ type: 'threads' })
  }

  archiveThread(accountId: number, threadId: string): void {
    const target = this.store.findMailbox(accountId, '\\Archive') ?? this.store.findMailbox(accountId, '\\All')
    if (!target) throw new Error('This account has no Archive folder')
    const inInbox = this.store.threadCopies(accountId, threadId).filter((c) => c.specialUse === '\\Inbox')
    if (!inInbox.length) return
    const byPath = new Map<string, number[]>()
    for (const c of inInbox) byPath.set(c.path, [...(byPath.get(c.path) ?? []), c.uid])
    this.store.tx(() => {
      this.store.deleteMessages(inInbox.map((c) => c.id))
      for (const [path, uids] of byPath) this.store.enqueueOp(accountId, { kind: 'move', mailboxPath: path, uids, target: target.path })
    })
    this.sync(accountId).kick()
    this.emit({ type: 'threads' })
  }

  // ---------- drafts & sending ----------

  listDrafts(): Draft[] {
    return this.store.listDrafts()
  }

  getDraft(id: number) {
    return this.store.getDraft(id)
  }

  saveDraft(d: DraftInput): number {
    const id = this.store.saveDraft(d)
    this.emit({ type: 'drafts' })
    return id
  }

  deleteDraft(id: number): void {
    this.store.deleteDraft(id)
    this.emit({ type: 'drafts' })
  }

  /** Build a reply draft (not saved) for a message. */
  replyDraft(messageId: number, all: boolean): DraftInput {
    const m = this.store.getMessage(messageId)
    if (!m) throw new Error(`No message ${messageId}`)
    const me = this.store.listAccounts().find((a) => a.id === m.accountId)!.email.toLowerCase()
    const fromMe = m.from?.address.toLowerCase() === me
    const to = fromMe ? m.to : m.from ? [m.from] : []
    const toSet = new Set(to.map((a) => a.address.toLowerCase()))
    const cc = all ? [...m.to, ...m.cc].filter((a) => a.address.toLowerCase() !== me && !toSet.has(a.address.toLowerCase())) : []
    const when = new Date(m.date).toLocaleString()
    const quoted = (m.text ?? '')
      .split('\n')
      .map((l) => `> ${l}`)
      .join('\n')
    return {
      accountId: m.accountId,
      to: to.map(fmtAddress).join(', '),
      cc: cc.map(fmtAddress).join(', '),
      subject: /^re:/i.test(m.subject) ? m.subject : `Re: ${m.subject}`,
      body: `\n\nOn ${when}, ${m.from ? fmtAddress(m.from) : 'someone'} wrote:\n${quoted}`,
      replyTo: m.id,
    }
  }

  async send(d: DraftInput): Promise<void> {
    const account = this.store.listAccounts().find((a) => a.id === d.accountId)
    if (!account) throw new Error('Unknown account')
    const to = splitAddresses(d.to)
    if (!to.length) throw new Error('No recipients')
    const parent = d.replyTo ? this.store.getMessage(d.replyTo) : undefined
    const mail = {
      from: { name: account.name, address: account.email },
      to,
      cc: splitAddresses(d.cc),
      subject: d.subject,
      text: d.body,
      inReplyTo: parent?.messageId ?? undefined,
      references: parent?.messageId ? [...parent.references, parent.messageId] : undefined,
    }
    const password = this.store.getPassword(account.id)
    await smtpTransport(account.config, password).sendMail(mail)
    if (!account.config.serverSavesSent) {
      const sent = this.store.findMailbox(account.id, '\\Sent')
      if (sent) {
        const raw = await nodemailer.createTransport({ streamTransport: true, buffer: true }).sendMail(mail)
        await this.appendTo(account.config, password, sent.path, raw.message as Buffer).catch((e) => this.log('append sent failed', e))
      }
    }
    if (d.id) this.deleteDraft(d.id)
  }

  private async appendTo(cfg: AccountConfig, password: string, path: string, raw: Buffer) {
    const c = imapClient(cfg, password)
    c.on('error', () => {})
    await c.connect()
    try {
      await c.append(path, raw, ['\\Seen'])
    } finally {
      await c.logout().catch(() => c.close())
    }
  }

  // ---------- settings ----------

  getSettings(): Settings {
    return this.store.getSettings()
  }

  setSettings(patch: Partial<Settings>): Settings {
    this.store.setSettings(patch)
    return this.store.getSettings()
  }
}
