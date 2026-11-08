//! Vim-style key engine: counts (5j), sequences (gg, dd, zz), ctrl chords (<C-d>).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    List,
    Thread,
    Page,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    Down,
    Up,
    First,
    Last,
    ScreenTop,
    ScreenMiddle,
    ScreenBottom,
    HalfDown,
    HalfUp,
    PageDown,
    PageUp,
    AlignCenter,
    AlignTop,
    AlignBottom,
    ToggleSelect,
    Visual,
    Open,
    Archive,
    Trash,
    MarkUnread,
    MarkRead,
    Reply,
    ReplyAll,
    Compose,
    ScrollDown,
    ScrollUp,
    ScrollHalfDown,
    ScrollHalfUp,
    ScrollPageDown,
    ScrollPageUp,
    ScrollTop,
    ScrollBottom,
    NextThread,
    PrevThread,
    LoadImages,
    OpenOriginal,
    Back,
    GoInbox,
    GoDrafts,
    GoSent,
    GoSettings,
    Search,
    ToggleSidebar,
    Help,
    ClearOrInbox,
    BackToInbox,
}

pub struct Binding {
    /// Alternative sequences, e.g. `["j", "<Down>"]` or `["gg"]`.
    pub keys: &'static [&'static str],
    pub modes: &'static [Mode],
    pub group: &'static str,
    pub desc: &'static str,
    pub cmd: Cmd,
    /// Left out of the help overlay (aliases and per-mode duplicates).
    pub hidden: bool,
}

use Cmd::*;
use Mode::*;

const L: &[Mode] = &[List];
const T: &[Mode] = &[Thread];
const LT: &[Mode] = &[List, Thread];
const TP: &[Mode] = &[Thread, Page];
const P: &[Mode] = &[Page];
const ALL: &[Mode] = &[List, Thread, Page];

macro_rules! b {
    ($keys:expr, $modes:expr, $group:expr, $desc:expr, $cmd:expr) => {
        Binding { keys: $keys, modes: $modes, group: $group, desc: $desc, cmd: $cmd, hidden: false }
    };
    ($keys:expr, $modes:expr, $group:expr, $desc:expr, $cmd:expr, hidden) => {
        Binding { keys: $keys, modes: $modes, group: $group, desc: $desc, cmd: $cmd, hidden: true }
    };
}

pub const BINDINGS: &[Binding] = &[
    // move
    b!(&["j", "<Down>"], L, "Move", "Down (count: 5j)", Down),
    b!(&["k", "<Up>"], L, "Move", "Up", Up),
    b!(&["gg", "<Home>"], L, "Move", "First (or line N)", First),
    b!(&["G", "<End>"], L, "Move", "Last (or line N)", Last),
    b!(&["H"], L, "Move", "Top of screen", ScreenTop),
    b!(&["M"], L, "Move", "Middle of screen", ScreenMiddle),
    b!(&["L"], L, "Move", "Bottom of screen", ScreenBottom),
    b!(&["<C-d>"], L, "Scroll", "Half page down", HalfDown),
    b!(&["<C-u>"], L, "Scroll", "Half page up", HalfUp),
    b!(&["<C-f>", "<PageDown>"], L, "Scroll", "Page down", PageDown),
    b!(&["<C-b>", "<PageUp>"], L, "Scroll", "Page up", PageUp),
    b!(&["zz"], L, "Scroll", "Centre cursor", AlignCenter),
    b!(&["zt"], L, "Scroll", "Cursor to top", AlignTop),
    b!(&["zb"], L, "Scroll", "Cursor to bottom", AlignBottom),
    // select
    b!(&["x"], L, "Select", "Toggle selection", ToggleSelect),
    b!(&["V", "v"], L, "Select", "Visual mode (select a range)", Visual),
    b!(&["<Esc>"], L, "Select", "Clear selection / back to inbox", ClearOrInbox),
    // act
    b!(&["<CR>", "o", "l"], L, "Act", "Open", Open),
    b!(&["e", "dd"], L, "Act", "Archive (selection)", Archive),
    b!(&["e"], T, "Act", "Archive", Archive, hidden),
    b!(&["#"], LT, "Act", "Delete (to Trash)", Trash),
    b!(&["U"], LT, "Act", "Mark unread", MarkUnread),
    b!(&["I"], L, "Act", "Mark read", MarkRead),
    b!(&["r"], T, "Act", "Reply", Reply),
    b!(&["a"], T, "Act", "Reply all", ReplyAll),
    b!(&["c"], ALL, "Act", "Compose", Compose),
    // reading
    b!(&["j", "<Down>"], TP, "Reading", "Scroll down", ScrollDown, hidden),
    b!(&["k", "<Up>"], TP, "Reading", "Scroll up", ScrollUp, hidden),
    b!(&["<C-d>"], TP, "Reading", "Half page down", ScrollHalfDown, hidden),
    b!(&["<C-u>"], TP, "Reading", "Half page up", ScrollHalfUp, hidden),
    b!(&["<C-f>", "<Space>", "<PageDown>"], TP, "Reading", "Page down", ScrollPageDown, hidden),
    b!(&["<C-b>", "<PageUp>"], TP, "Reading", "Page up", ScrollPageUp, hidden),
    b!(&["gg"], TP, "Reading", "Top", ScrollTop, hidden),
    b!(&["G"], TP, "Reading", "Bottom", ScrollBottom, hidden),
    b!(&["J"], T, "Reading", "Next conversation", NextThread),
    b!(&["K"], T, "Reading", "Previous conversation", PrevThread),
    b!(&["i"], T, "Reading", "Load images", LoadImages),
    b!(&["O"], T, "Reading", "Open original in browser", OpenOriginal),
    b!(&["<Esc>", "q", "h", "u"], T, "Reading", "Back to list", Back),
    // go
    b!(&["gi"], ALL, "Go", "Inbox", GoInbox),
    b!(&["gt"], ALL, "Go", "Sent", GoSent),
    b!(&["gd"], ALL, "Go", "Drafts", GoDrafts),
    b!(&["gs"], ALL, "Go", "Settings", GoSettings),
    b!(&["/"], ALL, "Go", "Search", Search),
    b!(&["\\"], ALL, "Go", "Toggle sidebar", ToggleSidebar),
    b!(&["?"], ALL, "Go", "All keys", Help),
    b!(&["<Esc>", "q"], P, "Go", "Back to inbox", BackToInbox, hidden),
];

