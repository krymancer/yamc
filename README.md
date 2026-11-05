<img src="assets/icon.svg" width="64" height="64" alt="">

# yamc

Yet another mail client: fast, minimal, dark by default, and drivable by AI agents over MCP.

- **Shared inbox** across all accounts, unread first. Opened mail stays in place until you leave the view.
- **No counts, badges or notifications.**
- **Dark email rendering** without white flashes (`d` toggles original colours). Remote images blocked until `i`.
- **Local-first:** everything is read from SQLite, never from the network.
- **MCP server** on `127.0.0.1` so Claude Code / Codex can read, search, draft, mark read and archive. Agents can't send.

See [SPEC.md](SPEC.md) for the design and roadmap.

## Run

```sh
npm install
npm run dev        # hot-reload dev build
npm run build && npm start
```

Add an account in Settings (`g s`):

- **Gmail:** app password (needs 2-Step Verification).
- **Proton:** run Proton Mail Bridge and use the credentials it shows.
- **Other:** any IMAP/SMTP server.
- **Outlook:** needs OAuth, coming in milestone 2.

Then copy the Claude Code command or Codex config from Settings → MCP.

## Keys

Vim-style, with counts (`5j`, `3J`). Press `?` in the app for the full list.

- **List:** `j`/`k`, `gg`/`G`, `ctrl-d`/`ctrl-u`, `ctrl-f`/`ctrl-b`, `H`/`M`/`L`, `zz`/`zt`/`zb`, `enter`/`o`/`l` open
- **Select:** `x` toggle, `V` visual range, then `e`/`dd` archive, `U` unread, `I` read, `esc` clear
- **Reading:** `j`/`k` scroll, `ctrl-d`/`ctrl-u`, `gg`/`G`, `J`/`K` next/prev conversation, `r`/`a` reply/all, `e` archive, `d` colours, `i` images, `q`/`h`/`esc` back
- **Anywhere:** `c` compose, `/` search, `g i` inbox, `g d` drafts, `g s` settings, `\` sidebar

## Test

```sh
npm test           # unit tests
docker run -d --rm --name yamc-greenmail \
  -e GREENMAIL_OPTS='-Dgreenmail.setup.test.all -Dgreenmail.hostname=0.0.0.0 -Dgreenmail.auth.disabled' \
  -p 3025:3025 -p 3143:3143 greenmail/standalone
npm run test:it    # end-to-end against a real IMAP/SMTP server
```

## Layout

- `src/core`: database, IMAP sync, SMTP, MCP server. No Electron imports.
- `src/main`: Electron main process, IPC, secret storage.
- `src/renderer`: React UI.
- `src/shared`: types shared by all of the above.
