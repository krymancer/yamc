// Themes: shadcn-style zinc (default) and the four Catppuccin flavours.
// https://tailwindcss.com/docs/colors · https://catppuccin.com/palette

export type ThemeId = 'zinc-dark' | 'zinc-light' | 'latte' | 'frappe' | 'macchiato' | 'mocha'
export type ThemeSetting = ThemeId | 'system'

export const CAT_ACCENTS = [
  'rosewater', 'flamingo', 'pink', 'mauve', 'red', 'maroon', 'peach',
  'yellow', 'green', 'teal', 'sky', 'sapphire', 'blue', 'lavender',
] as const
export type CatAccent = (typeof CAT_ACCENTS)[number]

type CatPalette = Record<CatAccent, string> & {
  text: string; subtext1: string; subtext0: string; overlay2: string; overlay1: string; overlay0: string
  surface2: string; surface1: string; surface0: string; base: string; mantle: string; crust: string
}

const CAT: Record<'latte' | 'frappe' | 'macchiato' | 'mocha', CatPalette> = {
  latte: {
    rosewater: '#dc8a78', flamingo: '#dd7878', pink: '#ea76cb', mauve: '#8839ef', red: '#d20f39', maroon: '#e64553',
    peach: '#fe640b', yellow: '#df8e1d', green: '#40a02b', teal: '#179299', sky: '#04a5e5', sapphire: '#209fb5',
    blue: '#1e66f5', lavender: '#7287fd', text: '#4c4f69', subtext1: '#5c5f77', subtext0: '#6c6f85', overlay2: '#7c7f93',
    overlay1: '#8c8fa1', overlay0: '#9ca0b0', surface2: '#acb0be', surface1: '#bcc0cc', surface0: '#ccd0da',
    base: '#eff1f5', mantle: '#e6e9ef', crust: '#dce0e8',
  },
  frappe: {
    rosewater: '#f2d5cf', flamingo: '#eebebe', pink: '#f4b8e4', mauve: '#ca9ee6', red: '#e78284', maroon: '#ea999c',
    peach: '#ef9f76', yellow: '#e5c890', green: '#a6d189', teal: '#81c8be', sky: '#99d1db', sapphire: '#85c1dc',
    blue: '#8caaee', lavender: '#babbf1', text: '#c6d0f5', subtext1: '#b5bfe2', subtext0: '#a5adce', overlay2: '#949cbb',
    overlay1: '#838ba7', overlay0: '#737994', surface2: '#626880', surface1: '#51576d', surface0: '#414559',
    base: '#303446', mantle: '#292c3c', crust: '#232634',
  },
  macchiato: {
    rosewater: '#f4dbd6', flamingo: '#f0c6c6', pink: '#f5bde6', mauve: '#c6a0f6', red: '#ed8796', maroon: '#ee99a0',
    peach: '#f5a97f', yellow: '#eed49f', green: '#a6da95', teal: '#8bd5ca', sky: '#91d7e3', sapphire: '#7dc4e4',
    blue: '#8aadf4', lavender: '#b7bdf8', text: '#cad3f5', subtext1: '#b8c0e0', subtext0: '#a5adcb', overlay2: '#939ab7',
    overlay1: '#8087a2', overlay0: '#6e738d', surface2: '#5b6078', surface1: '#494d64', surface0: '#363a4f',
    base: '#24273a', mantle: '#1e2030', crust: '#181926',
  },
  mocha: {
    rosewater: '#f5e0dc', flamingo: '#f2cdcd', pink: '#f5c2e7', mauve: '#cba6f7', red: '#f38ba8', maroon: '#eba0ac',
    peach: '#fab387', yellow: '#f9e2af', green: '#a6e3a1', teal: '#94e2d5', sky: '#89dceb', sapphire: '#74c7ec',
    blue: '#89b4fa', lavender: '#b4befe', text: '#cdd6f4', subtext1: '#bac2de', subtext0: '#a6adc8', overlay2: '#9399b2',
    overlay1: '#7f849c', overlay0: '#6c7086', surface2: '#585b70', surface1: '#45475a', surface0: '#313244',
    base: '#1e1e2e', mantle: '#181825', crust: '#11111b',
  },
}