/// Split a binding spec like `g<CR>` into tokens `["g", "<CR>"]`.
pub fn tokenize(spec: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = spec.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' && spec.len() > 1 {
            let mut tok = String::from("<");
            let mut closed = false;
            for d in chars.by_ref() {
                tok.push(d);
                if d == '>' {
                    closed = true;
                    break;
                }
            }
            if closed && tok.len() > 2 {
                out.push(tok);
                continue;
            }
            out.extend(tok.chars().map(String::from));
            continue;
        }
        out.push(c.to_string());
    }
    out
}

/// Normalize a key press to a vim-ish token: `j`, `G`, `<C-d>`, `<CR>`. None for bare modifiers.
pub fn key_token(key: &str, key_char: Option<&str>, ctrl: bool, alt: bool, platform: bool) -> Option<String> {
    let named = match key {
        "enter" => Some("CR"),
        "escape" => Some("Esc"),
        "down" => Some("Down"),
        "up" => Some("Up"),
        "left" => Some("Left"),
        "right" => Some("Right"),
        "space" => Some("Space"),
        "tab" => Some("Tab"),
        "backspace" => Some("BS"),
        "pagedown" => Some("PageDown"),
        "pageup" => Some("PageUp"),
        "home" => Some("Home"),
        "end" => Some("End"),
        "shift" | "control" | "alt" | "platform" | "function" | "capslock" => return None,
        _ => None,
    };
    if ctrl || platform {
        let base = named.map(String::from).unwrap_or_else(|| key.to_lowercase());
        return Some(format!("<C-{base}>"));
    }
    if alt {
        return None;
    }
    if let Some(n) = named {
        return Some(format!("<{n}>"));
    }
    key_char.filter(|c| !c.is_empty()).map(String::from).or_else(|| (key.chars().count() == 1).then(|| key.to_string()))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Feed {
    Run(Cmd, Option<usize>),
    Pending(String),
    None,
}

#[derive(Default)]
pub struct KeyEngine {
    count: String,
    seq: Vec<String>,
    /// A binding that matched exactly while a longer one was still possible (`d` vs `dd`).
    waiting: Option<Cmd>,
}

impl KeyEngine {
    pub fn reset(&mut self) {
        self.count.clear();
        self.seq.clear();
        self.waiting = None;
    }

    pub fn pending(&self) -> String {
        format!("{}{}", self.count, self.seq.concat())
    }

    fn count(&self) -> Option<usize> {
        self.count.parse().ok()
    }

    /// The timeout expired (vim's timeoutlen): run the shorter match, if any.
    pub fn flush(&mut self) -> Feed {
        let w = self.waiting;
        let c = self.count();
        self.reset();
        match w {
            Some(cmd) => Feed::Run(cmd, c),
            None => Feed::None,
        }
    }

    pub fn feed(&mut self, token: &str, mode: Mode) -> Feed {
        let is_digit = token.len() == 1 && token.chars().all(|c| c.is_ascii_digit());
        if self.seq.is_empty() && is_digit && (!self.count.is_empty() || token != "0") {
            self.count.push_str(token);
            return Feed::Pending(self.pending());
        }
        self.seq.push(token.to_string());
        let mut exact = None;
        let mut longer = false;
        for b in BINDINGS.iter().filter(|b| b.modes.contains(&mode)) {
            for spec in b.keys {
                let t = tokenize(spec);
                if t.len() < self.seq.len() || !self.seq.iter().zip(&t).all(|(a, b)| a == b) {
                    continue;
                }
                if t.len() == self.seq.len() {
                    exact.get_or_insert(b.cmd);
                } else {
                    longer = true;
                }
            }
        }
        if let (Some(cmd), false) = (exact, longer) {
            let c = self.count();
            self.reset();
            return Feed::Run(cmd, c);
        }
        if longer {
            self.waiting = exact;
            return Feed::Pending(self.pending());
        }
        // Unknown sequence: drop it, but let a lone key start fresh (e.g. `gx` then `j`).
        let was = self.seq.len();
        self.reset();
        if was > 1 { self.feed(token, mode) } else { Feed::None }
    }
}

/// How a binding spec is shown in the help overlay.
pub fn pretty(spec: &str) -> String {
    tokenize(spec)
        .iter()
        .map(|t| match t.as_str() {
            "<CR>" => "↵".to_string(),
            "<Esc>" => "esc".to_string(),
            "<Space>" => "space".to_string(),
            "<Down>" => "↓".to_string(),
            "<Up>" => "↑".to_string(),
            t if t.starts_with("<C-") && t.ends_with('>') => format!("⌃{}", &t[3..t.len() - 1]),
            t if t.starts_with('<') && t.ends_with('>') && t.len() > 2 => t[1..t.len() - 1].to_string(),
            t => t.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens() {
        assert_eq!(tokenize("gg"), ["g", "g"]);
        assert_eq!(tokenize("<C-d>"), ["<C-d>"]);
        assert_eq!(tokenize("g<CR>"), ["g", "<CR>"]);
        assert_eq!(tokenize("<"), ["<"]);
        assert_eq!(key_token("j", Some("j"), false, false, false).as_deref(), Some("j"));
        assert_eq!(key_token("j", Some("J"), false, false, false).as_deref(), Some("J"));
        assert_eq!(key_token("d", None, true, false, false).as_deref(), Some("<C-d>"));
        assert_eq!(key_token("enter", None, false, false, false).as_deref(), Some("<CR>"));
        assert_eq!(key_token("shift", None, false, false, false), None);
        assert_eq!(key_token("/", Some("/"), false, false, false).as_deref(), Some("/"));
    }

    #[test]
    fn counts_and_sequences() {
        let mut e = KeyEngine::default();
        assert_eq!(e.feed("5", Mode::List), Feed::Pending("5".into()));
        assert_eq!(e.feed("j", Mode::List), Feed::Run(Cmd::Down, Some(5)));
        assert_eq!(e.feed("g", Mode::List), Feed::Pending("g".into()));
        assert_eq!(e.feed("g", Mode::List), Feed::Run(Cmd::First, None));
        assert_eq!(e.feed("1", Mode::List), Feed::Pending("1".into()));
        assert_eq!(e.feed("2", Mode::List), Feed::Pending("12".into()));
        assert_eq!(e.feed("G", Mode::List), Feed::Run(Cmd::Last, Some(12)));
        assert_eq!(e.feed("d", Mode::List), Feed::Pending("d".into()));
        assert_eq!(e.feed("d", Mode::List), Feed::Run(Cmd::Archive, None));
        assert_eq!(e.feed("<C-d>", Mode::List), Feed::Run(Cmd::HalfDown, None));
    }

    #[test]
    fn modes_and_recovery() {
        let mut e = KeyEngine::default();
        assert_eq!(e.feed("j", Mode::Thread), Feed::Run(Cmd::ScrollDown, None));
        assert_eq!(e.feed("J", Mode::Thread), Feed::Run(Cmd::NextThread, None));
        assert_eq!(e.feed("e", Mode::Thread), Feed::Run(Cmd::Archive, None));
        // `g` then an unknown key: the second key starts fresh.
        assert_eq!(e.feed("g", Mode::List), Feed::Pending("g".into()));
        assert_eq!(e.feed("j", Mode::List), Feed::Run(Cmd::Down, None));
        // `0` alone is not a count.
        assert_eq!(e.feed("0", Mode::List), Feed::None);
        // Timeout runs nothing when nothing matched exactly.
        assert_eq!(e.feed("z", Mode::List), Feed::Pending("z".into()));
        assert_eq!(e.flush(), Feed::None);
        assert_eq!(pretty("<C-d>"), "⌃d");
        assert_eq!(pretty("<CR>"), "↵");
    }
}
