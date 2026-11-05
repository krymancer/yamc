// Types shared by core, main, preload and renderer. No runtime imports here.
import type { CatAccent, ThemeSetting } from './themes'

export type AccountKind = 'gmail' | 'proton' | 'outlook' | 'imap'
export type TlsMode = 'tls' | 'starttls' | 'none'

export interface ServerConfig {
  host: string
  port: number
  tls: TlsMode
}

export interface AccountConfig {
  imap: ServerConfig
  smtp: ServerConfig
  username: string
  /** Accept self-signed certificates. Only honoured for loopback hosts (Proton Bridge). */
  allowSelfSigned?: boolean
  /** Whether the server stores sent mail itself (Gmail, Outlook, Proton do). */
  serverSavesSent?: boolean
}

export interface AccountInput {
  name: string
  email: string
  kind: AccountKind
  config: AccountConfig
  password: string
}

export type SyncState = 'idle' | 'syncing' | 'error' | 'offline'

export interface Account {
  id: number
  name: string
  email: string
  kind: AccountKind
  color: string
  config: AccountConfig
  state: SyncState
  error?: string
}

export interface Mailbox {
  id: number
  accountId: number
  path: string
  name: string
  specialUse: string | null
  depth: number
}

export interface Address {
  name?: string
  address: string
}

export type View =
  | { kind: 'inbox' }
  | { kind: 'mailbox'; mailboxId: number }
  | { kind: 'search'; query: string }
  | { kind: 'drafts' }

export interface ThreadRow {
  /** `${accountId}:${threadId}` */
  key: string
  accountId: number
  threadId: string
  subject: string
  /** Display names of senders, latest first. */
  from: string
  snippet: string
  date: number
  unread: boolean
  count: number
}

export interface Attachment {
  filename: string
  mimeType: string
  size: number
}

export interface Message {
  id: number
  accountId: number
  mailboxId: number
  messageId: string | null
  subject: string
  from: Address | null
  to: Address[]
  cc: Address[]
  date: number
  seen: boolean
  hasBody: boolean
  html: string | null
  text: string | null
  attachments: Attachment[]
  snippet: string
  references: string[]
}

export interface Thread {
  accountId: number
  threadId: string
  subject: string
  messages: Message[]
}

export interface Draft {
  id: number
  accountId: number
  to: string
  cc: string
  subject: string
  body: string
  /** Local message id being replied to, if any. */
  replyTo: number | null
  createdBy: 'user' | 'agent'
  updatedAt: number
}

export type DraftInput = Omit<Draft, 'id' | 'updatedAt' | 'createdBy'> & {
  id?: number
  createdBy?: Draft['createdBy']
}

export interface Settings {
  theme: ThemeSetting
  /** Accent for Catppuccin themes. */
  accent: CatAccent
  bodyDays: number
  mcpEnabled: boolean
  mcpPort: number
  mcpToken: string
  /** Senders whose mail is shown with original colours. */
  originalColorSenders: string[]
}

export type CoreEvent =
  | { type: 'threads' }
  | { type: 'accounts' }
  | { type: 'drafts' }
  | { type: 'body'; messageId: number }
