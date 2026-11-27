// Transfers — move stock between stores. Ports the back-office
// 'transfers' tab: list, create (destination picker + product
// search), ship, receive. The back-office's hand-typed store UUIDs
// are replaced with a picker fed by the new GET /stores route.
// A single-store deployment has nowhere to transfer to, so
// creation is disabled with an explanation instead of failing.

import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  createTransfer,
  inventoryBalances,
  listStores,
  listTransfers,
  receiveTransfer,
  shipTransfer,
  type Store,
  type Transfer,
} from '../api'
import { fmtQty } from '../types'
import { LineEditor, ProductPicker, newDraftLine, type DraftLine } from './ScreenBits'

const TRANSFER_STATUS_LABELS: Record<string, string> = {
  draft: 'Draft',
  shipped: 'Shipped',
  received: 'Received',
  cancelled: 'Cancelled',
}

function shortId(id: string): string {
  return id.slice(0, 8)
}

export default function TransfersScreen({
  storeId,
  onToast,
}: {
  storeId: string | null
  onToast: (msg: string) => void
}) {
  const [stores, setStores] = useState<Store[]>([])
  const [transfers, setTransfers] = useState<Transfer[]>([])
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [names, setNames] = useState<Record<string, { sku: string; name: string; on_hand: string; standard_cost: string }>>({})
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const [creating, setCreating] = useState(false)
  const [destId, setDestId] = useState('')
  const [lines, setLines] = useState<DraftLine[]>([])

  const selected = transfers.find((t) => t.id === selectedId) ?? null

  const storeById = useMemo(() => {
    const m = new Map<string, Store>()
    stores.forEach((s) => m.set(s.id, s))
    return m
  }, [stores])

  const destinations = useMemo(() => stores.filter((s) => s.id !== storeId), [stores, storeId])
  const singleStore = stores.length <= 1

  const load = useCallback(async () => {
    if (!storeId) return
    setLoading(true)
    setError(null)
    try {
      const [storeList, transferList, balances] = await Promise.all([
        listStores(),
        listTransfers(storeId),
        inventoryBalances(storeId),
      ])
      setStores(storeList)
      setTransfers(transferList)
      if (selectedId && !transferList.some((t) => t.id === selectedId)) setSelectedId(null)
      const byId: Record<string, { sku: string; name: string; on_hand: string; standard_cost: string }> = {}
      balances.forEach((b) => {
        byId[b.product_id] = { sku: b.sku, name: b.name, on_hand: b.on_hand, standard_cost: b.standard_cost }
      })
      setNames(byId)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [storeId, selectedId])

  useEffect(() => {
    void load()
  }, [load])

  useEffect(() => {
    if (destinations.length > 0 && !destId) setDestId(destinations[0].id)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [destinations.length])

  const fail = (e: unknown) => setError(e instanceof Error ? e.message : String(e))

  function storeLabel(id: string): string {
    const s = storeById.get(id)
    return s ? `${s.code} — ${s.name}` : id.slice(0, 8)
  }

  async function create() {
    if (!storeId) return
    if (!destId || destId === storeId) {
      setError('Pick a different destination store.')
      return
    }
    const valid = lines.filter((l) => Number(l.qty) > 0)
    if (valid.length === 0) {
      setError('Add at least one line with a quantity above zero.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const t = await createTransfer(
        storeId,
        destId,
        // The server re-derives unit cost from avg cost; the
        // balance's standard cost is the back-office's convention
        // for the request field.
        valid.map((l) => ({
          product_id: l.product_id,
          qty: l.qty,
          unit_cost: names[l.product_id]?.standard_cost ?? '0.0000',
        })),
      )
      setCreating(false)
      setLines([])
      await load()
      setSelectedId(t.id)
      onToast(`Transfer drafted to ${storeLabel(destId)}.`)
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doShip() {
    if (!selected) return
    setBusy(true)
    setError(null)
    try {
      const updated = await shipTransfer(selected.id)
      await load()
      setSelectedId(updated.id)
      onToast('Transfer shipped — stock left this store.')
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doReceive() {
    if (!selected) return
    setBusy(true)
    setError(null)
    try {
      const updated = await receiveTransfer(selected.id)
      await load()
      setSelectedId(updated.id)
      onToast('Transfer received — stock landed at the destination.')
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  if (!storeId) return <section className="screen"><p className="screen-empty">Loading store…</p></section>

  return (
    <section className="screen" aria-label="Transfers">
      <div className="screen-head">
        <h2>Transfers</h2>
        <div className="screen-actions">
          <button
            type="button"
            className="screen-btn screen-btn-primary"
            disabled={busy || singleStore}
            title={singleStore ? 'Only one store in this deployment — transfers need a second store.' : undefined}
            onClick={() => setCreating((c) => !c)}
          >
            {creating ? 'Close new transfer' : 'New transfer'}
          </button>
          <button type="button" className="screen-btn" disabled={loading} onClick={() => void load()}>
            Refresh
          </button>
        </div>
      </div>

      {error && (
        <div className="error" role="alert">
          <span>{error}</span>
          <button type="button" className="banner-x" aria-label="Dismiss error" onClick={() => setError(null)}>✕</button>
        </div>
      )}

      {singleStore && (
        <p className="screen-note">
          Only one store in this deployment — transfers need a second store, so new transfers are
          disabled. Add a store and they become available.
        </p>
      )}

      {creating && !singleStore && (
        <div className="screen-panel">
          <h3>New transfer — from {storeById.get(storeId)?.name ?? 'this store'}</h3>
          <div className="screen-form-row">
            <label>
              Destination store
              <select className="screen-input" value={destId} onChange={(e) => setDestId(e.target.value)}>
                {destinations.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.code} — {s.name}
                  </option>
                ))}
              </select>
            </label>
          </div>
          <ProductPicker
            onAdd={(p) => {
              setLines((prev) => {
                if (prev.some((l) => l.product_id === p.product_id)) return prev
                return [...prev, newDraftLine(p)]
              })
            }}
          />
          <LineEditor
            lines={lines}
            onChange={(key, patch) => setLines((prev) => prev.map((l) => (l.key === key ? { ...l, ...patch } : l)))}
            onRemove={(key) => setLines((prev) => prev.filter((l) => l.key !== key))}
          />
          <div className="screen-actions">
            <button type="button" className="screen-btn screen-btn-primary" disabled={busy} onClick={() => void create()}>
              Create draft transfer
            </button>
          </div>
        </div>
      )}

      <div className="screen-split">
        <div className="screen-panel">
          <h3>Transfers</h3>
          {loading && transfers.length === 0 ? (
            <p className="screen-empty">Loading…</p>
          ) : transfers.length === 0 ? (
            <p className="screen-empty">No transfers yet.</p>
          ) : (
            <table className="screen-table screen-table-clickable">
              <thead>
                <tr>
                  <th>From</th>
                  <th>To</th>
                  <th>Status</th>
                  <th>Created</th>
                </tr>
              </thead>
              <tbody>
                {transfers.map((t) => (
                  <tr
                    key={t.id}
                    className={t.id === selectedId ? 'selected' : ''}
                    onClick={() => setSelectedId(t.id)}
                  >
                    <td>{storeById.get(t.from_store_id)?.code ?? t.from_store_id.slice(0, 8)}</td>
                    <td>{storeById.get(t.to_store_id)?.code ?? t.to_store_id.slice(0, 8)}</td>
                    <td>{TRANSFER_STATUS_LABELS[t.status] ?? t.status}</td>
                    <td>{new Date(t.created_at).toLocaleDateString()}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>

        <div className="screen-panel">
          <h3>Transfer detail</h3>
          {!selected ? (
            <p className="screen-empty">Pick a transfer on the left.</p>
          ) : (
            <>
              <p className="screen-meta">
                <span className="mono">{shortId(selected.id)}</span> ·{' '}
                {TRANSFER_STATUS_LABELS[selected.status] ?? selected.status} · {storeLabel(selected.from_store_id)}{' '}
                → {storeLabel(selected.to_store_id)}
              </p>
              <table className="screen-table">
                <thead>
                  <tr>
                    <th>SKU</th>
                    <th>Product</th>
                    <th className="num">On hand here</th>
                    <th className="num">Qty</th>
                  </tr>
                </thead>
                <tbody>
                  {selected.lines.map((l, i) => {
                    const n = names[l.product_id]
                    return (
                      <tr key={`${l.product_id}-${i}`}>
                        <td className="mono">{n?.sku ?? l.product_id.slice(0, 8)}</td>
                        <td>{n?.name ?? 'Unknown product'}</td>
                        <td className="num">{n ? fmtQty(n.on_hand) : '—'}</td>
                        <td className="num">{fmtQty(l.qty)}</td>
                      </tr>
                    )
                  })}
                </tbody>
              </table>
              <div className="screen-actions">
                {selected.status === 'draft' && (
                  <button type="button" className="screen-btn screen-btn-primary" disabled={busy} onClick={() => void doShip()}>
                    Ship
                  </button>
                )}
                {selected.status === 'shipped' && (
                  <button type="button" className="screen-btn screen-btn-primary" disabled={busy} onClick={() => void doReceive()}>
                    Receive
                  </button>
                )}
                {selected.status === 'draft' && (
                  <span className="screen-note-inline">Shipping moves the stock out of this store.</span>
                )}
              </div>
            </>
          )}
        </div>
      </div>
    </section>
  )
}
