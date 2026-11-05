import { useEffect, useMemo, useRef, useState } from 'react'
import DOMPurify from 'dompurify'
import { Recolor } from './darken'
import { useThemeTokens } from './theme'
import { api } from './api'

interface Props {
  html: string | null
  text: string | null
  /** Recolour into the theme (otherwise show the email's own colours). */
  dark: boolean
  images: boolean
}

interface Palette {
  bg: string
  text: string
  link: string
}

const REMOTE = /(<img[^>]+src\s*=\s*["']?\s*https?:|url\(\s*["']?\s*https?:|background\s*=\s*["']?\s*https?:)/i

export function hasRemoteContent(html: string | null): boolean {
  return !!html && REMOTE.test(html)
}

function escapeHtml(s: string): string {
  return s.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]!)
}

function plainToHtml(text: string): string {
  const linked = escapeHtml(text).replace(/\bhttps?:\/\/[^\s<>"]+/g, (u) => `<a href="${u}">${u}</a>`)
  const quoted = linked
    .split('\n')
    .map((l) => (l.startsWith('&gt;') ? `<span class="q">${l}</span>` : l))
    .join('\n')
  return `<div class="plain">${quoted}</div>`
}

/** Keep the email's own <style> blocks (DOMPurify drops <head>); CSS can't run code and CSP blocks remote url(). */
function extractStyles(html: string): string {
  const out: string[] = []
  for (const m of html.matchAll(/<style[^>]*>([\s\S]*?)<\/style>/gi)) out.push(m[1].replace(/<\/?style/gi, '').replace(/@import[^;]+;/gi, ''))
  return out.join('\n')
}

function buildDoc(p: Props, pal: Palette): string {
  const csp = [
    "default-src 'none'",
    "style-src 'unsafe-inline'",
    `img-src data: ${p.images ? 'https: http:' : ''}`,
    'font-src data:',
  ].join('; ')
  const body = p.html
    ? DOMPurify.sanitize(p.html, {
        FORBID_TAGS: ['script', 'iframe', 'object', 'embed', 'form', 'input', 'button', 'textarea', 'select', 'base', 'meta', 'link', 'style'],
        FORBID_ATTR: ['srcset'],
      })
    : plainToHtml(p.text ?? '')
  const styles = p.html ? extractStyles(p.html) : ''
  const base = p.dark
    ? `html,body{background:${pal.bg};color:${pal.text}}a{color:${pal.link}}`
    : 'html,body{background:#fff;color:#1f2328}'
  return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${csp}">
<style>${base}
body{margin:0;padding:20px;font:14px/1.6 ui-sans-serif,system-ui,sans-serif;overflow-wrap:anywhere}
img{max-width:100%;height:auto}
.plain{white-space:pre-wrap}.q{opacity:.6}
</style><style>${styles}</style></head><body>${body}</body></html>`
}

/** Sandboxed, script-free email renderer. Hidden until recoloured, so nothing flashes white. */
export function MessageFrame(props: Props) {
  const ref = useRef<HTMLIFrameElement>(null)
  const [ready, setReady] = useState(false)
  const theme = useThemeTokens()
  const pal = { bg: theme.vars['mail-bg'], text: theme.vars['mail-text'], link: theme.vars.link }
  const srcDoc = useMemo(() => buildDoc(props, pal), [props.html, props.text, props.dark, props.images, pal.bg, pal.text, pal.link])

  useEffect(() => {
    setReady(false)
  }, [srcDoc])

  useEffect(() => {
    const frame = ref.current
    if (!frame) return
    let ro: ResizeObserver | null = null
    const onLoad = () => {
      const doc = frame.contentDocument
      if (!doc) return
      if (props.dark) new Recolor(pal).apply(doc)
      const fit = () => {
        frame.style.height = `${doc.documentElement.scrollHeight}px`
      }
      fit()
      ro = new ResizeObserver(fit)
      ro.observe(doc.documentElement)
      // Clicking into an email moves focus into the iframe; keep vim keys working there.
      doc.addEventListener('keydown', (e) => {
        const init = { key: e.key, code: e.code, ctrlKey: e.ctrlKey, shiftKey: e.shiftKey, altKey: e.altKey, metaKey: e.metaKey, cancelable: true }
        if (!window.dispatchEvent(new KeyboardEvent('keydown', init))) e.preventDefault()
      })
      doc.addEventListener('click', (e) => {
        const a = (e.target as Element).closest?.('a')
        if (!a) return
        e.preventDefault()
        const href = a.getAttribute('href')
        if (href && /^(https?|mailto):/i.test(href)) void api.openExternal(href)
      })
      setReady(true)
    }
    frame.addEventListener('load', onLoad)
    return () => {
      frame.removeEventListener('load', onLoad)
      ro?.disconnect()
    }
  }, [srcDoc])

  return <iframe ref={ref} className={ready ? 'ready' : ''} sandbox="allow-same-origin" srcDoc={srcDoc} title="message" />
}
