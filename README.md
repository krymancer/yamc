<img src="assets/icon.svg" width="64" height="64" alt="">

# yamc

My ego and ignorance brought me here. Email was always more like notifications to me than anything else. I rarely
communicated by email, opting, out of convenience or just habit, for an IM client instead. Having been on the internet
for some years now, I've accumulated a bunch of email addresses that I need to check constantly. I've always looked for
an email client where I could have all my email in one big inbox: color-coded, insanely fast, minimal and beautiful.
I tried every option on the "10 best email clients" lists and never really stuck with any of them. I'm just sick of
scrolling through the list of Google accounts I'm logged into in my browser, or, when I need a 2FA code sent to some
address, having to log in again because I either reset my PC and never logged back into that Outlook account, or went
months without using it and got logged out.

That's why I'm making this: so I can have all my "notifications" in one place, as fast as possible. Right now I don't
have much ambition or many priorities. I just want to make an email client that doesn't suck (for me, at least). It
won't have that many features; I just need a clean, minimal UI, dark mode, for it to be fucking fast, and for it to
get out of my way.

Yet another mail client: fast, minimal, dark by default, and drivable by AI agents over MCP.

- **Shared inbox** across all accounts, unread first. Opened mail stays in place until you leave the view.
- **No counts, badges or notifications.**
- **Native email rendering** in the theme's colours, so nothing flashes white. Remote images blocked until `i`.
- **Local-first:** everything is read from SQLite, never from the network.
- **MCP server** on `127.0.0.1` so Claude Code / Codex can read, search, draft, mark read and archive. Agents can't send.

See [SPEC.md](SPEC.md) for the design and roadmap.

## Install (macOS)

```sh
curl -fL https://github.com/krymancer/yamc/releases/latest/download/yamc-macos.zip -o /tmp/yamc.zip \
  && ditto -x -k /tmp/yamc.zip /Applications && open /Applications/yamc.app
```

Installing from Terminal avoids Gatekeeper's warning about an app downloaded with a browser. Releases are signed
with a self-made certificate, not an Apple Developer ID. If you download the zip in a browser instead, open the app
once from System Settings → Privacy & Security → Open Anyway.

yamc updates itself: it checks GitHub releases at launch and every 6 hours, installs a newer version in the background,
then the sidebar shows **Restart to update**. yamc → Check for Updates… checks right away. It only installs builds
signed with the release certificate. Every release carries the same signature, so after you choose Always Allow
once, macOS stops asking for Keychain access on updates.

The certificate's private key lives in the `MACOS_SIGNING_P12` and `MACOS_SIGNING_PASSWORD` Actions secrets, and
a backup is kept in the maintainer's login keychain. If it's ever replaced, update `SIGNING_CERT` in
`crates/yamc/src/update.rs` and the identity in the release workflow; installed copies then need a manual reinstall.

To publish a release, push a version tag, or create a release with a new `v…` tag on GitHub:

```sh
git tag -a v0.2.0 -m v0.2.0 && git push origin v0.2.0
```

`.github/workflows/release.yml` builds the universal app with the tag's version and attaches `yamc-macos.zip` to the
release. `scripts/bundle-macos.sh` builds the same zip locally into `dist/`.

## Run from source

The app is Rust + [GPUI](https://www.gpui.rs) ([gpui-kit](https://gpui-kit.com) components):

```sh
cargo run --release
```

Data lives in `~/.local/share/yamc` (macOS: `~/Library/Application Support/yamc`); set `YAMC_DATA_DIR` to use another
folder. Passwords are sealed with a key kept in the system keychain, or in `secret.key` when there is none.

Add an account in Settings (`g s`):

- **Gmail:** app password (needs 2-Step Verification).
- **Proton:** run Proton Mail Bridge and use the credentials it shows.
- **Other:** any IMAP/SMTP server.
- **Outlook:** needs OAuth, coming in milestone 2.

Then copy the Claude Code command or Codex config from Settings → MCP.

## Keys

Vim-style, with counts (`5j`, `3J`). Press `?` in the app for the full list.

- **List:** `j`/`k`, `gg`/`G`, `ctrl-d`/`ctrl-u`, `ctrl-f`/`ctrl-b`, `H`/`M`/`L`, `zz`/`zt`/`zb`, `enter`/`o`/`l` open
- **Select:** `x` toggle, `V` visual range, then `e`/`dd` archive, `#` delete, `U` unread, `I` read, `esc` clear
- **Reading:** `j`/`k` scroll, `ctrl-d`/`ctrl-u`, `gg`/`G`, `J`/`K` next/prev conversation, `r`/`a` reply/all, `e` archive, `#` delete, `i` images, `O` open original in browser, `q`/`h`/`esc` back
- **Anywhere:** `c` compose, `/` search (live; `from:` `to:` `cc:` `subject:` `body:` narrow a word), `g i` inbox, `g d` drafts, `g s` settings, `\` sidebar

## Test

```sh
cargo test                       # unit tests + store benchmark (100k messages)
docker run -d --rm --name yamc-greenmail \
  -e GREENMAIL_OPTS='-Dgreenmail.setup.test.all -Dgreenmail.hostname=0.0.0.0 -Dgreenmail.auth.disabled' \
  -p 3025:3025 -p 3143:3143 greenmail/standalone
YAMC_IT=1 cargo test             # plus end-to-end against a real IMAP/SMTP server, MCP included
```

## Layout

- `crates/yamc-core`: SQLite store, IMAP sync (async-imap, IDLE), SMTP (lettre), MCP server (rmcp). No UI code.
- `crates/yamc`: the GPUI app: list, conversation, composer, settings, vim keys, themes.
