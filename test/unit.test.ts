import { describe, expect, it } from 'vitest'
import { mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { Store, ftsQuery, type NewMessage } from '../src/core/db'
import { mergeStable } from '../src/renderer/src/ThreadList'
import { Recolor, parseColor } from '../src/renderer/src/darken'
import { getTheme } from '../src/shared/themes'
import { makeSnippet } from '../src/core/parse'
import type { ThreadRow } from '../src/shared/types'

const plain = { encrypt: (s: string) => Buffer.from(s), decrypt: (b: Buffer) => b.toString() }

function store() {
  return new Store(join(mkdtempSync(join(tmpdir(), 'yamc-')), 'db'), plain)
}

function msg(p: Partial<NewMessage>): NewMessage {
  return {
    accountId: 1, mailboxId: 1, uid: 1, messageId: null, gmailThreadId: null, inReplyTo: null, references: [],
    subject: 's', from: { address: 'a@x' }, to: [], cc: [], date: 1, seen: false, size: 1, ...p,
  }
}

function seed(s: Store) {
  const acct = s.addAccount({ name: 'A', email: 'a@x', kind: 'imap', color: '#fff', password: 'p',
    config: { imap: { host: 'h', port: 1, tls: 'tls' }, smtp: { host: 'h', port: 1, tls: 'tls' }, username: 'a' } })
  const inbox = s.upsertMailbox({ accountId: acct, path: 'INBOX', name: 'Inbox', delimiter: '/', specialUse: '\\Inbox', depth: 0, synced: true })
  return { acct, inbox }
}

describe('store', () => {
  it('threads replies by References and In-Reply-To, even out of order', () => {
    const s = store()
    const { acct, inbox } = seed(s)
    s.insertMessages([msg({ accountId: acct, mailboxId: inbox, uid: 2, messageId: '<b>', inReplyTo: '<a>', references: ['<a>'], date: 2 })])
    s.insertMessages([msg({ accountId: acct, mailboxId: inbox, uid: 1, messageId: '<a>', date: 1, seen: true })])
    s.insertMessages([msg({ accountId: acct, mailboxId: inbox, uid: 3, messageId: '<c>', inReplyTo: '<b>', references: ['<a>', '<b>'], date: 3 })])
    s.insertMessages([msg({ accountId: acct, mailboxId: inbox, uid: 4, messageId: '<z>', date: 4, seen: true })])
    const rows = s.listThreads({ kind: 'inbox' }, 0, 50)
    expect(rows).toHaveLength(2)
    expect(rows[0].unread).toBe(true)
    expect(rows[0].count).toBe(3)
    expect(rows[1].unread).toBe(false)
  })

  it('puts unread first, then newest', () => {
    const s = store()
    const { acct, inbox } = seed(s)
    s.insertMessages([
      msg({ accountId: acct, mailboxId: inbox, uid: 1, messageId: '<1>', date: 10, seen: true }),
      msg({ accountId: acct, mailboxId: inbox, uid: 2, messageId: '<2>', date: 5 }),
      msg({ accountId: acct, mailboxId: inbox, uid: 3, messageId: '<3>', date: 20, seen: true }),
      msg({ accountId: acct, mailboxId: inbox, uid: 4, messageId: '<4>', date: 1 }),
    ])
    expect(s.listThreads({ kind: 'inbox' }, 0, 50).map((r) => r.threadId)).toEqual(['r:<2>', 'r:<4>', 'r:<3>', 'r:<1>'])
  })

  it('searches subject and body', () => {
    const s = store()
    const { acct, inbox } = seed(s)
    s.insertMessages([msg({ accountId: acct, mailboxId: inbox, uid: 1, messageId: '<1>', subject: 'Quarterly invoice' })])
    s.insertMessages([msg({ accountId: acct, mailboxId: inbox, uid: 2, messageId: '<2>', subject: 'hello' })])
    const id = s.listThreads({ kind: 'inbox' }, 0, 9).find((r) => r.subject === 'hello')!
    const m = s.getThreadMessages(acct, id.threadId)[0]
    s.saveBody(m.id, { html: null, text: 'the pelican flies', attachments: [], snippet: 'the pelican flies' })
    expect(s.listThreads({ kind: 'search', query: 'invo' }, 0, 9).map((r) => r.subject)).toEqual(['Quarterly invoice'])
    expect(s.listThreads({ kind: 'search', query: 'pelican' }, 0, 9).map((r) => r.subject)).toEqual(['hello'])
    expect(ftsQuery('a "b')).toBe('"a"* "b"*')
  })

  it('lists 100k messages fast', () => {
    const s = store()
    const { acct, inbox } = seed(s)
    const batch: NewMessage[] = []
    for (let i = 0; i < 100_000; i++) {
      batch.push(msg({ accountId: acct, mailboxId: inbox, uid: i + 1, messageId: `<m${i}>`, date: i, seen: i % 7 !== 0, subject: `subject ${i}` }))
    }
    s.insertMessages(batch)
    const t0 = performance.now()
    const rows = s.listThreads({ kind: 'inbox' }, 0, 300)
    const ms = performance.now() - t0
    expect(rows[0].unread).toBe(true)
    console.log(`listThreads over 100k messages: ${ms.toFixed(1)}ms`)
    expect(ms).toBeLessThan(500)
  }, 120_000)
})

describe('mergeStable', () => {
  const row = (key: string, date: number, unread = false) => ({ key, date, unread }) as ThreadRow
  it('keeps positions, adds new mail on top, drops removed rows', () => {
    const prev = [row('a', 5, true), row('b', 4, true), row('c', 3)]
    // Server order changed because 'a' was read; 'd' is new; 'b' was archived.
    const next = [row('d', 9, true), row('c', 3), row('a', 5)]
    expect(mergeStable(prev, next, new Map()).map((r) => r.key)).toEqual(['d', 'a', 'c'])
  })
})

describe('recolor', () => {
  const mocha = getTheme('mocha', 'mauve', true).vars
  const r = new Recolor({ bg: mocha['mail-bg'], text: mocha['mail-text'] })
  it('turns white into the theme base and black into the theme text', () => {
    // Mocha base #1e1e2e is hsl(240, 21%, 15%); text #cdd6f4 is hsl(226, 64%, 88%).
    expect(r.background(parseColor('rgb(255, 255, 255)')!)).toBe('hsla(240.0, 21.1%, 14.9%, 1)')
    expect(r.foreground(parseColor('rgb(0, 0, 0)')!)).toBe('hsla(226.2, 63.9%, 88.0%, 1)')
  })
  it('leaves dark backgrounds, light text and transparency alone', () => {
    expect(r.background(parseColor('rgb(10, 10, 10)')!)).toBeNull()
    expect(r.background(parseColor('rgba(0, 0, 0, 0)')!)).toBeNull()
    expect(r.foreground(parseColor('rgb(240, 240, 240)')!)).toBeNull()
  })
  it('treats near-white tints as grey', () => {
    expect(r.background(parseColor('#eef1f6')!)).toMatch(/^hsla\(240\.0, 21\.1%/)
  })
  it('keeps the hue of coloured backgrounds', () => {
    expect(r.background(parseColor('#1a73e8')!)).toMatch(/^hsla\(214/)
  })
})

describe('snippet', () => {
  it('skips quoted lines', () => {
    expect(makeSnippet('Thanks!\n> old stuff\n  more')).toBe('Thanks! more')
  })
})

import { KeyEngine, keyToken, tokenize, type Binding } from '../src/renderer/src/keys'

describe('key engine', () => {
  const log: string[] = []
  const b = (keys: string[], name: string, modes: Binding['modes'] = 'all'): Binding => ({
    keys, modes, group: '', desc: name, run: (n) => log.push(`${name}:${n ?? '-'}`),
  })
  const bindings = [b(['j', '<Down>'], 'down'), b(['gg'], 'top'), b(['G'], 'bottom'), b(['gi'], 'inbox'), b(['dd'], 'archive', ['list']), b(['d'], 'colours', ['thread']), b(['<C-d>'], 'half')]
  const feed = (e: KeyEngine, keys: string[], mode: 'list' | 'thread' = 'list') => {
    for (const k of keys) {
      const r = e.feed(k, bindings, mode)
      if (r.kind === 'run') r.binding.run(r.count)
    }
  }

  it('handles counts, sequences and chords', () => {
    log.length = 0
    const e = new KeyEngine()
    feed(e, ['5', 'j', 'g', 'g', '1', '2', 'G', 'd', 'd', '<C-d>', '<Down>'])
    expect(log).toEqual(['down:5', 'top:-', 'bottom:12', 'archive:-', 'half:-', 'down:-'])
  })

  it('scopes bindings by mode and recovers from unknown sequences', () => {
    log.length = 0
    const e = new KeyEngine()
    feed(e, ['d'], 'thread')
    feed(e, ['g', 'x', 'j'])
    expect(log).toEqual(['colours:-', 'down:-'])
  })

  it('runs the shorter match on timeout', () => {
    log.length = 0
    const e = new KeyEngine()
    const both = [...bindings, b(['z'], 'z'), b(['zz'], 'center')]
    expect(e.feed('z', both, 'list').kind).toBe('pending')
    const r = e.flush()
    if (r.kind === 'run') r.binding.run(r.count)
    expect(log).toEqual(['z:-'])
  })

  it('normalizes keys', () => {
    expect(keyToken({ key: 'D', ctrlKey: true, metaKey: false, altKey: false })).toBe('<C-d>')
    expect(keyToken({ key: 'Enter', ctrlKey: false, metaKey: false, altKey: false })).toBe('<CR>')
    expect(keyToken({ key: 'G', ctrlKey: false, metaKey: false, altKey: false })).toBe('G')
    expect(keyToken({ key: 'Shift', ctrlKey: false, metaKey: false, altKey: false })).toBeNull()
    expect(tokenize('g<CR>')).toEqual(['g', '<CR>'])
  })
})
