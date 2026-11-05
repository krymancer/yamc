# yamc — spec (POC)

A fast, minimal, dark-by-default desktop mail client that AI agents can drive over MCP.

## Principles

1. **Speed first.** The UI reads only from a local SQLite database and never waits on the network. Every interaction (open, scroll, switch view, search) should feel instant on 100k+ messages.
2. **Out of the way.** Clean and minimal, keyboard-first. No unread counts, badges, or notifications. Ever.
3. **Dark by default, no flashbangs.** HTML email is recoloured to dark before it's shown; one key shows the original.
4. **Private by default.** Remote content is blocked until asked for. No telemetry.
5. **AI from outside.** No AI inside the app; an MCP server (on by default) lets Claude Code, Codex, etc. read and act on mail.
6. **Open source, any provider.** Built for the author's accounts first, but nothing provider-specific is hard-coded beyond presets.

## Stack

- Electron + TypeScript everywhere, built with electron-vite.
- `imapflow` (IMAP), `nodemailer` (SMTP), `postal-mime` (parsing), `better-sqlite3` + FTS5 (storage/search).
- React renderer with a virtualized list. Plain CSS variables, no UI kit.
- Hot paths move to Rust (napi-rs) only if profiling shows the need.

## Architecture

```
renderer (React)  ──IPC──▶  main process
                              ├─ core/db       SQLite (WAL) + FTS5
                              ├─ core/sync     one IMAP session per account, IDLE on INBOX
                              ├─ core/ops      outbox of local changes → server (mark read, archive)
                              ├─ core/smtp     send
                              └─ core/mcp      Streamable HTTP MCP server on 127.0.0.1
```

`src/core` has no Electron imports, so it can move into a utility process or a headless daemon later.

## Accounts

| Provider | POC auth | Notes |
|---|---|---|
| Gmail | App password (needs 2FA) | Proper OAuth later; Google expires tokens for unverified "testing" apps every 7 days. Threads via `X-GM-THRID`. |
| Proton | Proton Mail Bridge | Local IMAP `127.0.0.1:1143` / SMTP `127.0.0.1:1025`, STARTTLS with a self-signed cert (trusted for localhost only). |
| Outlook | OAuth2 (milestone 2) | Basic auth is disabled. Needs an Azure app registration (public client, PKCE, loopback redirect). |
| Generic | IMAP/SMTP password | Host/port/TLS entered manually. |

Secrets are encrypted with Electron `safeStorage` (libsecret/KWallet on Linux).

## Sync

- Mailboxes are listed per account. Sync covers everything except Spam, Trash, and (on Gmail) All Mail / Important / Starred, which only duplicate other folders.
- **Headers for all messages**, newest first in batches, so the list and search cover all history.
- **Bodies for the last 90 days** (configurable), plus anything opened, cached forever.
- INBOX is watched with IDLE; other folders re-sync every 5 minutes.
- Flags: CONDSTORE when the server has it, otherwise a full flag fetch. Deleted messages are reconciled by UID set.
- UIDVALIDITY change → that mailbox's cache is dropped and re-synced.

## Data model

- `accounts`, `mailboxes`, `messages` (one row per mailbox+UID), `bodies`, `drafts`, `ops` (pending server changes), `messages_fts`.
- Threads are per account: Gmail thread ID when available, otherwise the `References`/`In-Reply-To` chain.

## Views

- **Shared inbox (default):** conversations with a message in any account's INBOX. Unread first, then newest. Opening a conversation marks it read, but it **keeps its position until you leave the view**. New mail appears at the top.
- **Account → folder:** the same list scoped to one mailbox.
- **Search:** FTS over subject, from, to, and cached bodies, across all accounts.
- **Conversation:** Gmail-style stacked messages, latest expanded.

## Email rendering

- Sanitized with DOMPurify, rendered in a sandboxed iframe (no scripts) with a CSP that blocks remote content.
- The iframe is dark and hidden until recolouring finishes. Recolouring rewrites computed colours, like Dark Reader's dynamic mode: light backgrounds go dark, dark text goes light, hue is kept, and images are untouched.
- `d` toggles original colours for the message; "always for this sender" is remembered.
- `i` loads remote images for the message.
- Links open in the system browser.

## Keyboard

| Key | Action |
|---|---|
| `j` / `k` | next / previous |
| `Enter` / `o` | open |
| `Esc` / `u` | back |
| `e` | archive |
| `r` / `a` | reply / reply all |
| `c` | compose |
| `/` | search |
| `g i` | shared inbox |
| `d` | toggle dark recolouring |
| `i` | load images |

## MCP

Streamable HTTP at `http://127.0.0.1:<port>/mcp` with a bearer token. Settings shows the `claude mcp add` / Codex config to copy.

Tools: `list_accounts`, `list_inbox`, `search`, `get_thread`, `create_draft`, `list_drafts`, `mark_read`, `archive`.
**Agents can't send.** Drafts show in the app and you send them yourself.

## Milestones

1. **POC:** IMAP/SMTP password accounts (Gmail app password, Proton Bridge, generic), shared inbox, folders, conversation view, dark rendering, compose/reply/send, search, MCP server.
2. Outlook OAuth, then Gmail OAuth.
3. Attachments (download/attach), move to folder, per-account settings, packaging (AppImage/deb).
4. Profiling pass; possible Rust core.

## Out of scope (for now)

Calendar, contacts, PGP, filters/rules, in-app AI, notifications (never).
