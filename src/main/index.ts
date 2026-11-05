import { app, BrowserWindow, ipcMain, nativeTheme, safeStorage, shell } from 'electron'
import { getTheme } from '@shared/themes'
import { join } from 'node:path'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { createCipheriv, createDecipheriv, randomBytes } from 'node:crypto'
import type { Server } from 'node:http'
import { Core } from '../core/core'
import { startMcpHttp } from '../core/mcp'
import { API_METHODS, type McpInfo, type YamcApi } from '@shared/api'
import type { Settings } from '@shared/types'

const dataDir = process.env.YAMC_DATA_DIR ?? join(app.getPath('userData'), 'data')

let core: Core
let win: BrowserWindow | null = null
let mcp: { server: Server | null; error?: string; port: number; token: string } = { server: null, port: 0, token: '' }

/**
 * Passwords are encrypted with the OS keyring (safeStorage). If there is no usable keyring
 * (e.g. a Linux session without libsecret/KWallet), fall back to AES-GCM with a key in a 0600 file.
 * The first byte of the blob says which was used.
 */
function fileKey(): Buffer {
  const path = join(dataDir, 'secret.key')
  if (!existsSync(path)) {
    mkdirSync(dataDir, { recursive: true })
    writeFileSync(path, randomBytes(32), { mode: 0o600 })
  }
  return readFileSync(path)
}

const secrets = {
  encrypt(s: string): Buffer {
    if (safeStorage.isEncryptionAvailable()) return Buffer.concat([Buffer.from('S'), safeStorage.encryptString(s)])
    const iv = randomBytes(12)
    const c = createCipheriv('aes-256-gcm', fileKey(), iv)
    const body = Buffer.concat([c.update(s, 'utf8'), c.final()])
    return Buffer.concat([Buffer.from('F'), iv, c.getAuthTag(), body])
  },
  decrypt(b: Buffer): string {
    if (b[0] === 0x53) return safeStorage.decryptString(b.subarray(1))
    const d = createDecipheriv('aes-256-gcm', fileKey(), b.subarray(1, 13))
    d.setAuthTag(b.subarray(13, 29))
    return Buffer.concat([d.update(b.subarray(29)), d.final()]).toString('utf8')
  },
}

async function applyMcp(s: Settings) {
  if (mcp.server && (!s.mcpEnabled || mcp.port !== s.mcpPort || mcp.token !== s.mcpToken)) {
    await new Promise((r) => mcp.server!.close(r))
    mcp.server = null
  }
  mcp.port = s.mcpPort
  mcp.token = s.mcpToken
  mcp.error = undefined
  if (s.mcpEnabled && !mcp.server) {
    try {
      mcp.server = await startMcpHttp(core, s.mcpPort, s.mcpToken)
    } catch (err) {
      mcp.error = err instanceof Error ? err.message : String(err)
    }
  }
}

function mcpInfo(): McpInfo {
  const s = core.getSettings()
  const url = `http://127.0.0.1:${s.mcpPort}/mcp`
  return {
    enabled: s.mcpEnabled,
    running: !!mcp.server,
    error: mcp.error,
    url,
    token: s.mcpToken,
    claudeCommand: `claude mcp add --transport http yamc ${url} --header "Authorization: Bearer ${s.mcpToken}"`,
    codexConfig: `[mcp_servers.yamc]\nurl = "${url}"\nhttp_headers = { Authorization = "Bearer ${s.mcpToken}" }`,
  }
}

function openExternal(url: string) {
  if (/^(https?|mailto):/i.test(url)) void shell.openExternal(url)
}

const api: { [K in keyof YamcApi]: (...args: Parameters<YamcApi[K]>) => unknown } = {
  listAccounts: () => core.listAccounts(),
  testAccount: (i) => core.testAccount(i),
  addAccount: (i) => core.addAccount(i),
  removeAccount: (id) => core.removeAccount(id),
  listMailboxes: () => core.listMailboxes(),
  listThreads: (v, o, l) => core.listThreads(v, o, l),
  getThread: (a, t) => core.getThread(a, t),
  loadMessage: (id) => core.loadMessage(id),
  markThread: (a, t, s) => core.markThread(a, t, s),
  archiveThread: (a, t) => core.archiveThread(a, t),
  listDrafts: () => core.listDrafts(),
  getDraft: (id) => core.getDraft(id),
  saveDraft: (d) => core.saveDraft(d),
  deleteDraft: (id) => core.deleteDraft(id),
  replyDraft: (id, all) => core.replyDraft(id, all),
  send: (d) => core.send(d),
  getSettings: () => core.getSettings(),
  setSettings: async (p) => {
    const s = core.setSettings(p)
    await applyMcp(s)
    win?.setBackgroundColor(getTheme(s.theme, s.accent, nativeTheme.shouldUseDarkColors).vars.bg)
    return s
  },
  openExternal: (u) => openExternal(u),
  mcpInfo: () => mcpInfo(),
}

function createWindow() {
  win = new BrowserWindow({
    width: 1200,
    height: 800,
    minWidth: 640,
    minHeight: 400,
    show: false,
    // Paint dark from the first frame: no white flash on startup.
    backgroundColor: (() => {
      const s = core.getSettings()
      return getTheme(s.theme, s.accent, nativeTheme.shouldUseDarkColors).vars.bg
    })(),
    autoHideMenuBar: true,
    title: 'yamc',
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      sandbox: true,
      contextIsolation: true,
      nodeIntegration: false,
      spellcheck: true,
    },
  })
  win.once('ready-to-show', () => win?.show())
  win.webContents.setWindowOpenHandler(({ url }) => {
    openExternal(url)
    return { action: 'deny' }
  })
  win.webContents.on('will-navigate', (e, url) => {
    if (url !== win?.webContents.getURL()) {
      e.preventDefault()
      openExternal(url)
    }
  })
  if (process.env.ELECTRON_RENDERER_URL) void win.loadURL(process.env.ELECTRON_RENDERER_URL)
  else void win.loadFile(join(__dirname, '../renderer/index.html'))
  win.on('closed', () => (win = null))
}

app.whenReady().then(async () => {
  core = new Core({ dataDir, secrets })
  core.on((e) => win?.webContents.send('yamc:event', e))
  ipcMain.handle('yamc:call', async (_e, method: string, ...args: unknown[]) => {
    if (!(API_METHODS as readonly string[]).includes(method)) throw new Error(`Unknown method ${method}`)
    return (api as Record<string, (...a: unknown[]) => unknown>)[method](...args)
  })
  createWindow()
  core.start()
  await applyMcp(core.getSettings())
  app.on('activate', () => BrowserWindow.getAllWindows().length === 0 && createWindow())
})

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') app.quit()
})

let quitting = false
app.on('before-quit', (e) => {
  if (quitting || !core) return
  e.preventDefault()
  quitting = true
  mcp.server?.close()
  void core.stop().finally(() => app.quit())
})
