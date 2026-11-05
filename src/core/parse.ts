import PostalMime from 'postal-mime'
import type { Body } from './db'

const INLINE_MAX = 2 * 1024 * 1024

function toBase64(content: ArrayBuffer | Uint8Array | string): string {
  if (typeof content === 'string') return Buffer.from(content).toString('base64')
  return Buffer.from(content instanceof Uint8Array ? content : new Uint8Array(content)).toString('base64')
}

export function htmlToText(html: string): string {
  return html
    .replace(/<(style|script|head)[\s\S]*?<\/\1>/gi, ' ')
    .replace(/<br\s*\/?>|<\/(p|div|tr|li|h\d)>/gi, '\n')
    .replace(/<[^>]+>/g, ' ')
    .replace(/&nbsp;/gi, ' ')
    .replace(/&amp;/gi, '&')
    .replace(/&lt;/gi, '<')
    .replace(/&gt;/gi, '>')
    .replace(/&quot;/gi, '"')
    .replace(/&#39;/gi, "'")
}

export function makeSnippet(text: string): string {
  return text
    .split('\n')
    .filter((l) => !l.trimStart().startsWith('>'))
    .join(' ')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, 200)
}

export async function parseSource(source: Buffer): Promise<Body> {
  const email = await PostalMime.parse(source, { attachmentEncoding: 'arraybuffer' })
  let html = email.html ?? null
  const attachments: Body['attachments'] = []
  for (const a of email.attachments) {
    const size = typeof a.content === 'string' ? a.content.length : a.content.byteLength
    const cid = a.contentId?.replace(/^<|>$/g, '')
    if (html && cid && html.includes(`cid:${cid}`) && size <= INLINE_MAX) {
      // Inline images become data URLs so the CSP can block everything remote.
      html = html.split(`cid:${cid}`).join(`data:${a.mimeType};base64,${toBase64(a.content)}`)
      continue
    }
    attachments.push({ filename: a.filename ?? 'attachment', mimeType: a.mimeType, size })
  }
  const text = email.text ?? (html ? htmlToText(html) : null)
  return { html, text, attachments, snippet: makeSnippet(text ?? '') }
}
