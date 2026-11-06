//! Themes: shadcn-style zinc (default) and the four Catppuccin flavours.
//! https://tailwindcss.com/docs/colors · https://catppuccin.com/palette

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Global, Hsla, Rgba, Window, WindowAppearance, rgb};

pub const THEME_IDS: &[&str] = &["zinc-dark", "zinc-light", "latte", "frappe", "macchiato", "mocha"];

pub const CAT_ACCENTS: &[&str] = &[
    "rosewater", "flamingo", "pink", "mauve", "red", "maroon", "peach", "yellow", "green", "teal", "sky", "sapphire",
    "blue", "lavender",
];

struct Cat {
    accents: [u32; 14],
    text: u32,
    subtext0: u32,
    overlay1: u32,
    surface1: u32,
    surface0: u32,
    base: u32,
    mantle: u32,
    crust: u32,
}

const LATTE: Cat = Cat {
    accents: [0xdc8a78, 0xdd7878, 0xea76cb, 0x8839ef, 0xd20f39, 0xe64553, 0xfe640b, 0xdf8e1d, 0x40a02b, 0x179299, 0x04a5e5, 0x209fb5, 0x1e66f5, 0x7287fd],
    text: 0x4c4f69,
    subtext0: 0x6c6f85,
    overlay1: 0x8c8fa1,
    surface1: 0xbcc0cc,
    surface0: 0xccd0da,
    base: 0xeff1f5,
    mantle: 0xe6e9ef,
    crust: 0xdce0e8,
};
const FRAPPE: Cat = Cat {
    accents: [0xf2d5cf, 0xeebebe, 0xf4b8e4, 0xca9ee6, 0xe78284, 0xea999c, 0xef9f76, 0xe5c890, 0xa6d189, 0x81c8be, 0x99d1db, 0x85c1dc, 0x8caaee, 0xbabbf1],
    text: 0xc6d0f5,
    subtext0: 0xa5adce,
    overlay1: 0x838ba7,
    surface1: 0x51576d,
    surface0: 0x414559,
    base: 0x303446,
    mantle: 0x292c3c,
    crust: 0x232634,
};
const MACCHIATO: Cat = Cat {
    accents: [0xf4dbd6, 0xf0c6c6, 0xf5bde6, 0xc6a0f6, 0xed8796, 0xee99a0, 0xf5a97f, 0xeed49f, 0xa6da95, 0x8bd5ca, 0x91d7e3, 0x7dc4e4, 0x8aadf4, 0xb7bdf8],
    text: 0xcad3f5,
    subtext0: 0xa5adcb,
    overlay1: 0x8087a2,
    surface1: 0x494d64,
    surface0: 0x363a4f,
    base: 0x24273a,
    mantle: 0x1e2030,
    crust: 0x181926,
};
const MOCHA: Cat = Cat {
    accents: [0xf5e0dc, 0xf2cdcd, 0xf5c2e7, 0xcba6f7, 0xf38ba8, 0xeba0ac, 0xfab387, 0xf9e2af, 0xa6e3a1, 0x94e2d5, 0x89dceb, 0x74c7ec, 0x89b4fa, 0xb4befe],
    text: 0xcdd6f4,
    subtext0: 0xa6adc8,
    overlay1: 0x7f849c,
    surface1: 0x45475a,
    surface0: 0x313244,
    base: 0x1e1e2e,
    mantle: 0x181825,
    crust: 0x11111b,
};

fn cat(id: &str) -> Option<&'static Cat> {
    match id {
        "latte" => Some(&LATTE),
        "frappe" => Some(&FRAPPE),
        "macchiato" => Some(&MACCHIATO),
        "mocha" => Some(&MOCHA),
        _ => None,
    }
}

pub fn label(id: &str) -> &'static str {
    match id {
        "zinc-dark" => "Zinc",
        "zinc-light" => "Zinc Light",
        "latte" => "Latte",
        "frappe" => "Frappé",
        "macchiato" => "Macchiato",
        "mocha" => "Mocha",
        _ => "System",
    }
}

fn accent_index(accent: &str) -> usize {
    CAT_ACCENTS.iter().position(|a| *a == accent).unwrap_or(3)
}

/// The accent colour a Catppuccin theme would use, for the swatches.
pub fn cat_swatch(id: &str, accent: &str) -> Option<Hsla> {
    cat(id).map(|p| c(p.accents[accent_index(accent)]))
}

fn c(hex: u32) -> Hsla {
    Rgba::from(rgb(hex)).into()
}

/// The colours yamc paints with. Mirrors shadcn's tokens.
#[derive(Clone, Debug)]
pub struct Palette {
    pub dark: bool,
    pub bg: Hsla,
    pub panel: Hsla,
    pub card: Hsla,
    pub hover: Hsla,
    pub active: Hsla,
    pub line: Hsla,
    pub text: Hsla,
    pub strong: Hsla,
    pub muted: Hsla,
    pub faint: Hsla,
    pub accent: Hsla,
    pub accent_fg: Hsla,
    pub ring: Hsla,
    pub danger: Hsla,
    pub link: Hsla,
    pub accounts: [Hsla; 6],
}

impl Global for Palette {}

impl Palette {
    pub fn account(&self, index: usize) -> Hsla {
        self.accounts[index % self.accounts.len()]
    }
}

