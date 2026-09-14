import { clsx } from 'clsx'
import { twMerge } from 'tailwind-merge'

/** Merge tailwind classes (shadcn-style `cn` helper). */
export function cn(...inputs) {
  return twMerge(clsx(inputs))
}

/** Human-readable byte size, e.g. 1536 -> "1.5 KB". */
export function formatBytes(n) {
  if (!Number.isFinite(n) || n <= 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  const i = Math.min(Math.floor(Math.log(n) / Math.log(1024)), units.length - 1)
  const v = n / 1024 ** i
  return `${v >= 100 ? Math.round(v) : v.toFixed(1)} ${units[i]}`
}

/** Seconds -> "1m 23s" / "32s" / "2h 5m". */
export function formatDuration(secs) {
  if (!Number.isFinite(secs) || secs < 0) return '—'
  const s = Math.floor(secs)
  if (s < 60) return `${s}s`
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}m ${s % 60}s`
  const h = Math.floor(m / 60)
  return `${h}h ${m % 60}m`
}