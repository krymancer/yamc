import { useCallback, useEffect, useRef, useState } from 'react'
import type { Account, Draft, DraftInput, Mailbox, Settings, ThreadRow, View } from '@shared/types'
import { api, onEvent } from './api'
import { ThreadList, type ListHandle } from './ThreadList'
import { ThreadView } from './ThreadView'
import { Composer } from './Composer'
import { SettingsView } from './Settings'
import { ChevronRight, FilePen, Inbox, Search, Settings as SettingsIcon } from 'lucide-react'
import { shortDate } from './format'
import { ThemeContext, accountColor, useTheme } from './theme'
import { KeyEngine, keyToken, tokenize, type Binding, type Mode } from './keys'

type Screen = View | { kind: 'settings' }

const sameView = (a: Screen, b: Screen) => JSON.stringify(a) === JSON.stringify(b)

function Drafts({ onOpen }: { onOpen(d: Draft): void }) {
  const [drafts, setDrafts] = useState<Draft[]>([])
  useEffect(() => {
    const load = () => void api.listDrafts().then(setDrafts)
    load()
    return onEvent((e) => e.type === 'drafts' && load())
  }, [])
  if (!drafts.length)
    return (
      <div className="pane">
        <div className="empty">
          <FilePen className="icon" />
          <strong>No drafts</strong>
          <span>Drafts you keep, and ones agents write, show up here.</span>
        </div>
      </div>
    )
  return (
    <div className="pane">
      {drafts.map((d, i) => (
        <div key={d.id} className="row" style={{ transform: `translateY(${8 + i * 38}px)` }} onClick={() => onOpen(d)}>
          <span className="from"><span className="name">{d.to || '(no recipient)'}</span></span>
          <span className="line">
            <span className="subject">{d.subject || '(no subject)'}</span>
            <span className="snippet"> — {d.body.slice(0, 120)}</span>
          </span>
          {d.createdBy === 'agent' && <span className="badge">agent</span>}
          <span className="date">{shortDate(d.updatedAt)}</span>
        </div>
      ))}
    </div>
  )
}

