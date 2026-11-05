import { useEffect, useRef, useState } from 'react'
import { Image, Palette, Paperclip, Reply, ReplyAll, UserRound } from 'lucide-react'
import type { Message, Settings, Thread } from '@shared/types'
import { api, onEvent } from './api'
import { addrList, longDate, shortDate, who } from './format'
import { MessageFrame, hasRemoteContent } from './MessageFrame'
import { useThemeTokens } from './theme'

interface Props {
  accountId: number
  threadId: string
  settings: Settings
  /** Bumped by the `d` / `i` keys to toggle the last expanded message. */
  toggles: { dark: number; images: number }
  onReply(messageId: number, all: boolean): void
  onSettings(patch: Partial<Settings>): void
}

/** Run fn when a counter changes, but not for the value it had on mount. */
function useBump(value: number, fn: () => void) {
  const first = useRef(value)
  useEffect(() => {
    if (value !== first.current) fn()
  }, [value])
}

function initials(m: Message): string {
  const s = (m.from?.name || m.from?.address || '?').replace(/[^\p{L}\p{N} ]/gu, '').trim()
  const parts = s.split(/\s+/)
  return ((parts[0]?.[0] ?? '?') + (parts.length > 1 ? (parts.at(-1)?.[0] ?? '') : '')).toUpperCase()
}

function Msg({ m, open, settings, onToggle, onReply, toggles, onSettings }: {
  m: Message
  open: boolean
  settings: Settings
  toggles: Props['toggles']
  onToggle(): void
  onReply: Props['onReply']
  onSettings: Props['onSettings']
}) {
  const theme = useThemeTokens()
  const [full, setFull] = useState<Message>(m)
  const sender = m.from?.address.toLowerCase() ?? ''
  const senderOriginal = settings.originalColorSenders.includes(sender)
  const [dark, setDark] = useState(theme.dark && !senderOriginal)
  const [images, setImages] = useState(false)

  useEffect(() => {
    if (open && !full.hasBody) void api.loadMessage(m.id).then((x) => x && setFull(x))
  }, [open, m.id])
  useEffect(() => setDark(theme.dark && !senderOriginal), [theme.dark])

  useBump(toggles.dark, () => open && setDark((d) => !d))
  useBump(toggles.images, () => open && setImages(true))

  const head = (
    <div className="msg-head" onClick={onToggle}>
      <div className="avatar">{initials(m)}</div>
      <div className="meta">
        <div className="who">
          {who(m.from)}
          {open && m.from?.name && <span className="addr">{m.from.address}</span>}
        </div>
        <div className="sub">
          {open ? (
            <>
              to {addrList(m.to) || '—'}
              {m.cc.length > 0 && <> · cc {addrList(m.cc)}</>}
            </>
          ) : (
            m.snippet
          )}
        </div>
      </div>
      <span className="when">{open ? longDate(m.date) : shortDate(m.date)}</span>
    </div>
  )

  if (!open) return <div className="msg">{head}</div>

  const remote = hasRemoteContent(full.html)
  const toggleSender = () =>
    onSettings({
      originalColorSenders: senderOriginal
        ? settings.originalColorSenders.filter((s) => s !== sender)
        : [...settings.originalColorSenders, sender],
    })

  return (
    <div className="msg">
      {head}
      <div className="msg-body">
        {full.hasBody ? (
          <MessageFrame html={full.html} text={full.text} dark={dark} images={images} />
        ) : (
          <div className="loading">Loading…</div>
        )}
      </div>
      {full.attachments.length > 0 && (
        <div className="attachments">
          {full.attachments.map((a, i) => (
            <span className="badge" key={i}>
              <Paperclip className="icon" style={{ width: 12, height: 12, marginRight: 4 }} />
              {a.filename} · {Math.ceil(a.size / 1024)} KB
            </span>
          ))}
        </div>
      )}
      <div className="msg-tools">
        <button className="btn ghost sm" onClick={() => onReply(m.id, false)}>
          <Reply className="icon" /> Reply
        </button>
        <button className="btn ghost sm" onClick={() => onReply(m.id, true)}>
          <ReplyAll className="icon" /> Reply all
        </button>
        <span style={{ flex: 1 }} />
        {remote && !images && (
          <button className="btn ghost sm" onClick={() => setImages(true)}>
            <Image className="icon" /> Load images
          </button>
        )}
        {full.html && theme.dark && (
          <button className="btn ghost sm" onClick={() => setDark((d) => !d)}>
            <Palette className="icon" /> {dark ? 'Original' : 'Recolour'}
          </button>
        )}
        {full.html && sender && theme.dark && (
          <button className="btn ghost sm" onClick={toggleSender} title="Remember for this sender">
            <UserRound className="icon" /> {senderOriginal ? 'Recolour sender' : 'Always original'}
          </button>
        )}
      </div>
    </div>
  )
}

const NO_TOGGLES = { dark: 0, images: 0 }

export function ThreadView({ accountId, threadId, settings, toggles, onReply, onSettings }: Props) {
  const [thread, setThread] = useState<Thread | null>(null)
  const [open, setOpen] = useState<Set<number>>(new Set())

  useEffect(() => {
    let alive = true
    const load = (first: boolean) =>
      api.getThread(accountId, threadId).then((t) => {
        if (!alive) return
        setThread(t)
        if (first) {
          // Latest message plus anything unread starts expanded.
          const ids = t.messages.filter((m, i) => !m.seen || i === t.messages.length - 1).map((m) => m.id)
          setOpen(new Set(ids))
        }
      })
    void load(true)
    const off = onEvent((e) => e.type === 'threads' && void load(false))
    return () => {
      alive = false
      off()
    }
  }, [accountId, threadId])

  if (!thread) return null
  const last = thread.messages.at(-1)

  return (
    <div className="thread">
      <h1>{thread.subject}</h1>
      {thread.messages.map((m) => (
        <Msg
          key={m.id}
          m={m}
          open={open.has(m.id)}
          settings={settings}
          toggles={m.id === last?.id ? toggles : NO_TOGGLES}
          onToggle={() =>
            setOpen((s) => {
              const n = new Set(s)
              if (n.has(m.id)) n.delete(m.id)
              else n.add(m.id)
              return n
            })
          }
          onReply={onReply}
          onSettings={onSettings}
        />
      ))}
      <div className="hint">
        <span><kbd>j</kbd><kbd>k</kbd> scroll</span>
        <span><kbd>J</kbd><kbd>K</kbd> next / prev</span>
        <span><kbd>r</kbd> reply</span>
        <span><kbd>e</kbd> archive</span>
        <span><kbd>d</kbd> colours</span>
        <span><kbd>q</kbd> back</span>
        <span><kbd>?</kbd> all keys</span>
      </div>
    </div>
  )
}
