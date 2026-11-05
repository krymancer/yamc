// Dark Reader-style recolouring of an email document into the active theme.
// Neutral colours take the theme's own base and text; coloured ones keep their hue.
// Reads every computed colour first, then writes, so layout is recalculated once.

type RGBA = [number, number, number, number]
type HSL = [number, number, number]

export interface MailPalette {
  bg: string
  text: string
}

const SKIP = new Set(['IMG', 'PICTURE', 'VIDEO', 'SVG', 'svg', 'CANVAS', 'STYLE', 'HEAD', 'META', 'TITLE', 'BR'])
/** Below this chroma a colour counts as grey. HSL saturation can't be used: it explodes near white. */
const NEUTRAL = 0.12

function chroma([r, g, b]: RGBA): number {
  return (Math.max(r, g, b) - Math.min(r, g, b)) / 255
}

export function parseColor(s: string): RGBA | null {
  const hex = s.match(/^#([0-9a-f]{6})$/i)
  if (hex) {
    const n = parseInt(hex[1], 16)
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255, 1]
  }
  const m = s.match(/rgba?\(\s*([\d.]+)[,\s]+([\d.]+)[,\s]+([\d.]+)(?:[,\s/]+([\d.]+%?))?\s*\)/)
  if (!m) return null
  let a = m[4] === undefined ? 1 : parseFloat(m[4])
  if (m[4]?.endsWith('%')) a /= 100
  return [+m[1], +m[2], +m[3], a]
}

function toHsl([r, g, b]: RGBA): HSL {
  r /= 255
  g /= 255
  b /= 255
  const max = Math.max(r, g, b)
  const min = Math.min(r, g, b)
  const l = (max + min) / 2
  if (max === min) return [0, 0, l]
  const d = max - min
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min)
  const h = max === r ? (g - b) / d + (g < b ? 6 : 0) : max === g ? (b - r) / d + 2 : (r - g) / d + 4
  return [h / 6, s, l]
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v))

function css(h: number, s: number, l: number, a: number): string {
  return `hsla(${(h * 360).toFixed(1)}, ${(clamp(s, 0, 1) * 100).toFixed(1)}%, ${(clamp(l, 0, 1) * 100).toFixed(1)}%, ${a})`
}

export class Recolor {
  private bg: HSL
  private text: HSL

  constructor(p: MailPalette) {
    this.bg = toHsl(parseColor(p.bg)!)
    this.text = toHsl(parseColor(p.text)!)
  }

  /** Light backgrounds sink into the theme base; already-dark ones are left alone. */
  background(c: RGBA): string | null {
    if (c[3] === 0) return null
    const [h, s, l] = toHsl(c)
    if (l < 0.3) return null
    const [bh, bs, bl] = this.bg
    if (chroma(c) < NEUTRAL) return css(bh, bs, bl + (1 - l) * 0.35, c[3])
    return css(h, s * 0.55, clamp(bl + 0.05 + (1 - l) * 0.3, 0, 0.34), c[3])
  }

  /** Dark text rises to the theme text colour; already-light text is left alone. */
  foreground(c: RGBA): string | null {
    const [h, s, l] = toHsl(c)
    if (l > 0.62) return null
    const [th, ts, tl] = this.text
    if (chroma(c) < NEUTRAL) return css(th, ts, tl - l * 0.3, c[3])
    return css(h, Math.min(s, 0.8), 0.66 + ((0.62 - l) / 0.62) * 0.2, c[3])
  }

  border(c: RGBA): string | null {
    if (c[3] === 0) return null
    const [h, s, l] = toHsl(c)
    if (l < 0.4) return null
    const [bh, bs, bl] = this.bg
    return chroma(c) < NEUTRAL ? css(bh, bs, bl + 0.08 + (1 - l) * 0.1, c[3]) : css(h, s * 0.5, bl + 0.12, c[3])
  }

  apply(doc: Document): void {
    const els = [doc.documentElement, ...Array.from(doc.querySelectorAll<HTMLElement>('body, body *'))]
    const win = doc.defaultView!
    const writes: [HTMLElement, string, string][] = []
    for (const el of els) {
      if (SKIP.has(el.tagName) || el.closest('svg')) continue
      const cs = win.getComputedStyle(el)
      const bg = parseColor(cs.backgroundColor)
      const fg = parseColor(cs.color)
      const bd = cs.borderTopStyle !== 'none' || cs.borderBottomStyle !== 'none' ? parseColor(cs.borderTopColor) : null
      const nb = bg && this.background(bg)
      const nf = fg && this.foreground(fg)
      const nd = bd && this.border(bd)
      if (nb) writes.push([el, 'background-color', nb])
      if (nf) writes.push([el, 'color', nf])
      if (nd) writes.push([el, 'border-color', nd])
    }
    for (const [el, prop, value] of writes) el.style.setProperty(prop, value, 'important')
  }
}
