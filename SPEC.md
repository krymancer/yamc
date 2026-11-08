# yamc — spec (POC)

A fast, minimal, dark-by-default desktop mail client that AI agents can drive over MCP.

## Principles

1. **Speed first.** The UI reads only from a local SQLite database and never waits on the network. Every interaction (open, scroll, switch view, search) should feel instant on 100k+ messages.
2. **Out of the way.** Clean and minimal, keyboard-first. No unread counts, badges, or notifications. Ever.
3. **Dark by default, no flashbangs.** HTML email is drawn in the app's theme; one key opens the original.
4. **Private by default.** Remote content is blocked until asked for. No telemetry.
5. **AI from outside.** No AI inside the app; an MCP server (on by default) lets Claude Code, Codex, etc. read and act on mail.
6. **Open source, any provider.** Built for the author's accounts first, but nothing provider-specific is hard-coded beyond presets.

## Stack

- Rust, native UI with GPUI and gpui-kit (shadcn-style components). No browser engine.
- `async-imap` (IMAP) on tokio + rustls, `lettre` (SMTP), `mail-parser` (parsing), `rusqlite` + FTS5 (storage/search), `rmcp` (MCP).

## Architecture

```
crates/yamc (GPUI)  ──calls / events──▶  crates/yamc-core
                                           ├─ store   SQLite (WAL) + FTS5, conversation summary table
                                           ├─ sync    a worker and an IDLE connection per account
                                           ├─ ops     outbox of local changes → server (mark read, archive)
                                           ├─ smtp    send
                                           └─ mcp     Streamable HTTP MCP server on 127.0.0.1
```

`yamc-core` has no UI code, so it can run as a headless daemon later.

## Accounts

| Provider | POC auth | Notes |
|---|---|---|
| Gmail | App password (needs 2FA) | Proper OAuth later; Google expires tokens for unverified "testing" apps every 7 days. Threads via `X-GM-THRID`. |
| Proton | Proton Mail Bridge | Local IMAP `127.0.0.1:1143` / SMTP `127.0.0.1:1025`, STARTTLS with a self-signed cert (trusted for localhost only). |
| Outlook | OAuth2 (milestone 2) | Basic auth is disabled. Needs an Azure app registration (public client, PKCE, loopback redirect). |
| Generic | IMAP/SMTP password | Host/port/TLS entered manually. |

Passwords are sealed with AES-256-GCM using a key kept in the OS keychain (`secret.key` in the data dir when there is none).

## Sync

- Mailboxes are listed per account. Sync covers everything except Spam, Trash, and (on Gmail) All Mail / Important / Starred, which only duplicate other folders.
- **Headers for all messages**, newest first in batches, so the list and search cover all history.
- **Bodies for the last 90 days** (configurable), plus anything opened, cached forever. Opening a message fetches its body on a separate reader connection, so it never waits behind a running sync.
- INBOX is watched with IDLE; other folders re-sync every 5 minutes.
- Flags: CONDSTORE when the server has it, otherwise a full flag fetch. Deleted messages are reconciled by UID set.
- UIDVALIDITY change → that mailbox's cache is dropped and re-synced.

## Data model

- `accounts`, `mailboxes`, `messages` (one row per mailbox+UID), `bodies`, `drafts`, `ops` (pending server changes), `messages_fts`.
- Threads are per account: Gmail thread ID when available, otherwise the `References`/`In-Reply-To` chain.

## Views

- **Shared inbox (default):** conversations with a message in any account's INBOX. Unread first, then newest. Opening a conversation marks it read, but it **keeps its position until you leave the view**. New mail appears at the top.
- **Account → folder:** the same list scoped to one mailbox.
- **Search:** live as you type. FTS prefix match over subject, from, to/cc, and cached bodies, across all accounts; `from:`, `to:`, `cc:`, `subject:`, `body:` scope a word to one field.
- **Conversation:** Gmail-style stacked messages, latest expanded.
- **Sent:** conversations with a message in any account's Sent folder, labelled "To: …".

## Composing

- Send checks recipients and attachment size, saves a draft, closes the composer and sends in the background. Success deletes the draft; a failure leaves it in Drafts and says why.
- To/Cc autocomplete from an address book built from synced mail: people you've sent to rank above people who wrote to you, then by count and recency. Your own addresses and no-reply senders are left out.
- Attachments: Attach button, drop files, or paste (images are saved as files). 25 MB per message.

## Email rendering

- Rendered natively in the app's theme, so there's never a white flash. Layout tables are flattened and the mail's own styles, scripts and tracking pixels are dropped.
- Remote images are blocked until `i`; "always for this sender" is remembered.
- `O` opens the original in the system browser when the exact look matters.
- Links open in the system browser.
- Attachments aren't stored locally. Clicking one downloads it on the reader connection into `~/Downloads` (renamed `name (2).ext` if taken) and opens it; the arrow next to it is Save as….

## Accounts

- Each account has a **label** (shown in yamc) and a **sender name** (the From name on mail you send). Leave the sender name empty and yamc uses the name you most often send under in mail from that address it has already synced.

## Keyboard

| Key | Action |
|---|---|
| `j` / `k` | next / previous |
| `Enter` / `o` | open |
| `Esc` / `u` | back |
| `e` | archive |
| `#` | delete (move to Trash) |
| `r` / `a` | reply / reply all |
| `c` | compose |
| `/` | search |
| `g i` | shared inbox |
| `g t` | sent |
| `i` | load images |
| `O` | open original in browser |

## MCP

Streamable HTTP at `http://127.0.0.1:<port>/mcp` with a bearer token. Settings shows the `claude mcp add` / Codex config to copy.

Tools: `list_accounts`, `list_inbox`, `search`, `get_thread`, `create_draft`, `list_drafts`, `mark_read`, `archive`.
**Agents can't send.** Drafts show in the app and you send them yourself.

## Milestones

1. **POC:** IMAP/SMTP password accounts (Gmail app password, Proton Bridge, generic), shared inbox, folders, conversation view, dark rendering, compose/reply/send, search, MCP server.
2. Outlook OAuth, then Gmail OAuth.
3. ~~Attachments (download/attach)~~, move to folder, per-account settings, packaging (AppImage/deb).
4. Profiling pass.

## Out of scope (for now)

Calendar, contacts, PGP, filters/rules, in-app AI, notifications (never).