fn zinc(dark: bool) -> Palette {
    // Tailwind zinc.
    let (z50, z100, z200, z400, z500, z700, z800, z900, z950) =
        (0xfafafa, 0xf4f4f5, 0xe4e4e7, 0xa1a1aa, 0x71717a, 0x3f3f46, 0x27272a, 0x18181b, 0x09090b);
    if dark {
        Palette {
            dark,
            bg: c(z950),
            panel: c(0x0c0c0e),
            card: c(0x0f0f11),
            hover: c(z900),
            active: c(z800),
            line: c(z800),
            text: c(z200),
            strong: c(z50),
            muted: c(z400),
            faint: c(z500),
            accent: c(z50),
            accent_fg: c(z900),
            ring: c(0x52525b),
            danger: c(0xf87171),
            link: c(z50),
            // Tailwind 400s: soft enough to sit next to zinc without shouting.
            accounts: [c(0xa78bfa), c(0xfb7185), c(0xfbbf24), c(0x34d399), c(0x38bdf8), c(0xfb923c)],
        }
    } else {
        Palette {
            dark,
            bg: c(0xffffff),
            panel: c(z50),
            card: c(0xffffff),
            hover: c(z100),
            active: c(z100),
            line: c(z200),
            text: c(z700),
            strong: c(z950),
            muted: c(z500),
            faint: c(z400),
            accent: c(z900),
            accent_fg: c(z50),
            ring: c(z400),
            danger: c(0xdc2626),
            link: c(z950),
            accounts: [c(0x7c3aed), c(0xe11d48), c(0xd97706), c(0x059669), c(0x0284c7), c(0xea580c)],
        }
    }
}

fn catppuccin(id: &str, p: &Cat, accent: &str) -> Palette {
    let a = c(p.accents[accent_index(accent)]);
    let [_, _, pink, mauve, _, _, peach, yellow, green, _, sky, _, _, _] = p.accents;
    Palette {
        dark: id != "latte",
        bg: c(p.base),
        panel: c(p.mantle),
        card: c(p.mantle),
        hover: c(p.surface0),
        active: c(p.surface1),
        line: c(p.surface0),
        text: c(p.text),
        strong: c(p.text),
        muted: c(p.subtext0),
        faint: c(p.overlay1),
        accent: a,
        accent_fg: c(p.crust),
        ring: a,
        danger: c(p.accents[4]),
        link: a,
        accounts: [c(mauve), c(peach), c(green), c(sky), c(pink), c(yellow)],
    }
}

pub fn palette(setting: &str, accent: &str, system_dark: bool) -> Palette {
    let id = if setting == "system" { if system_dark { "zinc-dark" } else { "zinc-light" } } else { setting };
    match id {
        "zinc-light" => zinc(false),
        "latte" => catppuccin("latte", &LATTE, accent),
        "frappe" => catppuccin("frappe", &FRAPPE, accent),
        "macchiato" => catppuccin("macchiato", &MACCHIATO, accent),
        "mocha" => catppuccin("mocha", &MOCHA, accent),
        _ => zinc(true),
    }
}

pub fn system_dark(window: Option<&Window>, cx: &App) -> bool {
    let a = window.map(|w| w.appearance()).unwrap_or_else(|| cx.window_appearance());
    matches!(a, WindowAppearance::Dark | WindowAppearance::VibrantDark)
}

/// Make the component library match the palette and store it for our own views.
pub fn apply(p: Palette, window: Option<&mut Window>, cx: &mut App) {
    Theme::change(if p.dark { ThemeMode::Dark } else { ThemeMode::Light }, window, cx);
    let t = Theme::global_mut(cx);
    let soft = |h: Hsla, a: f32| Hsla { a, ..h };
    t.background = p.bg;
    t.foreground = p.text;
    t.border = p.line;
    t.input = p.line;
    t.ring = p.ring;
    t.caret = p.strong;
    t.muted = p.hover;
    t.muted_foreground = p.muted;
    t.accent = p.hover;
    t.accent_foreground = p.strong;
    t.primary = p.accent;
    t.primary_hover = soft(p.accent, 0.9);
    t.primary_active = soft(p.accent, 0.8);
    t.primary_foreground = p.accent_fg;
    t.secondary = p.hover;
    t.secondary_hover = p.active;
    t.secondary_active = p.active;
    t.secondary_foreground = p.text;
    t.popover = p.card;
    t.popover_foreground = p.text;
    t.list_hover = p.hover;
    t.list_active = p.active;
    t.list_active_border = p.ring;
    t.selection = soft(p.accent, 0.3);
    t.link = p.link;
    t.link_hover = p.strong;
    t.link_active = p.strong;
    t.danger = p.danger;
    t.sidebar = p.panel;
    t.sidebar_foreground = p.text;
    t.sidebar_border = p.line;
    t.sidebar_accent = p.hover;
    t.sidebar_accent_foreground = p.strong;
    t.scrollbar = soft(p.bg, 0.0);
    t.scrollbar_thumb = p.line;
    t.scrollbar_thumb_hover = p.ring;
    t.switch = p.active;
    t.title_bar = p.panel;
    t.title_bar_border = p.line;
    t.window_border = p.line;
    t.overlay = Hsla { h: 0., s: 0., l: 0., a: 0.5 };
    t.radius = gpui_kit::px(6.);
    t.radius_lg = gpui_kit::px(10.);
    cx.set_global(p);
}