function Help({ bindings, onClose }: { bindings: Binding[]; onClose(): void }) {
  const groups = new Map<string, Binding[]>()
  for (const b of bindings) if (!b.hidden) groups.set(b.group, [...(groups.get(b.group) ?? []), b])
  const reading: Binding[] = [
    { keys: ['j', 'k'], desc: 'Scroll', group: '', modes: 'all', run: () => {} },
    { keys: ['<C-d>', '<C-u>'], desc: 'Half page', group: '', modes: 'all', run: () => {} },
    { keys: ['gg', 'G'], desc: 'Top / bottom', group: '', modes: 'all', run: () => {} },
    { keys: ['e'], desc: 'Archive', group: '', modes: 'all', run: () => {} },
  ]
  groups.set('Reading', [...reading, ...(groups.get('Reading') ?? [])])
  const order = ['Move', 'Scroll', 'Select', 'Act', 'Reading', 'Go']
  return (
    <div className="overlay" onClick={onClose}>
      <div className="help-dialog" onClick={(e) => e.stopPropagation()}>
        <h2>Keyboard</h2>
        <p className="help" style={{ margin: 0 }}>
          Vim-style. Prefix a count to repeat (<kbd>5</kbd><kbd>j</kbd>, <kbd>3</kbd><kbd>J</kbd>). <kbd>esc</kbd> to close.
        </p>
        <div className="help-cols">
          {order.filter((g) => groups.has(g)).map((g) => (
            <div className="help-group" key={g}>
              <h3>{g === 'Reading' ? 'Reading a conversation' : g}</h3>
              {groups.get(g)!.map((b, i) => (
                <div className="help-item" key={i}>
                  <span className="ks">
                    {b.keys.slice(0, 3).map((k) => (
                      <kbd key={k}>{tokenize(k).map((t) => t.replace(/^<C-(.)>$/, '⌃$1').replace(/^<(.+)>$/, '$1').replace('CR', '↵').replace('Esc', 'esc')).join('')}</kbd>
                    ))}
                  </span>
                  <span className="muted">{b.desc}</span>
                </div>
              ))}
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}

export function App() {
  const [accounts, setAccounts] = useState<Account[]>([])
  const [mailboxes, setMailboxes] = useState<Mailbox[]>([])
  const [settings, setSettingsState] = useState<Settings | null>(null)
  const [screen, setScreen] = useState<Screen>({ kind: 'inbox' })
  const [openThread, setOpenThread] = useState<ThreadRow | null>(null)
  const [composer, setComposer] = useState<DraftInput | null>(null)
  const [expanded, setExpanded] = useState<Set<number>>(new Set())
  const [query, setQuery] = useState('')
  const [adding, setAdding] = useState(false)
  const [toggles, setToggles] = useState({ dark: 0, images: 0 })
  const [sidebar, setSidebar] = useState(true)
  const list = useRef<ListHandle>(null)
  const search = useRef<HTMLInputElement>(null)
  const [help, setHelp] = useState(false)
  const [visual, setVisual] = useState(false)
  const [pending, setPending] = useState('')
  const engine = useRef(new KeyEngine())
  const keyTimer = useRef<ReturnType<typeof setTimeout>>(undefined)

  const theme = useTheme(settings?.theme, settings?.accent)

  useEffect(() => {
    const loadAccounts = () => void api.listAccounts().then(setAccounts)
    const loadBoxes = () => void api.listMailboxes().then(setMailboxes)
    loadAccounts()
    loadBoxes()
    void api.getSettings().then(setSettingsState)
    return onEvent((e) => {
      if (e.type === 'accounts') loadAccounts()
      if (e.type === 'threads') loadBoxes()
    })
  }, [])

  const updateSettings = useCallback(async (patch: Partial<Settings>) => {
    setSettingsState(await api.setSettings(patch))
  }, [])

  const go = (s: Screen) => {
    setOpenThread(null)
    setComposer(null)
    setScreen((prev) => (sameView(s, prev) ? prev : s))
  }

  const open = useCallback((row: ThreadRow) => {
    setOpenThread(row)
    if (row.unread) {
      list.current?.markLocal(row.key, false)
      void api.markThread(row.accountId, row.threadId, true)
    }
  }, [])

  const archive = (row: ThreadRow | undefined) => {
    if (!row) return
    list.current?.removeLocal(row.key)
    void api.archiveThread(row.accountId, row.threadId).catch((e) => alert(e.message))
  }

  const reply = async (messageId: number, all: boolean) => setComposer(await api.replyDraft(messageId, all))

  const replyToOpen = async (all: boolean) => {
    if (!openThread) return
    const t = await api.getThread(openThread.accountId, openThread.threadId)
    const last = t.messages.at(-1)
    if (last) await reply(last.id, all)
  }

  const compose = () => {
    if (!accounts.length) return go({ kind: 'settings' })
    setComposer({ accountId: openThread?.accountId ?? accounts[0].id, to: '', cc: '', subject: '', body: '', replyTo: null })
  }

  const isList = screen.kind === 'inbox' || screen.kind === 'mailbox' || screen.kind === 'search'

  // ---------- vim keys ----------

  /** The scrollable pane currently on screen (thread, settings or drafts). */
  const activePane = () =>
    [...document.querySelectorAll<HTMLElement>('.main > .pane')].find((el) => el.offsetParent !== null)
  const scroll = (dy: number) => activePane()?.scrollBy({ top: dy })
  const pageH = () => activePane()?.clientHeight ?? 600
  const n = (c: number | null) => c ?? 1

  const forTargets = (fn: (r: ThreadRow) => void) => {
    if (openThread) return fn(openThread)
    list.current?.targets().forEach(fn)
    list.current?.clearSelection()
  }
  const markRows = (unread: boolean) =>
    forTargets((r) => {
      list.current?.markLocal(r.key, unread)
      void api.markThread(r.accountId, r.threadId, !unread)
    })
  const archiveRows = () => {
    forTargets(archive)
    setOpenThread(null)
  }
  const stepThread = (d: number) => {
    const next = list.current?.move(d)
    if (next && next.key !== openThread?.key) open(next)
  }

  const L: Mode[] = ['list']
  const T: Mode[] = ['thread']
  const LT: Mode[] = ['list', 'thread']
  const P: Mode[] = ['page']
  const bindings: Binding[] = [
    // move
    { keys: ['j', '<Down>'], modes: L, group: 'Move', desc: 'Down (count: 5j)', run: (c) => list.current?.move(n(c)) },
    { keys: ['k', '<Up>'], modes: L, group: 'Move', desc: 'Up', run: (c) => list.current?.move(-n(c)) },
    { keys: ['gg', '<Home>'], modes: L, group: 'Move', desc: 'First (or line N)', run: (c) => list.current?.moveTo(c ? c - 1 : 0) },
    { keys: ['G', '<End>'], modes: L, group: 'Move', desc: 'Last (or line N)', run: (c) => list.current?.moveTo(c ? c - 1 : -1) },
    { keys: ['H'], modes: L, group: 'Move', desc: 'Top of screen', run: () => list.current?.screen('top') },
    { keys: ['M'], modes: L, group: 'Move', desc: 'Middle of screen', run: () => list.current?.screen('middle') },
    { keys: ['L'], modes: L, group: 'Move', desc: 'Bottom of screen', run: () => list.current?.screen('bottom') },
    { keys: ['<C-d>'], modes: L, group: 'Scroll', desc: 'Half page down', run: (c) => list.current?.page(0.5 * n(c)) },
    { keys: ['<C-u>'], modes: L, group: 'Scroll', desc: 'Half page up', run: (c) => list.current?.page(-0.5 * n(c)) },
    { keys: ['<C-f>', '<PageDown>'], modes: L, group: 'Scroll', desc: 'Page down', run: (c) => list.current?.page(n(c)) },
    { keys: ['<C-b>', '<PageUp>'], modes: L, group: 'Scroll', desc: 'Page up', run: (c) => list.current?.page(-n(c)) },
    { keys: ['zz'], modes: L, group: 'Scroll', desc: 'Centre cursor', run: () => list.current?.align('center') },
    { keys: ['zt'], modes: L, group: 'Scroll', desc: 'Cursor to top', run: () => list.current?.align('start') },
    { keys: ['zb'], modes: L, group: 'Scroll', desc: 'Cursor to bottom', run: () => list.current?.align('end') },
    // select
    { keys: ['x'], modes: L, group: 'Select', desc: 'Toggle selection', run: () => list.current?.toggleSelect() },
    { keys: ['V', 'v'], modes: L, group: 'Select', desc: 'Visual mode (select a range)', run: () => list.current?.visual() },
    // act
    { keys: ['<CR>', 'o', 'l'], modes: L, group: 'Act', desc: 'Open', run: () => { const r = list.current?.current(); if (r) open(r) } },
    { keys: ['e', 'dd'], modes: L, group: 'Act', desc: 'Archive (selection)', run: archiveRows },
    { keys: ['e'], modes: T, group: 'Act', desc: 'Archive', run: archiveRows, hidden: true },
    { keys: ['U'], modes: LT, group: 'Act', desc: 'Mark unread', run: () => { markRows(true); setOpenThread(null) } },
    { keys: ['I'], modes: L, group: 'Act', desc: 'Mark read', run: () => markRows(false) },
    { keys: ['r'], modes: T, group: 'Act', desc: 'Reply', run: () => void replyToOpen(false) },
    { keys: ['a'], modes: T, group: 'Act', desc: 'Reply all', run: () => void replyToOpen(true) },
    { keys: ['c'], modes: 'all', group: 'Act', desc: 'Compose', run: compose },
    // reading
    { keys: ['j', '<Down>'], modes: T, group: 'Reading', desc: 'Scroll down', run: (c) => scroll(60 * n(c)), hidden: true },
    { keys: ['k', '<Up>'], modes: T, group: 'Reading', desc: 'Scroll up', run: (c) => scroll(-60 * n(c)), hidden: true },
    { keys: ['<C-d>'], modes: [...T, ...P], group: 'Reading', desc: 'Half page down', run: (c) => scroll((pageH() / 2) * n(c)), hidden: true },
    { keys: ['<C-u>'], modes: [...T, ...P], group: 'Reading', desc: 'Half page up', run: (c) => scroll((-pageH() / 2) * n(c)), hidden: true },
    { keys: ['<C-f>', '<Space>', '<PageDown>'], modes: [...T, ...P], group: 'Reading', desc: 'Page down', run: (c) => scroll(pageH() * 0.9 * n(c)), hidden: true },
    { keys: ['<C-b>', '<PageUp>'], modes: [...T, ...P], group: 'Reading', desc: 'Page up', run: (c) => scroll(-pageH() * 0.9 * n(c)), hidden: true },
    { keys: ['gg'], modes: [...T, ...P], group: 'Reading', desc: 'Top', run: () => activePane()?.scrollTo({ top: 0 }), hidden: true },
    { keys: ['G'], modes: [...T, ...P], group: 'Reading', desc: 'Bottom', run: () => activePane()?.scrollTo({ top: 1e9 }), hidden: true },
    { keys: ['j'], modes: P, group: 'Reading', desc: 'Scroll down', run: (c) => scroll(60 * n(c)), hidden: true },
    { keys: ['k'], modes: P, group: 'Reading', desc: 'Scroll up', run: (c) => scroll(-60 * n(c)), hidden: true },
    { keys: ['J'], modes: T, group: 'Reading', desc: 'Next conversation', run: (c) => stepThread(n(c)) },
    { keys: ['K'], modes: T, group: 'Reading', desc: 'Previous conversation', run: (c) => stepThread(-n(c)) },
    { keys: ['d'], modes: T, group: 'Reading', desc: 'Toggle original colours', run: () => setToggles((x) => ({ ...x, dark: x.dark + 1 })) },
    { keys: ['i'], modes: T, group: 'Reading', desc: 'Load images', run: () => setToggles((x) => ({ ...x, images: x.images + 1 })) },
    { keys: ['<Esc>', 'q', 'h', 'u'], modes: T, group: 'Reading', desc: 'Back to list', run: () => setOpenThread(null) },
    // go
    { keys: ['gi'], modes: 'all', group: 'Go', desc: 'Inbox', run: () => go({ kind: 'inbox' }) },
    { keys: ['gd'], modes: 'all', group: 'Go', desc: 'Drafts', run: () => go({ kind: 'drafts' }) },
    { keys: ['gs'], modes: 'all', group: 'Go', desc: 'Settings', run: () => go({ kind: 'settings' }) },
    { keys: ['/'], modes: 'all', group: 'Go', desc: 'Search', run: () => { search.current?.focus(); search.current?.select() } },
    { keys: ['\\'], modes: 'all', group: 'Go', desc: 'Toggle sidebar', run: () => setSidebar((x) => !x) },
    { keys: ['?'], modes: 'all', group: 'Go', desc: 'All keys', run: () => setHelp(true) },
    {
      keys: ['<Esc>'], modes: L, group: 'Select', desc: 'Clear selection / back to inbox',
      run: () => { if (!list.current?.clearSelection() && screen.kind !== 'inbox') go({ kind: 'inbox' }) },
    },
    { keys: ['<Esc>', 'q'], modes: P, group: 'Go', desc: 'Back to inbox', run: () => go({ kind: 'inbox' }), hidden: true },
  ]

  const live = useRef({ bindings, mode: 'list' as Mode, composer: false, help: false, searchKind: screen.kind })
  live.current = {
    bindings,
    mode: openThread ? 'thread' : isList ? 'list' : 'page',
    composer: !!composer,
    help,
    searchKind: screen.kind,
  }

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const s = live.current
      const t = e.target
      if (s.help) {
        if (e.key === 'Escape' || e.key === '?' || e.key === 'q') setHelp(false)
        return
      }
      if (s.composer || (t instanceof Element && t.closest('input, textarea, select, [contenteditable]'))) {
        if (e.key === 'Escape' && t === search.current) {
          search.current?.blur()
          if (s.searchKind === 'search') {
            setQuery('')
            go({ kind: 'inbox' })
          }
        }
        return
      }
      const tok = keyToken(e)
      if (!tok) return
      clearTimeout(keyTimer.current)
      const r = engine.current.feed(tok, s.bindings, s.mode)
      if (r.kind === 'run') {
        e.preventDefault()
        setPending('')
        r.binding.run(r.count)
      } else if (r.kind === 'pending') {
        e.preventDefault()
        setPending(r.display)
        keyTimer.current = setTimeout(() => {
          const f = engine.current.flush()
          setPending('')
          if (f.kind === 'run') f.binding.run(f.count)
        }, 1000)
      } else {
        setPending('')
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  if (!settings) return null

  const title =
    screen.kind === 'inbox'
      ? 'Inbox'
      : screen.kind === 'drafts'
        ? 'Drafts'
        : screen.kind === 'settings'
          ? 'Settings'
          : screen.kind === 'search'
            ? `Search: ${screen.query}`
            : (mailboxes.find((m) => m.id === screen.mailboxId)?.name ?? '')

  return (
    <ThemeContext.Provider value={theme}>
    <div className="app">
      {sidebar && (
        <nav className="sidebar">
          <div className="brand">
            <span className="mark">y</span> yamc
          </div>
          <button className={`nav${screen.kind === 'inbox' ? ' on' : ''}`} onClick={() => go({ kind: 'inbox' })}>
            <Inbox className="icon" />
            <span className="label">Inbox</span>
          </button>
          <button className={`nav${screen.kind === 'drafts' ? ' on' : ''}`} onClick={() => go({ kind: 'drafts' })}>
            <FilePen className="icon" />
            <span className="label">Drafts</span>
          </button>
          {accounts.length > 0 && <div className="side-head">Accounts</div>}
          {accounts.map((a, ai) => (
            <div key={a.id}>
              <button
                className="nav"
                onClick={() => setExpanded((s) => {
                  const n = new Set(s)
                  if (n.has(a.id)) n.delete(a.id)
                  else n.add(a.id)
                  return n
                })}
                title={a.error ?? a.state}
              >
                <span style={{ width: 16, display: 'grid', placeItems: 'center' }}>
                  <span
                    className={`dot${a.state === 'syncing' ? ' syncing' : ''}`}
                    style={{ background: a.state === 'error' || a.state === 'offline' ? 'var(--faint)' : accountColor(theme, ai) }}
                  />
                </span>
                <span className="label">{a.name || a.email}</span>
                <ChevronRight className={`icon chev${expanded.has(a.id) ? ' open' : ''}`} />
              </button>
              {expanded.has(a.id) &&
                mailboxes
                  .filter((m) => m.accountId === a.id)
                  .map((m) => (
                    <button
                      key={m.id}
                      className={`nav sub${screen.kind === 'mailbox' && screen.mailboxId === m.id ? ' on' : ''}`}
                      style={{ paddingLeft: 32 + m.depth * 12 }}
                      onClick={() => go({ kind: 'mailbox', mailboxId: m.id })}
                    >
                      <span className="label">{m.name}</span>
                    </button>
                  ))}
              {a.state === 'error' && a.error && expanded.has(a.id) && <div className="err">{a.error}</div>}
            </div>
          ))}
          <div className="side-foot">
            <button className={`nav${screen.kind === 'settings' ? ' on' : ''}`} onClick={() => go({ kind: 'settings' })}>
              <SettingsIcon className="icon" />
              <span className="label">Settings</span>
            </button>
          </div>
        </nav>
      )}
      <main className="main">
        <header className="topbar">
          {openThread ? (
            <button className="btn ghost sm" style={{ marginLeft: -8 }} onClick={() => setOpenThread(null)}>← {title}</button>
          ) : (
            <span className="title">{title}</span>
          )}
          <div className="searchbox">
          <Search className="icon" />
          <input
            ref={search}
            className="input"
            placeholder="Search mail"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && query.trim()) {
                go({ kind: 'search', query: query.trim() })
                search.current?.blur()
              }
            }}
          />
          <kbd>/</kbd>
          </div>
        </header>
        {isList && (
          <ThreadList ref={list} view={screen as View} accounts={accounts} hidden={!!openThread} onOpen={open} onVisual={setVisual} />
        )}
        {openThread && (
          <div className="pane">
            <ThreadView
              key={openThread.key}
              accountId={openThread.accountId}
              threadId={openThread.threadId}
              settings={settings}
              toggles={toggles}
              onReply={reply}
              onSettings={updateSettings}
            />
          </div>
        )}
        {screen.kind === 'drafts' && !openThread && <Drafts onOpen={(d) => setComposer(d)} />}
        {screen.kind === 'settings' && (
          <SettingsView accounts={accounts} settings={settings} onSettings={updateSettings} adding={adding || !accounts.length} setAdding={setAdding} />
        )}
        {composer && <Composer draft={composer} accounts={accounts} onClose={() => setComposer(null)} />}
        {(pending || (visual && !openThread)) && (
          <div className="statusline">
            {visual && !openThread && <span className="mode">VISUAL</span>}
            {pending && <span className="keys">{pending}</span>}
          </div>
        )}
      </main>
      {help && <Help bindings={bindings} onClose={() => setHelp(false)} />}
    </div>
    </ThemeContext.Provider>
  )
}
