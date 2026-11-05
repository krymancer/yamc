// Vim-style key engine: counts (5j), sequences (gg, dd, zz), ctrl chords (<C-d>).

export type Mode = 'list' | 'thread' | 'page'

export interface Binding {
  /** One or more alternative sequences, e.g. ['j', '<Down>'] or ['gg']. */
  keys: string[]
  modes: Mode[] | 'all'
  group: string
  desc: string
  run(count: number | null): void
  /** Hide from the help overlay (duplicate aliases). */
  hidden?: boolean
}

const NAMED: Record<string, string> = {
  Enter: 'CR',
  Escape: 'Esc',
  ArrowDown: 'Down',
  ArrowUp: 'Up',
  ArrowLeft: 'Left',
  ArrowRight: 'Right',
  ' ': 'Space',
  Tab: 'Tab',
  Backspace: 'BS',
  PageDown: 'PageDown',
  PageUp: 'PageUp',
  Home: 'Home',
  End: 'End',
}

/** Normalize a KeyboardEvent to a vim-ish token: `j`, `G`, `<C-d>`, `<CR>`. Null for bare modifiers. */
export function keyToken(e: { key: string; ctrlKey: boolean; metaKey: boolean; altKey: boolean }): string | null {
  if (['Shift', 'Control', 'Alt', 'Meta', 'CapsLock'].includes(e.key)) return null
  const base = NAMED[e.key] ?? e.key
  if (e.ctrlKey || e.metaKey) return `<C-${base.length === 1 ? base.toLowerCase() : base}>`
  if (e.altKey) return null
  return NAMED[e.key] ? `<${base}>` : base
}

/** Split a binding spec like `g<CR>` into tokens [`g`, `<CR>`]. */
export function tokenize(spec: string): string[] {
  return spec.match(/<[^>]+>|./g) ?? []
}

export type FeedResult =
  | { kind: 'run'; binding: Binding; count: number | null }
  | { kind: 'pending'; display: string }
  | { kind: 'none' }

export class KeyEngine {
  private count = ''
  private seq: string[] = []
  /** A binding that matched exactly while a longer one was still possible (`d` vs `dd`). */
  private waiting: Binding | null = null

  reset(): void {
    this.count = ''
    this.seq = []
    this.waiting = null
  }

  /** Timeout expired (vim's timeoutlen): run the shorter match, if any. */
  flush(): FeedResult {
    const b = this.waiting
    const count = this.count ? Number(this.count) : null
    this.reset()
    return b ? { kind: 'run', binding: b, count } : { kind: 'none' }
  }

  get pending(): string {
    return this.count + this.seq.join('')
  }

  feed(token: string, bindings: Binding[], mode: Mode): FeedResult {
    const active = bindings.filter((b) => b.modes === 'all' || b.modes.includes(mode))
    if (!this.seq.length && /^[0-9]$/.test(token) && (this.count || token !== '0')) {
      this.count += token
      return { kind: 'pending', display: this.pending }
    }
    this.seq.push(token)
    let exact: Binding | undefined
    let longer = false
    for (const b of active) {
      for (const spec of b.keys) {
        const t = tokenize(spec)
        if (t.length < this.seq.length || !this.seq.every((k, i) => t[i] === k)) continue
        if (t.length === this.seq.length) exact = b
        else longer = true
      }
    }
    if (exact && !longer) {
      const count = this.count ? Number(this.count) : null
      this.reset()
      return { kind: 'run', binding: exact, count }
    }
    if (longer) {
      this.waiting = exact ?? null
      return { kind: 'pending', display: this.pending }
    }
    // Unknown sequence: drop it, but let a lone key start fresh (e.g. `gx` then `j`).
    const was = this.seq.length
    this.reset()
    return was > 1 ? this.feed(token, bindings, mode) : { kind: 'none' }
  }
}
