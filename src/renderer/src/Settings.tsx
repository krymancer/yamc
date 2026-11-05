import { useEffect, useState } from 'react'
import { Check, Copy as CopyIcon, Plus, Trash2 } from 'lucide-react'
import type { McpInfo } from '@shared/api'
import type { Account, AccountKind, AccountInput, Settings, TlsMode } from '@shared/types'
import { PRESETS } from '@shared/presets'
import { CAT_ACCENTS, THEME_IDS, catSwatch, getTheme, type ThemeSetting } from '@shared/themes'
import { api } from './api'
import { accountColor, useThemeTokens } from './theme'

function AddAccount({ onDone }: { onDone(): void }) {
  const [kind, setKind] = useState<AccountKind>('gmail')
  const [email, setEmail] = useState('')
  const [name, setName] = useState('')
  const [password, setPassword] = useState('')
  const [advanced, setAdvanced] = useState(false)
  const preset = PRESETS.find((p) => p.kind === kind)!
  const [config, setConfig] = useState(preset.config(''))
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')

  useEffect(() => setConfig(preset.config(email)), [kind, email])

  const server = (which: 'imap' | 'smtp') => (
    <div className="grid3">
      <input className="input" placeholder="host" value={config[which].host}
        onChange={(e) => setConfig({ ...config, [which]: { ...config[which], host: e.target.value } })} />
      <input className="input" type="number" value={config[which].port}
        onChange={(e) => setConfig({ ...config, [which]: { ...config[which], port: Number(e.target.value) } })} />
      <select className="select" value={config[which].tls}
        onChange={(e) => setConfig({ ...config, [which]: { ...config[which], tls: e.target.value as TlsMode } })}>
        <option value="tls">TLS</option>
        <option value="starttls">STARTTLS</option>
        <option value="none">None</option>
      </select>
    </div>
  )

  const save = async () => {
    setBusy(true)
    setError('')
    const input: AccountInput = { name: name || email.split('@')[0], email, kind, config, password }
    try {
      await api.addAccount(input)
      onDone()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="card">
      <div className="card-body">
        <div className="segmented" style={{ marginBottom: 12 }}>
          {PRESETS.map((p) => (
            <button key={p.kind} className={p.kind === kind ? 'on' : ''} disabled={p.disabled} onClick={() => setKind(p.kind)}>
              {p.label}
            </button>
          ))}
        </div>
        <p className="help">{preset.help}</p>
        <label className="field">
          <span>Email</span>
          <input className="input" value={email} placeholder="you@example.com" onChange={(e) => setEmail(e.target.value)} autoFocus />
        </label>
        <label className="field">
          <span>Display name</span>
          <input className="input" value={name} placeholder="Your name" onChange={(e) => setName(e.target.value)} />
        </label>
        <label className="field">
          <span>{kind === 'gmail' ? 'App password' : kind === 'proton' ? 'Bridge password' : 'Password'}</span>
          <input className="input" type="password" value={password} onChange={(e) => setPassword(e.target.value)} />
        </label>
        {advanced || kind === 'imap' ? (
          <>
            <label className="field">
              <span>Username</span>
              <input className="input" value={config.username} onChange={(e) => setConfig({ ...config, username: e.target.value })} />
            </label>
            <div className="field"><span>Incoming (IMAP)</span>{server('imap')}</div>
            <div className="field"><span>Outgoing (SMTP)</span>{server('smtp')}</div>
          </>
        ) : (
          <p className="help">
            <button className="btn ghost sm" style={{ marginLeft: -8 }} onClick={() => setAdvanced(true)}>Server settings…</button>
          </p>
        )}
        {error && <div className="error">{error}</div>}
        <div className="row-flex">
          <button className="btn primary" disabled={busy || !email || !password} onClick={save}>
            {busy ? 'Connecting…' : 'Add account'}
          </button>
          <button className="btn" onClick={onDone}>Cancel</button>
        </div>
      </div>
    </div>
  )
}

function Copy({ text }: { text: string }) {
  const [done, setDone] = useState(false)
  return (
    <button className="btn ghost sm" onClick={() => {
      void navigator.clipboard.writeText(text)
      setDone(true)
      setTimeout(() => setDone(false), 1200)
    }}>
      {done ? <Check className="icon" /> : <CopyIcon className="icon" />}
      {done ? 'Copied' : 'Copy'}
    </button>
  )
}

function ThemePicker({ settings, onSettings }: { settings: Settings; onSettings(p: Partial<Settings>): void }) {
  const current = useThemeTokens()
  const options: ThemeSetting[] = [...THEME_IDS, 'system']
  const isCat = ['latte', 'frappe', 'macchiato', 'mocha'].includes(settings.theme)
  return (
    <>
      <div className="themes">
        {options.map((id) => {
          const t = getTheme(id, settings.accent, true)
          const v = t.vars
          return (
            <button key={id} className={`theme-card${settings.theme === id ? ' on' : ''}`} onClick={() => onSettings({ theme: id })}>
              <div className="preview" style={{ background: v.bg }}>
                <div className="side" style={{ background: v.panel, border: `1px solid ${v.line}` }} />
                <div className="lines">
                  <i style={{ background: v.strong, width: '70%' }} />
                  <i style={{ background: v.muted, width: '90%', opacity: 0.6 }} />
                  <i style={{ background: v.muted, width: '55%', opacity: 0.6 }} />
                  <i style={{ background: v.accent, width: '30%' }} />
                </div>
              </div>
              <div className="name">{id === 'system' ? 'System' : t.label}</div>
            </button>
          )
        })}
      </div>
      {isCat && (
        <div className="accents">
          {CAT_ACCENTS.map((a) => (
            <button
              key={a}
              title={a}
              className={`swatch${settings.accent === a ? ' on' : ''}`}
              style={{ background: catSwatch(current.id, a) ?? undefined }}
              onClick={() => onSettings({ accent: a })}
            />
          ))}
        </div>
      )}
    </>
  )
}

export function SettingsView({ accounts, settings, onSettings, adding, setAdding }: {
  accounts: Account[]
  settings: Settings
  onSettings(patch: Partial<Settings>): Promise<void>
  adding: boolean
  setAdding(v: boolean): void
}) {
  const theme = useThemeTokens()
  const [mcp, setMcp] = useState<McpInfo | null>(null)
  useEffect(() => void api.mcpInfo().then(setMcp), [settings])

  return (
    <div className="pane">
      <div className="sheet">
        <section className="section">
          <h2 className="section-title">Accounts</h2>
          <p className="section-desc">Each account keeps its own folders. The inbox shows all of them together.</p>
          {accounts.length > 0 && (
            <div className="card" style={{ marginBottom: 12 }}>
              {accounts.map((a, i) => (
                <div className="list-item" key={a.id}>
                  <span className="dot" style={{ background: accountColor(theme, i) }} />
                  <div className="grow">
                    <div style={{ color: 'var(--strong)', fontWeight: 500 }}>{a.name}</div>
                    <div className="muted" style={{ fontSize: 13 }}>{a.email}</div>
                    {a.error && <div className="error" style={{ margin: '4px 0 0' }}>{a.error}</div>}
                  </div>
                  <span className="badge">{a.state}</span>
                  <button className="btn ghost sm danger" onClick={() => confirm(`Remove ${a.email} and its local cache?`) && api.removeAccount(a.id)}>
                    <Trash2 className="icon" />
                  </button>
                </div>
              ))}
            </div>
          )}
          {adding ? (
            <AddAccount onDone={() => setAdding(false)} />
          ) : (
            <button className="btn" onClick={() => setAdding(true)}><Plus className="icon" /> Add account</button>
          )}
        </section>

        <section className="section">
          <h2 className="section-title">Theme</h2>
          <p className="section-desc">Emails are recoloured to match. Catppuccin themes take an accent.</p>
          <ThemePicker settings={settings} onSettings={onSettings} />
        </section>

        <section className="section">
          <h2 className="section-title">Offline</h2>
          <p className="section-desc">Full messages from the last N days are downloaded ahead of time. Older ones load when opened.</p>
          <input className="input" style={{ maxWidth: 160 }} type="number" min={1} value={settings.bodyDays}
            onChange={(e) => onSettings({ bodyDays: Math.max(1, Number(e.target.value) || 90) })} />
        </section>

        <section className="section">
          <h2 className="section-title">MCP</h2>
          <p className="section-desc">Agents can read, search, draft, mark read and archive. They can't send: drafts wait for you.</p>
          <div className="card">
            <div className="setting-row">
              <div className="grow">
                <div>Let local AI agents use yamc</div>
                <div>{mcp ? (mcp.running ? mcp.url : mcp.error ? `Failed: ${mcp.error}` : 'Stopped') : ''}</div>
              </div>
              <button className={`switch${settings.mcpEnabled ? ' on' : ''}`} onClick={() => onSettings({ mcpEnabled: !settings.mcpEnabled })} />
            </div>
            {mcp && (
              <div className="setting-row" style={{ display: 'block' }}>
                <div className="row-flex" style={{ justifyContent: 'space-between' }}>
                  <span className="muted">Claude Code</span>
                  <Copy text={mcp.claudeCommand} />
                </div>
                <pre className="code">{mcp.claudeCommand}</pre>
                <div className="row-flex" style={{ justifyContent: 'space-between' }}>
                  <span className="muted">Codex · ~/.codex/config.toml</span>
                  <Copy text={mcp.codexConfig} />
                </div>
                <pre className="code">{mcp.codexConfig}</pre>
                <button className="btn sm" onClick={() => confirm('Agents using the old token will stop working. Continue?') &&
                  onSettings({ mcpToken: crypto.randomUUID().replace(/-/g, '') })}>
                  New token
                </button>
              </div>
            )}
          </div>
        </section>
      </div>
    </div>
  )
}
