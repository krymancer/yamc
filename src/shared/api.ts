import type { Account, AccountInput, Draft, DraftInput, Mailbox, Message, Settings, Thread, ThreadRow, View } from './types'

export interface McpInfo {
  enabled: boolean
  running: boolean
  error?: string
  url: string
  token: string
  claudeCommand: string
  codexConfig: string
}

/** Methods the renderer can call in the main process. Same names as Core where possible. */
export interface YamcApi {
  listAccounts(): Account[]
  testAccount(input: AccountInput): void
  addAccount(input: AccountInput): number
  removeAccount(id: number): void
  listMailboxes(): Mailbox[]
  listThreads(view: View, offset: number, limit: number): ThreadRow[]
  getThread(accountId: number, threadId: string): Thread
  loadMessage(id: number): Message | undefined
  markThread(accountId: number, threadId: string, seen: boolean): void
  archiveThread(accountId: number, threadId: string): void
  listDrafts(): Draft[]
  getDraft(id: number): Draft | undefined
  saveDraft(d: DraftInput): number
  deleteDraft(id: number): void
  replyDraft(messageId: number, all: boolean): DraftInput
  send(d: DraftInput): void
  getSettings(): Settings
  setSettings(patch: Partial<Settings>): Settings
  openExternal(url: string): void
  mcpInfo(): McpInfo
}

export const API_METHODS = [
  'listAccounts', 'testAccount', 'addAccount', 'removeAccount', 'listMailboxes', 'listThreads', 'getThread',
  'loadMessage', 'markThread', 'archiveThread', 'listDrafts', 'getDraft', 'saveDraft', 'deleteDraft', 'replyDraft',
  'send', 'getSettings', 'setSettings', 'openExternal', 'mcpInfo',
] as const satisfies readonly (keyof YamcApi)[]
