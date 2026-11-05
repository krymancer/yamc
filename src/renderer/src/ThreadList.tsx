import { forwardRef, useCallback, useEffect, useImperativeHandle, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import type { Account, ThreadRow, View } from '@shared/types'
import { api, onEvent } from './api'
import { Inbox, SearchX } from 'lucide-react'
import { shortDate } from './format'
import { accountColor, useThemeTokens } from './theme'

const ROW = 38
const PAGE = 300

export interface ListHandle {
  move(delta: number): ThreadRow | undefined
  /** Jump to a row; negative counts from the end (-1 = last). */
  moveTo(index: number): ThreadRow | undefined
  /** Scroll by a fraction of the visible rows and move the cursor with it (<C-d>, <C-f>). */
  page(fraction: number): void
  /** H / M / L: first, middle, last visible row. */
  screen(where: 'top' | 'middle' | 'bottom'): void
  /** zt / zz / zb. */
  align(where: 'start' | 'center' | 'end'): void
  current(): ThreadRow | undefined
  /** x: toggle the row under the cursor in the selection. */
  toggleSelect(): void
  /** V: start/stop visual mode anchored at the cursor. Returns whether it's now on. */
  visual(): boolean
  /** Selected rows (visual range + x-marked), or the cursor row if nothing is selected. */
  targets(): ThreadRow[]
  clearSelection(): boolean
  markLocal(key: string, unread: boolean): void
  removeLocal(key: string): void
}

/**
 * Keep rows where they are while the view is open: updated rows keep their slot,
 * new rows go on top, rows gone from the server are dropped.
 */
export function mergeStable(prev: ThreadRow[], next: ThreadRow[], local: Map<string, boolean>): ThreadRow[] {
  const byKey = new Map(next.map((r) => [r.key, r]))
  const had = new Set(prev.map((r) => r.key))
  const newest = prev.reduce((m, r) => Math.max(m, r.date), 0)
  const fresh = next.filter((r) => !had.has(r.key))
  // Genuinely new mail goes on top; rows that just came into the fetch window go at the bottom.
  const top = fresh.filter((r) => r.date > newest)
  const bottom = fresh.filter((r) => r.date <= newest)
  const kept = prev.filter((r) => byKey.has(r.key)).map((r) => byKey.get(r.key)!)
  return [...top, ...kept, ...bottom].map((r) => (local.has(r.key) ? { ...r, unread: local.get(r.key)! } : r))
}

interface Props {
  view: View
  accounts: Account[]
  hidden: boolean
  onOpen(row: ThreadRow): void
  onVisual(on: boolean): void
}

export const ThreadList = forwardRef<ListHandle, Props>(function ThreadList({ view, accounts, hidden, onOpen, onVisual }, ref) {
  const [rows, setRows] = useState<ThreadRow[]>([])
  const [cursor, setCursor] = useState(0)
  const [marked, setMarked] = useState<Set<string>>(new Set())
  const [anchor, setAnchor] = useState<number | null>(null)
  const markedRef = useRef(marked)
  markedRef.current = marked
  const anchorRef = useRef(anchor)
  anchorRef.current = anchor
  const [loaded, setLoaded] = useState(false)
  const rowsRef = useRef(rows)
  rowsRef.current = rows
  const cursorRef = useRef(cursor)
  cursorRef.current = cursor
  // Read state we changed locally; wins over a refresh that raced ahead of the server.
  const local = useRef(new Map<string, boolean>())
  const scroller = useRef<HTMLDivElement>(null)
  const loadingMore = useRef(false)
  const exhausted = useRef(false)

  const refresh = useCallback(async () => {
    const want = Math.max(PAGE, rowsRef.current.length + 50)
    const next = await api.listThreads(view, 0, want)
    exhausted.current = next.length < want
    setRows((prev) => (prev.length ? mergeStable(prev, next, local.current) : next))
    setLoaded(true)
  }, [view])

  useEffect(() => {
    local.current.clear()
    exhausted.current = false
    setRows([])
    setCursor(0)
    setMarked(new Set())
    setAnchor(null)
    onVisual(false)
    setLoaded(false)
    scroller.current?.scrollTo(0, 0)
    void refresh()
    return onEvent((e) => e.type === 'threads' && void refresh())
  }, [refresh])

  const loadMore = useCallback(async () => {
    if (loadingMore.current || exhausted.current) return
    loadingMore.current = true
    try {
      const more = await api.listThreads(view, rowsRef.current.length, PAGE)
      exhausted.current = more.length < PAGE
      setRows((prev) => {
        const have = new Set(prev.map((r) => r.key))
        return [...prev, ...more.filter((r) => !have.has(r.key))]
      })
    } finally {
      loadingMore.current = false
    }
  }, [view])

  const virt = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scroller.current,
    estimateSize: () => ROW,
    overscan: 20,
    paddingStart: 8,
    paddingEnd: 8,
  })

  const items = virt.getVirtualItems()
  useEffect(() => {
    const last = items.at(-1)
    if (last && last.index >= rows.length - 40) void loadMore()
  }, [items, rows.length, loadMore])

  const go = (i: number) => {
    const n = rowsRef.current.length
    if (!n) return undefined
    i = Math.max(0, Math.min(n - 1, i))
    setCursor(i)
    cursorRef.current = i
    if (i === 0) scroller.current?.scrollTo({ top: 0 })
    else virt.scrollToIndex(i, { align: 'auto' })
    return rowsRef.current[i]
  }
  const visibleRows = () => Math.max(1, Math.floor((scroller.current?.clientHeight ?? ROW * 10) / ROW))
  const firstVisible = () => Math.max(0, Math.ceil(((scroller.current?.scrollTop ?? 0) - 8) / ROW))

  useImperativeHandle(ref, () => ({
    move: (delta) => go(cursorRef.current + delta),
    moveTo: (i) => go(i < 0 ? rowsRef.current.length + i : i),
    page(fraction) {
      const d = Math.trunc(visibleRows() * fraction) || Math.sign(fraction)
      if (scroller.current) scroller.current.scrollTop += d * ROW
      const n = rowsRef.current.length
      const i = Math.max(0, Math.min(n - 1, cursorRef.current + d))
      setCursor(i)
      cursorRef.current = i
    },
    screen(where) {
      const top = firstVisible()
      const last = Math.min(rowsRef.current.length - 1, top + visibleRows() - 1)
      go(where === 'top' ? top : where === 'bottom' ? last : Math.floor((top + last) / 2))
    },
    align: (where) => virt.scrollToIndex(cursorRef.current, { align: where }),
    current: () => rowsRef.current[cursorRef.current],
    toggleSelect() {
      const r = rowsRef.current[cursorRef.current]
      if (!r) return
      setMarked((m) => {
        const n = new Set(m)
        if (n.has(r.key)) n.delete(r.key)
        else n.add(r.key)
        return n
      })
    },
    visual() {
      const on = anchorRef.current === null
      setAnchor(on ? cursorRef.current : null)
      anchorRef.current = on ? cursorRef.current : null
      onVisual(on)
      return on
    },
    targets() {
      const all = rowsRef.current
      const keys = new Set(markedRef.current)
      const a = anchorRef.current
      if (a !== null) {
        const [lo, hi] = [Math.min(a, cursorRef.current), Math.max(a, cursorRef.current)]
        for (let i = lo; i <= hi; i++) if (all[i]) keys.add(all[i].key)
      }
      if (!keys.size) return all[cursorRef.current] ? [all[cursorRef.current]] : []
      return all.filter((r) => keys.has(r.key))
    },
    clearSelection() {
      const had = markedRef.current.size > 0 || anchorRef.current !== null
      setMarked(new Set())
      setAnchor(null)
      anchorRef.current = null
      onVisual(false)
      return had
    },
    markLocal(key, unread) {
      local.current.set(key, unread)
      setRows((prev) => prev.map((r) => (r.key === key ? { ...r, unread } : r)))
    },
    removeLocal(key) {
      setRows((prev) => prev.filter((r) => r.key !== key))
      setCursor((c) => Math.min(c, Math.max(0, rowsRef.current.length - 2)))
    },
  }))

  const inVisual = (i: number) => anchor !== null && i >= Math.min(anchor, cursor) && i <= Math.max(anchor, cursor)

  const theme = useThemeTokens()
  const color = (id: number) => accountColor(theme, Math.max(0, accounts.findIndex((a) => a.id === id)))
  const multi = accounts.length > 1

  return (
    <div className="pane" ref={scroller} style={{ display: hidden ? 'none' : undefined }}>
      {loaded && !rows.length && (
        <div className="empty">
          {view.kind === 'search' ? <SearchX className="icon" /> : <Inbox className="icon" />}
          <strong>{view.kind === 'search' ? 'Nothing found' : accounts.length ? 'All clear' : 'No accounts yet'}</strong>
          <span>{view.kind === 'search' ? 'Try fewer words.' : accounts.length ? 'Nothing in here.' : 'Add one in Settings (g s).'}</span>
        </div>
      )}
      <div style={{ height: virt.getTotalSize(), position: 'relative' }}>
        {items.map((v) => {
          const r = rows[v.index]
          return (
            <div
              key={r.key}
              className={`row${r.unread ? ' unread' : ''}${v.index === cursor ? ' cursor' : ''}${marked.has(r.key) || inVisual(v.index) ? ' selected' : ''}`}
              style={{ transform: `translateY(${v.start}px)` }}
              onMouseDown={() => setCursor(v.index)}
              onDoubleClick={() => onOpen(r)}
              onClick={() => onOpen(r)}
            >
              <span className="from">
                {multi && <span className="dot" style={{ background: color(r.accountId) }} />}
                <span className="name">{r.from}</span>
                {r.count > 1 && <span className="count">{r.count}</span>}
              </span>
              <span className="line">
                <span className="subject">{r.subject}</span>
                {r.snippet && <span className="snippet"> — {r.snippet}</span>}
              </span>
              <span className="date">{shortDate(r.date)}</span>
            </div>
          )
        })}
      </div>
    </div>
  )
})
