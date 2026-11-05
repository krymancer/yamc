import { useEffect, useRef, useState } from 'react'
import type { Account, DraftInput } from '@shared/types'
import { Send, X } from 'lucide-react'
import { api } from './api'

interface Props {
  draft: DraftInput
  accounts: Account[]
  onClose(): void
}

export function Composer({ draft: initial, accounts, onClose }: Props) {
  const [d, setD] = useState<DraftInput>(initial)
  const [error, setError] = useState('')
  const [sending, setSending] = useState(false)
  const body = useRef<HTMLTextAreaElement>(null)
  const to = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (d.to) {
      body.current?.focus()
      body.current?.setSelectionRange(0, 0)
    } else to.current?.focus()
  }, [])

  const set = (k: keyof DraftInput) => (e: { target: { value: string } }) =>
    setD((x) => ({ ...x, [k]: k === 'accountId' ? Number(e.target.value) : e.target.value }))

  const send = async () => {
    setSending(true)
    setError('')
    try {
      await api.send(d)
      onClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      setSending(false)
    }
  }

  // Closing keeps anything typed as a draft.
  const close = async () => {
    const touched = d.body.trim() || d.subject.trim() || d.to.trim()
    if (touched) await api.saveDraft(d)
    onClose()
  }

  const discard = async () => {
    if (d.id) await api.deleteDraft(d.id)
    onClose()
  }

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault()
      void send()
    } else if (e.key === 'Escape') {
      e.preventDefault()
      void close()
    }
  }

  return (
    <div className="composer" onKeyDown={onKey}>
      <div className="head">
        <span className="title">{d.replyTo ? 'Reply' : 'New message'}</span>
        {d.createdBy === 'agent' && <span className="badge">drafted by agent</span>}
        <span style={{ flex: 1 }} />
        <button className="btn ghost sm" onClick={close} title="Keep as draft (esc)"><X className="icon" /></button>
      </div>
      <div className="fields">
        <label className="cline">
          <span>From</span>
          <select value={d.accountId} onChange={set('accountId')}>
            {accounts.map((a) => (
              <option key={a.id} value={a.id}>
                {a.name} &lt;{a.email}&gt;
              </option>
            ))}
          </select>
        </label>
        <label className="cline">
          <span>To</span>
          <input ref={to} value={d.to} onChange={set('to')} />
        </label>
        <label className="cline">
          <span>Cc</span>
          <input value={d.cc} onChange={set('cc')} />
        </label>
        <label className="cline">
          <span>Subject</span>
          <input value={d.subject} onChange={set('subject')} />
        </label>
      </div>
      <textarea ref={body} value={d.body} onChange={set('body')} />
      <div className="actions">
        <button className="btn primary" disabled={sending} onClick={send}>
          <Send className="icon" /> {sending ? 'Sending…' : 'Send'}
        </button>
        <span className="muted" style={{ fontSize: 12.5 }}><kbd>ctrl</kbd> <kbd>enter</kbd></span>
        <span style={{ marginLeft: 'auto' }} />
        <button className="btn ghost danger" onClick={discard}>
          Discard
        </button>
      </div>
      {error && <div className="error" style={{ padding: '0 16px 12px' }}>{error}</div>}
    </div>
  )
}
