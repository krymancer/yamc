// End-to-end against a real IMAP/SMTP server. Needs GreenMail:
//   docker run -d --rm --name yamc-greenmail -e GREENMAIL_OPTS='-Dgreenmail.setup.test.all -Dgreenmail.hostname=0.0.0.0 -Dgreenmail.auth.disabled' \
//     -p 3025:3025 -p 3143:3143 greenmail/standalone
// then: YAMC_IT=1 npm test
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import nodemailer from 'nodemailer'
import { Client } from '@modelcontextprotocol/sdk/client/index.js'
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js'
import { Core } from '../src/core/core'
import { imapClient } from '../src/core/sync'
import { startMcpHttp } from '../src/core/mcp'
import type { AccountConfig } from '../src/shared/types'

const run = process.env.YAMC_IT ? describe : describe.skip
const user = `it${Date.now()}@localhost`
const config: AccountConfig = {
  imap: { host: '127.0.0.1', port: 3143, tls: 'none' },
  smtp: { host: '127.0.0.1', port: 3025, tls: 'none' },
  username: user,
}
const plain = { encrypt: (s: string) => Buffer.from(s), decrypt: (b: Buffer) => b.toString() }

async function until<T>(fn: () => T | Promise<T>, ok: (v: T) => boolean, ms = 15000): Promise<T> {
  const end = Date.now() + ms
  for (;;) {
    const v = await fn()
    if (ok(v)) return v
    if (Date.now() > end) throw new Error(`timed out; last value: ${JSON.stringify(v)?.slice(0, 500)}`)
    await new Promise((r) => setTimeout(r, 200))
  }
}

run('core against GreenMail', () => {
  let core: Core
  let accountId: number
  const smtp = nodemailer.createTransport({ host: '127.0.0.1', port: 3025, secure: false, ignoreTLS: true })

  beforeAll(async () => {
    // Make the mailbox exist with an Archive folder, plus mail waiting before first sync.
    const c = imapClient(config, 'pw')
    await c.connect()
    await c.mailboxCreate('Archive').catch(() => {})
    await c.logout()
    await smtp.sendMail({ from: 'Bob <bob@x.test>', to: user, subject: 'Lunch?', text: 'Lunch tomorrow?', messageId: '<lunch@x.test>' })
    await smtp.sendMail({
      from: 'Carol <carol@x.test>', to: user, subject: 'Re: Lunch?', text: 'I am in.\n> Lunch tomorrow?',
      inReplyTo: '<lunch@x.test>', references: '<lunch@x.test>',
    })
    await smtp.sendMail({
      from: 'News <news@x.test>', to: user, subject: 'Weekly digest',
      html: '<body style="background:#fff"><p style="color:#000">Hello <img src="https://example.com/t.png"></p></body>',
    })
    core = new Core({ dataDir: mkdtempSync(join(tmpdir(), 'yamc-it-')), secrets: plain, log: () => {} })
    accountId = await core.addAccount({ name: 'Me', email: user, kind: 'imap', config, password: 'pw' })
  })

  afterAll(async () => {
    await core?.stop()
  })

  it('syncs the inbox into conversations, unread first', async () => {
    const rows = await until(() => core.listThreads({ kind: 'inbox' }), (r) => r.length === 2)
    expect(rows.every((r) => r.unread)).toBe(true)
    const lunch = rows.find((r) => r.subject.includes('Lunch'))!
    expect(lunch.count).toBe(2)
    expect(rows[0].subject).toBe('Weekly digest')
  })

  it('downloads bodies and fills snippets', async () => {
    const rows = await until(() => core.listThreads({ kind: 'inbox' }), (r) => r.every((x) => x.snippet))
    expect(rows.find((r) => r.subject === 'Weekly digest')!.snippet).toContain('Hello')
    const t = core.getThread(accountId, rows.find((r) => r.subject.includes('Lunch'))!.threadId)
    const m = await core.loadMessage(t.messages[0].id)
    expect(m?.text).toContain('Lunch tomorrow?')
  })

  it('marks read on the server', async () => {
    const lunch = core.listThreads({ kind: 'inbox' }).find((r) => r.subject.includes('Lunch'))!
    core.markThread(accountId, lunch.threadId, true)
    expect(core.listThreads({ kind: 'inbox' }).find((r) => r.key === lunch.key)!.unread).toBe(false)
    const c = imapClient(config, 'pw')
    await c.connect()
    try {
      await until(async () => {
        const lock = await c.getMailboxLock('INBOX')
        try {
          return ((await c.search({ seen: true }, { uid: true })) || []).length
        } finally {
          lock.release()
        }
      }, (n) => n === 2)
    } finally {
      await c.logout()
    }
  })

  it('searches', () => {
    expect(core.listThreads({ kind: 'search', query: 'digest' }).map((r) => r.subject)).toEqual(['Weekly digest'])
  })

  it('sends mail that comes back via IDLE', async () => {
    const d = core.replyDraft(core.getThread(accountId, core.listThreads({ kind: 'inbox' }).find((r) => r.subject.includes('Lunch'))!.threadId).messages[1].id, false)
    expect(d.subject).toBe('Re: Lunch?')
    await core.send({ ...d, to: user, body: `See you there${d.body}` })
    const lunch = await until(
      () => core.listThreads({ kind: 'inbox' }).find((r) => r.subject.includes('Lunch')),
      (r) => r?.count === 3,
    )
    expect(lunch!.unread).toBe(true)
  })

  it('archives into the Archive folder', async () => {
    const digest = core.listThreads({ kind: 'inbox' }).find((r) => r.subject === 'Weekly digest')!
    core.archiveThread(accountId, digest.threadId)
    expect(core.listThreads({ kind: 'inbox' }).some((r) => r.key === digest.key)).toBe(false)
    const archive = core.listMailboxes().find((m) => m.specialUse === '\\Archive')!
    await until(() => core.listThreads({ kind: 'mailbox', mailboxId: archive.id }), (r) => r.length === 1, 20000)
  }, 30000)

  it('serves MCP tools over HTTP with a token', async () => {
    const port = 47000 + Math.floor(Math.random() * 1000)
    const http = await startMcpHttp(core, port, 'secret', () => {})
    try {
      const bad = await fetch(`http://127.0.0.1:${port}/mcp`, { method: 'POST', body: '{}' })
      expect(bad.status).toBe(401)
      const client = new Client({ name: 'test', version: '0' })
      await client.connect(new StreamableHTTPClientTransport(new URL(`http://127.0.0.1:${port}/mcp`), {
        requestInit: { headers: { Authorization: 'Bearer secret' } },
      }))
      const tools = (await client.listTools()).tools.map((t) => t.name).sort()
      expect(tools).toEqual(['archive', 'create_draft', 'get_thread', 'list_accounts', 'list_drafts', 'list_inbox', 'mark_read', 'search'])
      expect(tools).not.toContain('send')
      const call = async (name: string, args: Record<string, unknown>) =>
        JSON.parse(((await client.callTool({ name, arguments: args })).content as { text: string }[])[0].text)
      const inbox = await call('list_inbox', {})
      const lunch = inbox.find((t: { subject: string }) => t.subject.includes('Lunch'))
      const thread = await call('get_thread', lunch)
      expect(thread.messages[0].body).toContain('Lunch tomorrow?')
      const draft = await call('create_draft', { reply_to_message_id: thread.messages.at(-1).message_id, body: 'Sounds good' })
      expect(core.getDraft(draft.draft_id)).toMatchObject({ createdBy: 'agent', subject: 'Re: Lunch?' })
      await client.close()
    } finally {
      http.close()
    }
  })
})
