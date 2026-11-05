import { createServer, type IncomingMessage, type Server } from 'node:http'
import { timingSafeEqual } from 'node:crypto'
import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js'
import { StreamableHTTPServerTransport } from '@modelcontextprotocol/sdk/server/streamableHttp.js'
import { z } from 'zod'
import type { Core } from './core'
import { htmlToText } from './parse'

const BODY_MAX = 20_000

function json(value: unknown) {
  return { content: [{ type: 'text' as const, text: JSON.stringify(value, null, 2) }] }
}

function threadRef(t: { accountId: number; threadId: string }) {
  return { account_id: t.accountId, thread_id: t.threadId }
}

export function buildMcpServer(core: Core): McpServer {
  const server = new McpServer({ name: 'yamc', version: '0.0.1' }, {
    instructions:
      'yamc is the user\'s local mail client. Threads are identified by (account_id, thread_id). ' +
      'You cannot send mail: create_draft puts a draft in the app and the user reviews and sends it.',
  })

  const rows = (list: ReturnType<Core['listThreads']>) =>
    list.map((t) => ({ ...threadRef(t), subject: t.subject, from: t.from, snippet: t.snippet, date: new Date(t.date).toISOString(), unread: t.unread, messages: t.count }))

  server.registerTool(
    'list_accounts',
    { description: 'List mail accounts and their folders.', inputSchema: {} },
    async () => {
      const boxes = core.listMailboxes()
      return json(
        core.listAccounts().map((a) => ({
          id: a.id,
          name: a.name,
          email: a.email,
          kind: a.kind,
          state: a.state,
          folders: boxes.filter((b) => b.accountId === a.id).map((b) => ({ mailbox_id: b.id, path: b.path, special_use: b.specialUse })),
        })),
      )
    },
  )

  server.registerTool(
    'list_inbox',
    {
      description: 'List conversations in the shared inbox (all accounts), unread first then newest. Optionally a single folder.',
      inputSchema: {
        mailbox_id: z.number().int().optional().describe('Folder to list instead of the shared inbox (from list_accounts).'),
        unread_only: z.boolean().optional(),
        limit: z.number().int().min(1).max(500).optional(),
      },
    },
    async ({ mailbox_id, unread_only, limit }) => {
      const view = mailbox_id ? ({ kind: 'mailbox', mailboxId: mailbox_id } as const) : ({ kind: 'inbox' } as const)
      let list = core.listThreads(view, 0, limit ?? 50)
      if (unread_only) list = list.filter((t) => t.unread)
      return json(rows(list))
    },
  )

  server.registerTool(
    'search',
    {
      description: 'Full-text search across all accounts (subject, sender, recipients, downloaded bodies). Newest first.',
      inputSchema: { query: z.string().min(1), limit: z.number().int().min(1).max(200).optional() },
    },
    async ({ query, limit }) => json(rows(core.listThreads({ kind: 'search', query }, 0, limit ?? 25))),
  )

  server.registerTool(
    'get_thread',
    {
      description: 'Read a whole conversation as plain text. Downloads bodies that are not cached yet. Does not mark it read.',
      inputSchema: { account_id: z.number().int(), thread_id: z.string() },
    },
    async ({ account_id, thread_id }) => {
      const t = core.getThread(account_id, thread_id)
      const messages = []
      for (const m of t.messages) {
        const full = (await core.loadMessage(m.id)) ?? m
        const text = full.text ?? (full.html ? htmlToText(full.html) : '')
        messages.push({
          message_id: full.id,
          from: full.from,
          to: full.to,
          cc: full.cc,
          date: new Date(full.date).toISOString(),
          subject: full.subject,
          unread: !full.seen,
          attachments: full.attachments,
          body: text.length > BODY_MAX ? `${text.slice(0, BODY_MAX)}\n[truncated]` : text,
        })
      }
      return json({ ...threadRef(t), subject: t.subject, messages })
    },
  )

  server.registerTool(
    'create_draft',
    {
      description:
        'Create a draft for the user to review and send in yamc. Plain text body. For a reply, pass reply_to_message_id ' +
        '(from get_thread) and the recipients/subject are filled in unless given.',
      inputSchema: {
        account_id: z.number().int().optional().describe('Required unless replying.'),
        reply_to_message_id: z.number().int().optional(),
        reply_all: z.boolean().optional(),
        to: z.string().optional().describe('Comma-separated addresses.'),
        cc: z.string().optional(),
        subject: z.string().optional(),
        body: z.string(),
      },
    },
    async (a) => {
      let draft
      if (a.reply_to_message_id) {
        const base = core.replyDraft(a.reply_to_message_id, !!a.reply_all)
        draft = { ...base, to: a.to ?? base.to, cc: a.cc ?? base.cc, subject: a.subject ?? base.subject, body: `${a.body}${base.body}` }
      } else {
        if (!a.account_id) throw new Error('account_id is required for a new message')
        draft = { accountId: a.account_id, to: a.to ?? '', cc: a.cc ?? '', subject: a.subject ?? '', body: a.body, replyTo: null }
      }
      const id = core.saveDraft({ ...draft, createdBy: 'agent' })
      return json({ draft_id: id, note: 'Draft saved. The user will review and send it from yamc.' })
    },
  )

  server.registerTool(
    'list_drafts',
    { description: 'List drafts waiting in yamc.', inputSchema: {} },
    async () => json(core.listDrafts()),
  )

  server.registerTool(
    'mark_read',
    {
      description: 'Mark a conversation read (or unread with read=false).',
      inputSchema: { account_id: z.number().int(), thread_id: z.string(), read: z.boolean().optional() },
    },
    async ({ account_id, thread_id, read }) => {
      core.markThread(account_id, thread_id, read ?? true)
      return json({ ok: true })
    },
  )

  server.registerTool(
    'archive',
    {
      description: 'Archive a conversation (remove it from the inbox).',
      inputSchema: { account_id: z.number().int(), thread_id: z.string() },
    },
    async ({ account_id, thread_id }) => {
      core.archiveThread(account_id, thread_id)
      return json({ ok: true })
    },
  )

  return server
}