export interface Theme {
  id: ThemeId
  label: string
  dark: boolean
  /** CSS custom properties, without the leading `--`. */
  vars: Record<string, string>
  /** Colours that tell accounts apart. */
  accountColors: string[]
}

const zinc = {
  50: '#fafafa', 100: '#f4f4f5', 200: '#e4e4e7', 300: '#d4d4d8', 400: '#a1a1aa', 500: '#71717a',
  600: '#52525b', 700: '#3f3f46', 800: '#27272a', 900: '#18181b', 950: '#09090b',
}

// Tailwind 400s: soft enough to sit next to zinc without shouting.
const TW_ACCOUNT_DARK = ['#a78bfa', '#fb7185', '#fbbf24', '#34d399', '#38bdf8', '#fb923c']
const TW_ACCOUNT_LIGHT = ['#7c3aed', '#e11d48', '#d97706', '#059669', '#0284c7', '#ea580c']

function zincTheme(dark: boolean): Theme {
  return {
    id: dark ? 'zinc-dark' : 'zinc-light',
    label: dark ? 'Zinc' : 'Zinc Light',
    dark,
    accountColors: dark ? TW_ACCOUNT_DARK : TW_ACCOUNT_LIGHT,
    vars: dark
      ? {
          bg: zinc[950], panel: '#0c0c0e', card: '#0f0f11', raised: zinc[900], hover: '#18181b', active: zinc[800],
          line: zinc[800], text: zinc[200], strong: zinc[50], muted: zinc[400], faint: zinc[500],
          accent: zinc[50], 'accent-fg': zinc[900], ring: zinc[600], danger: '#f87171', 'mail-bg': zinc[950], 'mail-text': zinc[200],
          link: zinc[50],
        }
      : {
          bg: '#ffffff', panel: zinc[50], card: '#ffffff', raised: '#ffffff', hover: zinc[100], active: zinc[100],
          line: zinc[200], text: zinc[700], strong: zinc[950], muted: zinc[500], faint: zinc[400],
          accent: zinc[900], 'accent-fg': zinc[50], ring: zinc[400], danger: '#dc2626', 'mail-bg': '#ffffff', 'mail-text': zinc[800],
          link: zinc[950],
        },
  }
}

function catTheme(id: keyof typeof CAT, accent: CatAccent): Theme {
  const p = CAT[id]
  const dark = id !== 'latte'
  const a = p[accent]
  return {
    id,
    label: { latte: 'Latte', frappe: 'Frappé', macchiato: 'Macchiato', mocha: 'Mocha' }[id],
    dark,
    accountColors: [p.mauve, p.peach, p.green, p.sky, p.pink, p.yellow],
    vars: {
      bg: p.base, panel: p.mantle, card: p.mantle, raised: p.mantle, hover: p.surface0, active: p.surface0,
      line: p.surface0, text: p.text, strong: dark ? p.text : p.text, muted: p.subtext0, faint: p.overlay1,
      accent: a, 'accent-fg': p.crust, ring: a, danger: p.red, 'mail-bg': p.base, 'mail-text': p.text, link: a,
    },
  }
}

export const THEME_IDS: ThemeId[] = ['zinc-dark', 'zinc-light', 'latte', 'frappe', 'macchiato', 'mocha']

export function getTheme(setting: ThemeSetting | string, accent: CatAccent, systemDark: boolean): Theme {
  const id = setting === 'system' ? (systemDark ? 'zinc-dark' : 'zinc-light') : setting
  switch (id) {
    case 'zinc-light':
    case 'light':
      return zincTheme(false)
    case 'latte':
    case 'frappe':
    case 'macchiato':
    case 'mocha':
      return catTheme(id, accent)
    default:
      return zincTheme(true)
  }
}

export function catSwatch(id: ThemeId, accent: CatAccent): string | null {
  return id in CAT ? CAT[id as keyof typeof CAT][accent] : null
}
