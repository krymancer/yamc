import type { Address } from '@shared/types'

const time = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' })
const day = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric' })
const full = new Intl.DateTimeFormat(undefined, { year: 'numeric', month: 'short', day: 'numeric' })
const long = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' })

export function shortDate(ms: number): string {
  const d = new Date(ms)
  const now = new Date()
  if (d.toDateString() === now.toDateString()) return time.format(d)
  if (d.getFullYear() === now.getFullYear()) return day.format(d)
  return full.format(d)
}

export const longDate = (ms: number) => long.format(new Date(ms))

export const who = (a: Address | null) => (a ? a.name || a.address : '(unknown)')

export const addrList = (list: Address[]) => list.map((a) => (a.name ? `${a.name} <${a.address}>` : a.address)).join(', ')