function readBody(req: IncomingMessage): Promise<unknown> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = []
    req.on('data', (c) => chunks.push(c))
    req.on('end', () => {
      if (!chunks.length) return resolve(undefined)
      try {
        resolve(JSON.parse(Buffer.concat(chunks).toString('utf8')))
      } catch (e) {
        reject(e)
      }
    })
    req.on('error', reject)
  })
}

function tokenOk(header: string | undefined, token: string): boolean {
  const got = Buffer.from(header?.replace(/^Bearer\s+/i, '') ?? '')
  const want = Buffer.from(token)
  return got.length === want.length && timingSafeEqual(got, want)
}

/** Stateless Streamable HTTP MCP endpoint on 127.0.0.1. */
export function startMcpHttp(core: Core, port: number, token: string, log: (...a: unknown[]) => void = console.log): Promise<Server> {
  const http = createServer(async (req, res) => {
    const host = req.headers.host ?? ''
    if (host !== `127.0.0.1:${port}` && host !== `localhost:${port}`) {
      res.writeHead(403).end('bad host')
      return
    }
    if (!req.url?.startsWith('/mcp')) {
      res.writeHead(404).end()
      return
    }
    if (!tokenOk(req.headers.authorization, token)) {
      res.writeHead(401, { 'content-type': 'application/json' }).end(JSON.stringify({ error: 'unauthorized' }))
      return
    }
    if (req.method !== 'POST') {
      res.writeHead(405, { allow: 'POST' }).end()
      return
    }
    try {
      const body = await readBody(req)
      const server = buildMcpServer(core)
      const transport = new StreamableHTTPServerTransport({ sessionIdGenerator: undefined, enableJsonResponse: true })
      res.on('close', () => {
        void transport.close()
        void server.close()
      })
      await server.connect(transport)
      await transport.handleRequest(req, res, body)
    } catch (err) {
      log('mcp error', err)
      if (!res.headersSent) res.writeHead(500).end()
    }
  })
  return new Promise((resolve, reject) => {
    http.once('error', reject)
    http.listen(port, '127.0.0.1', () => resolve(http))
  })
}
