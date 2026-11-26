import { useCallback, useEffect, useState } from 'react'
import { DEMO_STORE_ID, getHealth } from './api'
import Driver from './Driver'
import Sales from './Sales'
import Warehouse from './Warehouse'
import { loadOutbox, replayOutbox } from './offline'

type Tab = 'warehouse' | 'sales' | 'driver'

const TABS: { id: Tab; label: string }[] = [
  { id: 'warehouse', label: 'Warehouse' },
  { id: 'sales', label: 'Outside Sales' },
  { id: 'driver', label: 'Delivery Driver' },
]

const STORE_KEY = 'decor-pos-mobile-store-id'

export default function App() {
  const [tab, setTab] = useState<Tab>('warehouse')
  const [online, setOnline] = useState(navigator.onLine)
  const [storeId, setStoreId] = useState(
    () => localStorage.getItem(STORE_KEY) ?? DEMO_STORE_ID,
  )
  const [pending, setPending] = useState(loadOutbox().length)
  const [health, setHealth] = useState<string | null>(null)
  const [syncMsg, setSyncMsg] = useState<string | null>(null)
  const [syncing, setSyncing] = useState(false)

  const refreshPending = useCallback(() => setPending(loadOutbox().length), [])

  useEffect(() => {
    const on = () => setOnline(true)
    const off = () => setOnline(false)
    window.addEventListener('online', on)
    window.addEventListener('offline', off)
    return () => {
      window.removeEventListener('online', on)
      window.removeEventListener('offline', off)
    }
  }, [])

  // Re-read the outbox count whenever the tab regains focus or a
  // mutation reports back (components call refreshPending via a
  // custom event they dispatch after mutate()).
  useEffect(() => {
    const handler = () => refreshPending()
    window.addEventListener('focus', handler)
    window.addEventListener('decor-pos-outbox', handler)
    return () => {
      window.removeEventListener('focus', handler)
      window.removeEventListener('decor-pos-outbox', handler)
    }
  }, [refreshPending])

  useEffect(() => {
    localStorage.setItem(STORE_KEY, storeId)
  }, [storeId])

  useEffect(() => {
    let cancelled = false
    getHealth()
      .then((h) => {
        if (!cancelled) setHealth(`edge ${h.status} · store ${h.store_id.slice(0, 8)}`)
      })
      .catch(() => {
        if (!cancelled) setHealth('edge unreachable')
      })
    return () => {
      cancelled = true
    }
  }, [])

  async function sync() {
    if (!navigator.onLine) {
      setSyncMsg('Still offline — nothing to sync yet.')
      return
    }
    const count = loadOutbox().length
    if (count === 0) {
      setSyncMsg('Outbox is empty — nothing to sync.')
      return
    }
    if (
      !window.confirm(
        `${count} queued mutation(s) will be replayed.\n\n` +
          'The server has NO idempotency for PODs, counts, PO receives or transfers: ' +
          'if a mutation actually reached the server before the connection dropped, ' +
          'replaying it will apply it TWICE. Only sync if these were queued while ' +
          'offline and never sent.\n\nReplay now?',
      )
    ) {
      return
    }
    setSyncing(true)
    setSyncMsg(null)
    try {
      const result = await replayOutbox()
      refreshPending()
      const parts: string[] = []
      if (result.sent.length) parts.push(`sent ${result.sent.length}`)
      if (result.failed.length) {
        parts.push(
          `${result.failed.length} failed: ${result.failed.map((f) => `${f.label} (${f.error})`).join('; ')}`,
        )
      }
      setSyncMsg(parts.join(' · ') || 'Outbox is empty.')
    } finally {
      setSyncing(false)
    }
  }

  return (
    <div>
      <header className="app-header">
        <h1>Decor POS — Mobile</h1>
        <div className="sub">{health ?? 'checking edge…'} · PWA v0.6.0</div>
      </header>

      {online ? (
        <div className="online-banner">Online — changes go straight to the edge.</div>
      ) : (
        <div className="offline-banner">
          OFFLINE — reads are unavailable and new changes queue in the outbox until you sync.
        </div>
      )}

      {pending > 0 && (
        <div className="outbox-bar">
          <span>
            Outbox: <span className="count">{pending}</span> pending
          </span>
          <button className="btn small" onClick={() => void sync()} disabled={syncing || !online}>
            {syncing ? 'Syncing…' : 'Sync now'}
          </button>
        </div>
      )}
      {syncMsg && (
        <div className="outbox-bar" style={{ background: '#f0fdf4', borderBottomColor: '#bbf7d0' }}>
          {syncMsg}
        </div>
      )}

      <nav className="tab-bar" aria-label="Role">
        {TABS.map((t) => (
          <button
            key={t.id}
            className={tab === t.id ? 'active' : ''}
            onClick={() => setTab(t.id)}
            aria-current={tab === t.id ? 'page' : undefined}
          >
            {t.label}
          </button>
        ))}
      </nav>

      <main>
        <section className="card">
          <label className="field" style={{ margin: 0 }}>
            Store id
            <input
              type="text"
              value={storeId}
              onChange={(e) => setStoreId(e.target.value)}
              className="mono"
            />
          </label>
        </section>
        {tab === 'warehouse' && <Warehouse storeId={storeId} />}
        {tab === 'sales' && <Sales storeId={storeId} />}
        {tab === 'driver' && <Driver />}
      </main>
    </div>
  )
}
