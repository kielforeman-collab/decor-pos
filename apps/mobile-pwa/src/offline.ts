// Offline outbox for the mobile PWA (Phase 5, v0.6).
//
// Mutations attempted while offline (or when the fetch itself fails
// at the network level) are stored in localStorage and replayed by
// the Sync button when the device is back online.
//
// HONEST LIMIT — the edge has NO server-side idempotency for these
// endpoints (idempotency keys exist only for portal bill-pay). A
// replayed POD, count scan, PO receive or transfer ship that
// actually reached the server before the connection dropped will be
// applied TWICE. The UI warns before replaying, and the outbox only
// ever holds mutations that were never confirmed sent.

export interface OutboxEntry {
  id: string
  label: string
  path: string
  body: unknown
  created_at: string
}

const KEY = 'decor-pos-mobile-outbox-v1'

export function loadOutbox(): OutboxEntry[] {
  try {
    const raw = localStorage.getItem(KEY)
    if (!raw) return []
    const parsed = JSON.parse(raw) as OutboxEntry[]
    return Array.isArray(parsed) ? parsed : []
  } catch {
    return []
  }
}

function saveOutbox(entries: OutboxEntry[]): void {
  localStorage.setItem(KEY, JSON.stringify(entries))
}

export function addToOutbox(label: string, path: string, body: unknown): OutboxEntry {
  const entry: OutboxEntry = {
    id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    label,
    path,
    body,
    created_at: new Date().toISOString(),
  }
  saveOutbox([...loadOutbox(), entry])
  return entry
}

export function removeFromOutbox(id: string): void {
  saveOutbox(loadOutbox().filter((e) => e.id !== id))
}

export function clearOutbox(): void {
  localStorage.removeItem(KEY)
}

export interface ReplayResult {
  sent: string[]
  failed: { label: string; error: string }[]
}

/** Replay every queued mutation in order. Successes are removed;
 * failures stay queued and the rest are still attempted. */
export async function replayOutbox(): Promise<ReplayResult> {
  const result: ReplayResult = { sent: [], failed: [] }
  for (const entry of loadOutbox()) {
    try {
      const res = await fetch(`/api${entry.path}`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(entry.body),
      })
      const json = await res.json().catch(() => null)
      if (!res.ok) {
        throw new Error(
          typeof json?.error === 'string' ? json.error : `HTTP ${res.status}`,
        )
      }
      removeFromOutbox(entry.id)
      result.sent.push(entry.label)
    } catch (err) {
      result.failed.push({
        label: entry.label,
        error: err instanceof Error ? err.message : String(err),
      })
    }
  }
  return result
}
