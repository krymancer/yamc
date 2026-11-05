import type { AccountConfig, AccountKind } from './types'

export interface Preset {
  kind: AccountKind
  label: string
  help: string
  /** Not usable yet (needs OAuth). */
  disabled?: boolean
  config: (email: string) => AccountConfig
}

export const PRESETS: Preset[] = [
  {
    kind: 'gmail',
    label: 'Gmail',
    help: 'Use an app password (Google Account → Security → 2-Step Verification → App passwords).',
    config: (email) => ({
      imap: { host: 'imap.gmail.com', port: 993, tls: 'tls' },
      smtp: { host: 'smtp.gmail.com', port: 465, tls: 'tls' },
      username: email,
      serverSavesSent: true,
    }),
  },
  {
    kind: 'proton',
    label: 'Proton (Bridge)',
    help: 'Proton Mail Bridge must be running. Use the username and password Bridge shows, not your Proton password.',
    config: (email) => ({
      imap: { host: '127.0.0.1', port: 1143, tls: 'starttls' },
      smtp: { host: '127.0.0.1', port: 1025, tls: 'starttls' },
      username: email,
      allowSelfSigned: true,
      serverSavesSent: true,
    }),
  },
  {
    kind: 'outlook',
    label: 'Outlook',
    help: 'Outlook requires OAuth sign-in, which lands in milestone 2.',
    disabled: true,
    config: (email) => ({
      imap: { host: 'outlook.office365.com', port: 993, tls: 'tls' },
      smtp: { host: 'smtp.office365.com', port: 587, tls: 'starttls' },
      username: email,
      serverSavesSent: true,
    }),
  },
  {
    kind: 'imap',
    label: 'Other (IMAP)',
    help: 'Any IMAP/SMTP server.',
    config: (email) => {
      const domain = email.split('@')[1] ?? ''
      return {
        imap: { host: domain ? `imap.${domain}` : '', port: 993, tls: 'tls' },
        smtp: { host: domain ? `smtp.${domain}` : '', port: 465, tls: 'tls' },
        username: email,
      }
    },
  },
]

export const ACCOUNT_COLORS = ['#22d3ee', '#a78bfa', '#f472b6', '#facc15', '#4ade80', '#fb923c']
