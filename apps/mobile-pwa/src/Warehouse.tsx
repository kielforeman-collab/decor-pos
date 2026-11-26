import { useCallback, useEffect, useState } from 'react'
import {
  createTransfer,
  listCounts,
  listPurchaseOrders,
  listTransfers,
  receivePurchaseOrder,
  receiveTransfer,
  scanCount,
  reconcileCount,
  shipTransfer,
  startCount,
} from './api'
import { DEMO_CATALOG } from './catalog'
import type { PhysicalCount, PurchaseOrderView, Transfer } from './types'
import { fmtQty } from './types'

function useNotice() {
  const [msg, setMsg] = useState<{ kind: 'ok' | 'error'; text: string } | null>(null)
  const show = useCallback((kind: 'ok' | 'error', text: string) => {
    setMsg({ kind, text })
  }, [])
  const node = msg ? <div className={msg.kind}>{msg.text}</div> : null
  return { show, node }
}

export default function Warehouse({ storeId }: { storeId: string }) {
  return (
    <div>
      <PoReceive storeId={storeId} />
      <Counts storeId={storeId} />
      <Transfers storeId={storeId} />
    </div>
  )
}

function PoReceive({ storeId }: { storeId: string }) {
  const [pos, setPos] = useState<PurchaseOrderView[]>([])
  const [selectedId, setSelectedId] = useState('')
  const [qtys, setQtys] = useState<Record<string, string>>({})
  const [busy, setBusy] = useState(false)
  const { show, node } = useNotice()

  const refresh = useCallback(async () => {
    try {
      const list = await listPurchaseOrders(storeId)
      setPos(list)
      if (list.length && !selectedId) setSelectedId(list[0].id)
    } catch (err) {
      show('error', err instanceof Error ? err.message : String(err))
    }
  }, [storeId, selectedId, show])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const selected = pos.find((p) => p.id === selectedId)

  async function receive() {
    if (!selected) return
    const lines = selected.lines
      .map((l) => ({ product_id: l.product_id, qty: (qtys[l.product_id] ?? '').trim() }))
      .filter((l) => l.qty !== '' && Number(l.qty) > 0)
    if (!lines.length) {
      show('error', 'Enter a received quantity for at least one line.')
      return
    }
    setBusy(true)
    try {
      const res = await receivePurchaseOrder(selected.id, lines)
      if (res.queued) {
        show('ok', 'Offline — PO receive queued in the outbox. Sync when back online.')
      } else {
        show('ok', `PO ${res.data.po_number} received — status ${res.data.status}.`)
        setQtys({})
      }
      await refresh()
    } catch (err) {
      show('error', err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <section className="card">
      <h2>PO Receive</h2>
      {node}
      {pos.length === 0 ? (
        <p className="muted">
          No purchase orders for this store yet. Create one in the back office
          (Purchasing → create PO) and it appears here.
        </p>
      ) : (
        <>
          <label className="field">
            Purchase order
            <select value={selectedId} onChange={(e) => setSelectedId(e.target.value)}>
              {pos.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.po_number} — {p.status}
                </option>
              ))}
            </select>
          </label>
          {selected && (
            <table className="data">
              <thead>
                <tr>
                  <th>Line</th>
                  <th>Ordered</th>
                  <th>Received</th>
                  <th>Receive now</th>
                </tr>
              </thead>
              <tbody>
                {selected.lines.map((l) => (
                  <tr key={l.id}>
                    <td>
                      {l.sku}
                      <br />
                      <span className="muted">{l.description}</span>
                    </td>
                    <td>{fmtQty(l.qty)}</td>
                    <td>{fmtQty(l.received_qty)}</td>
                    <td>
                      <input
                        type="number"
                        min="0"
                        step="any"
                        inputMode="decimal"
                        style={{ width: 72 }}
                        value={qtys[l.product_id] ?? ''}
                        onChange={(e) =>
                          setQtys((q) => ({ ...q, [l.product_id]: e.target.value }))
                        }
                        aria-label={`Receive quantity for ${l.sku}`}
                      />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          <div className="row" style={{ marginTop: 10 }}>
            <button className="btn" onClick={() => void receive()} disabled={busy || !selected}>
              Receive
            </button>
            <button className="btn secondary" onClick={() => void refresh()}>
              Refresh
            </button>
          </div>
        </>
      )}
    </section>
  )
}

function Counts({ storeId }: { storeId: string }) {
  const [counts, setCounts] = useState<PhysicalCount[]>([])
  const [selectedId, setSelectedId] = useState('')
  const [productId, setProductId] = useState(DEMO_CATALOG[0].product_id)
  const [qty, setQty] = useState('')
  const [busy, setBusy] = useState(false)
  const { show, node } = useNotice()

  const refresh = useCallback(async () => {
    try {
      const list = await listCounts(storeId)
      setCounts(list)
      if (list.length && !selectedId) setSelectedId(list[0].id)
    } catch (err) {
      show('error', err instanceof Error ? err.message : String(err))
    }
  }, [storeId, selectedId, show])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const selected = counts.find((c) => c.id === selectedId)

  async function start() {
    setBusy(true)
    try {
      const res = await startCount(storeId)
      if (res.queued) {
        show('ok', 'Offline — new count queued in the outbox. Sync when back online.')
      } else {
        show('ok', `Count ${res.data.id.slice(0, 8)} started.`)
        setSelectedId(res.data.id)
      }
      await refresh()
    } catch (err) {
      show('error', err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  async function scan() {
    if (!selected || !qty.trim() || Number(qty) < 0) {
      show('error', 'Pick a count and enter a counted quantity.')
      return
    }
    setBusy(true)
    try {
      const res = await scanCount(selected.id, productId, qty.trim())
      if (res.queued) {
        show('ok', 'Offline — scan queued in the outbox. Sync when back online.')
      } else {
        show('ok', 'Scan recorded.')
        setQty('')
      }
      await refresh()
    } catch (err) {
      show('error', err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  async function reconcile() {
    if (!selected) return
    setBusy(true)
    try {
      const res = await reconcileCount(selected.id)
      if (res.queued) {
        show('ok', 'Offline — reconcile queued in the outbox. Sync when back online.')
      } else {
        const n = res.data.adjustments.length
        show('ok', `Count reconciled — ${n} adjustment${n === 1 ? '' : 's'} posted to on-hand.`)
      }
      await refresh()
    } catch (err) {
      show('error', err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <section className="card">
      <h2>Physical Counts</h2>
      {node}
      <div className="row">
        <button className="btn" onClick={() => void start()} disabled={busy}>
          Start new count
        </button>
        <button className="btn secondary" onClick={() => void refresh()}>
          Refresh
        </button>
      </div>
      {counts.length > 0 && (
        <>
          <label className="field">
            Count
            <select value={selectedId} onChange={(e) => setSelectedId(e.target.value)}>
              {counts.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.id.slice(0, 8)} — {c.status} ({c.lines.length} lines)
                </option>
              ))}
            </select>
          </label>
          {selected && selected.lines.length > 0 && (
            <table className="data">
              <thead>
                <tr>
                  <th>Product</th>
                  <th>System</th>
                  <th>Counted</th>
                  <th>Variance</th>
                </tr>
              </thead>
              <tbody>
                {selected.lines.map((l) => (
                  <tr key={l.product_id}>
                    <td className="mono">{l.product_id.slice(0, 8)}</td>
                    <td>{fmtQty(l.system_qty)}</td>
                    <td>{fmtQty(l.counted_qty)}</td>
                    <td>{fmtQty(l.variance_qty)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          <div className="row">
            <select
              value={productId}
              onChange={(e) => setProductId(e.target.value)}
              aria-label="Product to scan"
              style={{ flex: 1, minHeight: 44 }}
            >
              {DEMO_CATALOG.map((p) => (
                <option key={p.product_id} value={p.product_id}>
                  {p.sku} — {p.description}
                </option>
              ))}
            </select>
            <input
              type="number"
              min="0"
              step="any"
              inputMode="decimal"
              placeholder="Counted qty"
              style={{ width: 110, minHeight: 44 }}
              value={qty}
              onChange={(e) => setQty(e.target.value)}
              aria-label="Counted quantity"
            />
            <button className="btn" onClick={() => void scan()} disabled={busy || !selected}>
              Scan
            </button>
          </div>
          <div className="row" style={{ marginTop: 10 }}>
            <button
              className="btn secondary"
              onClick={() => void reconcile()}
              disabled={busy || !selected || selected.status !== 'open'}
            >
              Reconcile count
            </button>
            <span className="muted">Reconcile posts adjustments so on-hand equals counted.</span>
          </div>
        </>
      )}
    </section>
  )
}

function Transfers({ storeId }: { storeId: string }) {
  const [transfers, setTransfers] = useState<Transfer[]>([])
  const [toStoreId, setToStoreId] = useState('')
  const [productId, setProductId] = useState(DEMO_CATALOG[0].product_id)
  const [qty, setQty] = useState('')
  const [busy, setBusy] = useState(false)
  const { show, node } = useNotice()

  const refresh = useCallback(async () => {
    try {
      setTransfers(await listTransfers(storeId))
    } catch (err) {
      show('error', err instanceof Error ? err.message : String(err))
    }
  }, [storeId, show])

  useEffect(() => {
    void refresh()
  }, [refresh])

  async function create() {
    if (!toStoreId.trim()) {
      show('error', 'Enter the destination store id.')
      return
    }
    if (toStoreId.trim() === storeId) {
      show('error', 'A transfer needs two different stores.')
      return
    }
    if (!qty.trim() || Number(qty) <= 0) {
      show('error', 'Enter a positive quantity.')
      return
    }
    const product = DEMO_CATALOG.find((p) => p.product_id === productId)!
    setBusy(true)
    try {
      const res = await createTransfer({
        from_store_id: storeId,
        to_store_id: toStoreId.trim(),
        lines: [{ product_id: productId, qty: qty.trim(), unit_cost: product.unit_cost }],
      })
      if (res.queued) {
        show('ok', 'Offline — transfer queued in the outbox. Sync when back online.')
      } else {
        show('ok', `Transfer ${res.data.id.slice(0, 8)} created (draft).`)
        setQty('')
      }
      await refresh()
    } catch (err) {
      show('error', err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  async function ship(id: string) {
    setBusy(true)
    try {
      const res = await shipTransfer(id)
      show('ok', res.queued ? 'Offline — ship queued in the outbox.' : `Transfer shipped — status ${res.data.status}.`)
      await refresh()
    } catch (err) {
      show('error', err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  async function receive(id: string) {
    setBusy(true)
    try {
      const res = await receiveTransfer(id)
      show('ok', res.queued ? 'Offline — receive queued in the outbox.' : `Transfer received — status ${res.data.status}.`)
      await refresh()
    } catch (err) {
      show('error', err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <section className="card">
      <h2>Transfers</h2>
      {node}
      <label className="field">
        Destination store id
        <input
          type="text"
          inputMode="text"
          placeholder="00000000-0000-0000-0000-000000000000"
          value={toStoreId}
          onChange={(e) => setToStoreId(e.target.value)}
        />
      </label>
      <div className="row">
        <select
          value={productId}
          onChange={(e) => setProductId(e.target.value)}
          aria-label="Product to transfer"
          style={{ flex: 1, minHeight: 44 }}
        >
          {DEMO_CATALOG.map((p) => (
            <option key={p.product_id} value={p.product_id}>
              {p.sku} — {p.description}
            </option>
          ))}
        </select>
        <input
          type="number"
          min="0"
          step="any"
          inputMode="decimal"
          placeholder="Qty"
          style={{ width: 90, minHeight: 44 }}
          value={qty}
          onChange={(e) => setQty(e.target.value)}
          aria-label="Transfer quantity"
        />
        <button className="btn" onClick={() => void create()} disabled={busy}>
          Create
        </button>
      </div>
      {transfers.length === 0 ? (
        <p className="muted">No transfers for this store yet.</p>
      ) : (
        transfers.map((t) => (
          <div className="list-item" key={t.id}>
            <div className="row">
              <span className="title mono">{t.id.slice(0, 8)}</span>
              <span className={`status ${t.status}`}>{t.status}</span>
            </div>
            <div className="muted">
              {t.from_store_id.slice(0, 8)} → {t.to_store_id.slice(0, 8)} ·{' '}
              {t.lines.map((l) => `${fmtQty(l.qty)} × ${l.product_id.slice(0, 8)}`).join(', ')}
            </div>
            <div className="row" style={{ marginTop: 6 }}>
              {t.status === 'draft' && (
                <button className="btn small" onClick={() => void ship(t.id)} disabled={busy}>
                  Ship
                </button>
              )}
              {t.status === 'in_transit' && (
                <button className="btn small" onClick={() => void receive(t.id)} disabled={busy}>
                  Receive
                </button>
              )}
            </div>
          </div>
        ))
      )}
      <div className="row" style={{ marginTop: 10 }}>
        <button className="btn secondary" onClick={() => void refresh()}>
          Refresh
        </button>
      </div>
    </section>
  )
}
