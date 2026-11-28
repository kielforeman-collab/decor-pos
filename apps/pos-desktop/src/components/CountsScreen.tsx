// Physical Counts — start counts, scan counted quantities with the
// product picker, and reconcile variances to the inventory ledger.

import { useCallback, useEffect, useState } from 'react'
import { listCounts, reconcileCount, scanCount, startCount } from '../backofficeApi'
import type { PhysicalCount } from '../backofficeTypes'
import { fmtQty, type ProductSearchResult } from '../types'
import { ProductPicker } from './ScreenBits'

export default function CountsScreen({
  storeId,
  onToast,
}: {
  storeId: string | null
  onToast: (msg: string) => void
}) {
  const [counts, setCounts] = useState<PhysicalCount[]>([])
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [picked, setPicked] = useState<ProductSearchResult | null>(null)
  const [countedQty, setCountedQty] = useState('')
  const [loading, setLoading] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const selected = counts.find((c) => c.id === selectedId) ?? null

  const load = useCallback(async () => {
    if (!storeId) return
    setLoading(true)
    setError(null)
    try {
      const list = await listCounts(storeId)
      setCounts(list)
      setSelectedId((prev) => (prev && list.some((c) => c.id === prev) ? prev : list[0]?.id ?? null))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [storeId])

  useEffect(() => {
    void load()
  }, [load])

  async function doStart() {
    if (!storeId) return
    setBusy(true)
    setError(null)
    try {
      const created = await startCount(storeId)
      onToast(`Physical count ${created.id.slice(0, 8)} started.`)
      await load()
      setSelectedId(created.id)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  async function doScan() {
    if (!selected || !picked) {
      setError('Pick a product to scan.')
      return
    }
    if (!countedQty.trim() || Number.isNaN(Number(countedQty))) {
      setError('Enter a valid counted quantity.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const updated = await scanCount(selected.id, picked.product_id, countedQty.trim())
      setCounts((prev) => prev.map((c) => (c.id === updated.id ? updated : c)))
      setPicked(null)
      setCountedQty('')
      onToast(`Recorded ${picked.sku} = ${countedQty.trim()}.`)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  async function doReconcile() {
    if (!selected) return
    setBusy(true)
    setError(null)
    try {
      const res = await reconcileCount(selected.id)
      onToast(`Count reconciled (${res.adjustments.length} ledger adjustment(s)).`)
      await load()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  if (!storeId) {
    return (
      <section className="screen">
        <p className="screen-empty">Loading store…</p>
      </section>
    )
  }

  return (
    <section className="screen" aria-label="Physical counts">
      <div className="screen-head">
        <h2>Physical counts</h2>
        <div className="screen-actions">
          <button type="button" className="screen-btn" disabled={loading} onClick={() => void load()}>
            {loading ? 'Loading…' : 'Refresh'}
          </button>
          <button type="button" className="screen-btn screen-btn-primary" disabled={busy} onClick={() => void doStart()}>
            + Start count
          </button>
        </div>
      </div>

      {error && (
        <div className="error" role="alert">
          <span>{error}</span>
          <button type="button" className="banner-x" aria-label="Dismiss error" onClick={() => setError(null)}>✕</button>
        </div>
      )}

      <div className="screen-split">
        <div className="screen-panel">
          <h3>Counts ({counts.length})</h3>
          {counts.length === 0 ? (
            <p className="screen-empty">No physical counts yet — start one above.</p>
          ) : (
            <table className="screen-table screen-table-clickable">
              <thead>
                <tr>
                  <th>Count</th>
                  <th>Status</th>
                  <th className="num">Lines</th>
                  <th>Created</th>
                </tr>
              </thead>
              <tbody>
                {counts.map((c) => (
                  <tr
                    key={c.id}
                    className={selected?.id === c.id ? 'selected' : ''}
                    onClick={() => setSelectedId(c.id)}
                  >
                    <td className="mono">{c.id.slice(0, 8)}</td>
                    <td>{c.status}</td>
                    <td className="num">{c.lines.length}</td>
                    <td className="mono">{c.created_at.slice(0, 10)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>

        <div className="screen-panel">
          <h3>Count detail</h3>
          {!selected ? (
            <p className="screen-empty">Pick a count on the left.</p>
          ) : (
            <>
              <dl className="screen-facts">
                <div><dt>ID</dt><dd className="mono">{selected.id.slice(0, 8)}…</dd></div>
                <div><dt>Status</dt><dd>{selected.status}</dd></div>
                <div><dt>Scanned lines</dt><dd className="mono">{selected.lines.length}</dd></div>
                <div><dt>Reconciled at</dt><dd className="mono">{selected.counted_at ? selected.counted_at.slice(0, 19).replace('T', ' ') : '—'}</dd></div>
              </dl>

              {selected.status === 'open' && (
                <div className="screen-sub">
                  <h4>Scan / record counted quantity</h4>
                  <ProductPicker
                    onAdd={(p) => {
                      setPicked(p)
                      setCountedQty(p.on_hand ?? '0')
                    }}
                    placeholder="Search product by SKU or name to count…"
                  />
                  {picked && (
                    <div className="screen-form-row">
                      <label>
                        Product
                        <input className="screen-input mono" value={`${picked.sku} — ${picked.name}`} disabled />
                      </label>
                      <label>
                        Counted qty
                        <input
                          className="screen-input screen-qty"
                          inputMode="decimal"
                          value={countedQty}
                          onChange={(e) => setCountedQty(e.target.value)}
                        />
                      </label>
                      <button type="button" className="screen-btn screen-btn-primary" disabled={busy} onClick={() => void doScan()}>
                        Record scan
                      </button>
                    </div>
                  )}
                  <div className="screen-actions">
                    <button
                      type="button"
                      className="screen-btn screen-btn-primary"
                      disabled={busy || selected.lines.length === 0}
                      onClick={() => void doReconcile()}
                    >
                      Reconcile count &amp; post adjustments
                    </button>
                  </div>
                </div>
              )}

              <div className="screen-sub">
                <h4>Lines</h4>
                {selected.lines.length === 0 ? (
                  <p className="screen-empty">No lines scanned yet.</p>
                ) : (
                  <table className="screen-table">
                    <thead>
                      <tr>
                        <th>Product ID</th>
                        <th className="num">System</th>
                        <th className="num">Counted</th>
                        <th className="num">Variance</th>
                      </tr>
                    </thead>
                    <tbody>
                      {selected.lines.map((l) => (
                        <tr key={l.product_id}>
                          <td className="mono">{l.product_id.slice(0, 8)}…</td>
                          <td className="num">{fmtQty(l.system_qty)}</td>
                          <td className="num">{fmtQty(l.counted_qty)}</td>
                          <td className="num">{fmtQty(l.variance_qty)}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </div>

              {selected.adjustments.length > 0 && (
                <div className="screen-sub">
                  <h4>Posted adjustments</h4>
                  <table className="screen-table">
                    <thead>
                      <tr>
                        <th>Product ID</th>
                        <th className="num">Before</th>
                        <th className="num">Counted</th>
                        <th className="num">Adjustment</th>
                      </tr>
                    </thead>
                    <tbody>
                      {selected.adjustments.map((a) => (
                        <tr key={a.product_id}>
                          <td className="mono">{a.product_id.slice(0, 8)}…</td>
                          <td className="num">{fmtQty(a.on_hand_before)}</td>
                          <td className="num">{fmtQty(a.counted_qty)}</td>
                          <td className="num">{fmtQty(a.adjustment)}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </>
          )}
        </div>
      </div>
    </section>
  )
}
