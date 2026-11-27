// Orders — purchase orders. Ports the back-office 'purchasing'
// tab: replenishment suggestions, PO list, create (manual or from
// a suggestion), receive per line, and the EDI send / documents /
// 3-way match. The back-office's raw UUID fields and paste-the-X12
// boxes are replaced with pickers; the intake, catalog-import,
// price-application and transport screens stay out (they need
// pasted documents, not a counter flow).

import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  createPurchaseOrder,
  listEdiDocuments,
  listPoAcknowledgments,
  listPurchaseOrders,
  receivePurchaseOrder,
  sendPurchaseOrder,
  suggestReplenishment,
  threeWayMatch,
  type EdiDocumentRow,
  type InvoiceMatchReport,
  type PoAcknowledgmentRow,
  type PurchaseOrderView,
  type SuggestedPurchaseOrder,
} from '../api'
import { cad, fmtMoney, fmtQty } from '../types'
import { LineEditor, ProductPicker, newDraftLine, type DraftLine } from './ScreenBits'

const PO_STATUS_LABELS: Record<string, string> = {
  draft: 'Draft',
  sent: 'Sent',
  partially_received: 'Partially received',
  received: 'Received',
  cancelled: 'Cancelled',
}

function poLabel(status: string): string {
  return PO_STATUS_LABELS[status] ?? status
}

const EDI_TYPE_LABELS: Record<string, string> = {
  Po850: '850 PO',
  Ack855: '855 ack',
  Asn856: '856 ASN',
  Invoice810: '810 invoice',
  Catalog832: '832 catalog',
}

export default function OrdersScreen({
  storeId,
  onToast,
}: {
  storeId: string | null
  onToast: (msg: string) => void
}) {
  const [pos, setPos] = useState<PurchaseOrderView[]>([])
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const [suggestions, setSuggestions] = useState<SuggestedPurchaseOrder[] | null>(null)
  const [suggesting, setSuggesting] = useState(false)

  const [creating, setCreating] = useState(false)
  const [vendorId, setVendorId] = useState('')
  const [vendorOther, setVendorOther] = useState(false)
  const [lines, setLines] = useState<DraftLine[]>([])

  const [receiveQty, setReceiveQty] = useState<Record<string, string>>({})
  const [busy, setBusy] = useState(false)

  const [ediDocs, setEdiDocs] = useState<EdiDocumentRow[] | null>(null)
  const [ediAcks, setEdiAcks] = useState<PoAcknowledgmentRow[] | null>(null)
  const [match, setMatch] = useState<InvoiceMatchReport | null>(null)
  const [ediBusy, setEdiBusy] = useState(false)

  const selected = pos.find((p) => p.id === selectedId) ?? null

  const knownVendors = useMemo(() => {
    const ids = new Set<string>()
    pos.forEach((p) => ids.add(p.vendor_id))
    ;(suggestions ?? []).forEach((s) => ids.add(s.vendor_id))
    return [...ids].sort()
  }, [pos, suggestions])

  const load = useCallback(async () => {
    if (!storeId) return
    setLoading(true)
    setError(null)
    try {
      const list = await listPurchaseOrders(storeId)
      setPos(list)
      if (selectedId && !list.some((p) => p.id === selectedId)) setSelectedId(null)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [storeId, selectedId])

  useEffect(() => {
    void load()
  }, [load])

  // Reset the per-line receive inputs whenever the selection changes.
  useEffect(() => {
    if (!selected) {
      setReceiveQty({})
      setEdiDocs(null)
      setEdiAcks(null)
      setMatch(null)
      return
    }
    const next: Record<string, string> = {}
    selected.lines.forEach((l) => {
      const remaining = Number(l.qty) - Number(l.received_qty)
      if (remaining > 0) next[l.product_id] = String(remaining)
    })
    setReceiveQty(next)
    setEdiDocs(null)
    setEdiAcks(null)
    setMatch(null)
  }, [selected])

  const fail = (e: unknown) => setError(e instanceof Error ? e.message : String(e))

  async function doSuggest() {
    if (!storeId) return
    setSuggesting(true)
    setError(null)
    try {
      const list = await suggestReplenishment(storeId)
      setSuggestions(list)
      if (list.length === 0) onToast('Nothing to replenish — no reorder points are below stock.')
    } catch (e) {
      fail(e)
    } finally {
      setSuggesting(false)
    }
  }

  async function createFromSuggestions(po: SuggestedPurchaseOrder) {
    if (!storeId) return
    setBusy(true)
    setError(null)
    try {
      const created = await createPurchaseOrder(
        po.vendor_id,
        storeId,
        po.lines.map((l) => ({ product_id: l.product_id, qty: l.qty })),
      )
      setSuggestions((prev) => (prev ?? []).filter((s) => s.po_number !== po.po_number))
      await load()
      setSelectedId(created.id)
      onToast(`Purchase order created for ${po.vendor_id} — ${created.lines.length} lines.`)
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function createManual() {
    const vendor = vendorOther ? vendorId.trim() : vendorId
    if (!vendor) {
      setError('Pick a vendor first.')
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
      const po = await createPurchaseOrder(
        vendor,
        storeId,
        valid.map((l) => ({
          product_id: l.product_id,
          qty: l.qty,
          ...(l.unit_cost ? { unit_cost: l.unit_cost } : {}),
        })),
      )
      setCreating(false)
      setLines([])
      setVendorId('')
      setVendorOther(false)
      await load()
      setSelectedId(po.id)
      onToast(`Purchase order created — ${po.lines.length} lines.`)
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doReceive() {
    if (!selected) return
    const toReceive = Object.entries(receiveQty)
      .filter(([, qty]) => Number(qty) > 0)
      .map(([product_id, qty]) => ({ product_id, qty }))
    if (toReceive.length === 0) {
      setError('Enter a quantity above zero on at least one line.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const updated = await receivePurchaseOrder(selected.id, toReceive)
      await load()
      setSelectedId(updated.id)
      onToast(`Received — PO is now ${poLabel(updated.status)}.`)
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doSend() {
    if (!selected) return
    setBusy(true)
    setError(null)
    try {
      const res = await sendPurchaseOrder(selected.id)
      await load()
      setSelectedId(selected.id)
      onToast(res.path ? `850 sent — file at ${res.path}.` : '850 recorded in the EDI log (no file written).')
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function loadEdi() {
    if (!selected) return
    setEdiBusy(true)
    setError(null)
    try {
      const [docs, acks] = await Promise.all([
        listEdiDocuments(selected.id),
        listPoAcknowledgments(selected.id),
      ])
      setEdiDocs(docs)
      setEdiAcks(acks)
    } catch (e) {
      fail(e)
    } finally {
      setEdiBusy(false)
    }
  }

  async function doMatch() {
    if (!selected) return
    setEdiBusy(true)
    setError(null)
    try {
      setMatch(await threeWayMatch(selected.id))
    } catch (e) {
      fail(e)
    } finally {
      setEdiBusy(false)
    }
  }

  if (!storeId) return <section className="screen"><p className="screen-empty">Loading store…</p></section>

  const canReceive = selected && selected.status !== 'received' && selected.status !== 'cancelled'

  return (
    <section className="screen" aria-label="Orders">
      <div className="screen-head">
        <h2>Orders</h2>
        <div className="screen-actions">
          <button type="button" className="screen-btn" disabled={suggesting || busy} onClick={() => void doSuggest()}>
            {suggesting ? 'Checking stock…' : 'Suggest replenishment'}
          </button>
          <button
            type="button"
            className="screen-btn screen-btn-primary"
            disabled={busy}
            onClick={() => {
              setCreating((c) => !c)
              if (!creating && knownVendors.length > 0 && !vendorId) setVendorId(knownVendors[0])
            }}
          >
            {creating ? 'Close new order' : 'New purchase order'}
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

      {creating && (
        <div className="screen-panel">
          <h3>New purchase order</h3>
          <div className="screen-form-row">
            <label>
              Vendor
              {knownVendors.length > 0 && !vendorOther ? (
                <select
                  className="screen-input"
                  value={vendorId}
                  onChange={(e) => {
                    if (e.target.value === '__other') {
                      setVendorOther(true)
                      setVendorId('')
                    } else {
                      setVendorId(e.target.value)
                    }
                  }}
                >
                  {knownVendors.map((v) => (
                    <option key={v} value={v}>{v}</option>
                  ))}
                  <option value="__other">Other vendor…</option>
                </select>
              ) : (
                <input
                  type="text"
                  className="screen-input"
                  value={vendorId}
                  placeholder="Vendor id"
                  onChange={(e) => setVendorId(e.target.value)}
                />
              )}
            </label>
            {knownVendors.length > 0 && (
              <button
                type="button"
                className="screen-btn screen-btn-ghost"
                onClick={() => {
                  setVendorOther((o) => !o)
                  setVendorId('')
                }}
              >
                {vendorOther ? 'Use known vendor' : 'Other vendor…'}
              </button>
            )}
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
            showCost
            onChange={(key, patch) => setLines((prev) => prev.map((l) => (l.key === key ? { ...l, ...patch } : l)))}
            onRemove={(key) => setLines((prev) => prev.filter((l) => l.key !== key))}
          />
          <div className="screen-actions">
            <button type="button" className="screen-btn screen-btn-primary" disabled={busy} onClick={() => void createManual()}>
              Create draft PO
            </button>
          </div>
        </div>
      )}

      {suggestions !== null && (
        <div className="screen-panel">
          <h3>Replenishment suggestions</h3>
          {suggestions.length === 0 ? (
            <p className="screen-empty">Nothing below its reorder point.</p>
          ) : (
            suggestions.map((po) => (
              <div key={po.po_number} className="screen-group">
                <div className="screen-group-head">
                  <span className="mono">{po.vendor_id}</span>
                  <span>
                    {po.lines.length} item{po.lines.length === 1 ? '' : 's'} · preview {po.po_number}
                  </span>
                  <button
                    type="button"
                    className="screen-btn screen-btn-primary"
                    disabled={busy}
                    onClick={() => void createFromSuggestions(po)}
                  >
                    Create PO
                  </button>
                </div>
                <table className="screen-table">
                  <thead>
                    <tr>
                      <th>SKU</th>
                      <th>Product</th>
                      <th className="num">On hand</th>
                      <th className="num">Reorder at</th>
                      <th className="num">Suggested</th>
                    </tr>
                  </thead>
                  <tbody>
                    {po.lines.map((s) => (
                      <tr key={s.product_id}>
                        <td className="mono">{s.sku}</td>
                        <td>{s.description}</td>
                        <td className="num">{fmtQty(s.on_hand)}</td>
                        <td className="num">{fmtQty(s.min_qty)}</td>
                        <td className="num">{fmtQty(s.qty)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            ))
          )}
          <div className="screen-actions">
            <button type="button" className="screen-btn screen-btn-ghost" onClick={() => setSuggestions(null)}>
              Dismiss suggestions
            </button>
          </div>
        </div>
      )}

      <div className="screen-split">
        <div className="screen-panel">
          <h3>Purchase orders</h3>
          {loading && pos.length === 0 ? (
            <p className="screen-empty">Loading…</p>
          ) : pos.length === 0 ? (
            <p className="screen-empty">No purchase orders yet.</p>
          ) : (
            <table className="screen-table screen-table-clickable">
              <thead>
                <tr>
                  <th>PO</th>
                  <th>Vendor</th>
                  <th>Status</th>
                  <th>Lines</th>
                  <th>Created</th>
                </tr>
              </thead>
              <tbody>
                {pos.map((p) => (
                  <tr
                    key={p.id}
                    className={p.id === selectedId ? 'selected' : ''}
                    onClick={() => setSelectedId(p.id)}
                  >
                    <td className="mono">{p.po_number}</td>
                    <td className="mono">{p.vendor_id}</td>
                    <td>{poLabel(p.status)}</td>
                    <td className="num">{p.lines.length}</td>
                    <td>{new Date(p.created_at).toLocaleDateString()}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>

        <div className="screen-panel">
          <h3>Order detail</h3>
          {!selected ? (
            <p className="screen-empty">Pick an order on the left.</p>
          ) : (
            <>
              <p className="screen-meta">
                PO <span className="mono">{selected.po_number}</span> · {poLabel(selected.status)} · Vendor{' '}
                <span className="mono">{selected.vendor_id}</span> · Total {fmtMoney(cad(selected.total))}
              </p>
              <table className="screen-table">
                <thead>
                  <tr>
                    <th>SKU</th>
                    <th>Product</th>
                    <th className="num">Ordered</th>
                    <th className="num">Received</th>
                    <th className="num">Receive now</th>
                  </tr>
                </thead>
                <tbody>
                  {selected.lines.map((l) => {
                    const remaining = Number(l.qty) - Number(l.received_qty)
                    return (
                      <tr key={l.id}>
                        <td className="mono">{l.sku}</td>
                        <td>{l.description}</td>
                        <td className="num">{fmtQty(l.qty)}</td>
                        <td className="num">{fmtQty(l.received_qty)}</td>
                        <td className="num">
                          {canReceive && remaining > 0 ? (
                            <input
                              type="text"
                              inputMode="decimal"
                              className="screen-input screen-qty"
                              value={receiveQty[l.product_id] ?? ''}
                              aria-label={`Receive quantity for ${l.sku}`}
                              onChange={(e) =>
                                setReceiveQty((prev) => ({ ...prev, [l.product_id]: e.target.value }))
                              }
                            />
                          ) : (
                            '—'
                          )}
                        </td>
                      </tr>
                    )
                  })}
                </tbody>
              </table>
              <div className="screen-actions">
                {canReceive && (
                  <button type="button" className="screen-btn screen-btn-primary" disabled={busy} onClick={() => void doReceive()}>
                    Receive
                  </button>
                )}
                {selected.status === 'draft' && (
                  <button type="button" className="screen-btn" disabled={busy} onClick={() => void doSend()}>
                    Send as X12 850
                  </button>
                )}
                <button type="button" className="screen-btn" disabled={ediBusy} onClick={() => void loadEdi()}>
                  EDI documents
                </button>
                <button type="button" className="screen-btn" disabled={ediBusy} onClick={() => void doMatch()}>
                  3-way match
                </button>
              </div>

              {ediDocs !== null && ediAcks !== null && (
                <div className="screen-sub">
                  <h4>EDI documents</h4>
                  {ediDocs.length === 0 && ediAcks.length === 0 ? (
                    <p className="screen-empty">No EDI documents for this order yet.</p>
                  ) : (
                    <>
                      {ediDocs.length > 0 && (
                        <table className="screen-table">
                          <thead>
                            <tr>
                              <th>Type</th>
                              <th>Direction</th>
                              <th>Received</th>
                            </tr>
                          </thead>
                          <tbody>
                            {ediDocs.map((d) => (
                              <tr key={d.id}>
                                <td>{EDI_TYPE_LABELS[d.doc_type] ?? d.doc_type}</td>
                                <td>{d.direction === 'Outbound' ? 'Out' : 'In'}</td>
                                <td>{new Date(d.received_at).toLocaleString()}</td>
                              </tr>
                            ))}
                          </tbody>
                        </table>
                      )}
                      {ediAcks.length > 0 && (
                        <table className="screen-table">
                          <thead>
                            <tr>
                              <th>Ack</th>
                              <th>Status</th>
                              <th>Received</th>
                            </tr>
                          </thead>
                          <tbody>
                            {ediAcks.map((a) => (
                              <tr key={a.id}>
                                <td className="mono">{a.acknowledgement_number}</td>
                                <td>{a.status}</td>
                                <td>{new Date(a.received_at).toLocaleString()}</td>
                              </tr>
                            ))}
                          </tbody>
                        </table>
                      )}
                    </>
                  )}
                </div>
              )}

              {match !== null && (
                <div className="screen-sub">
                  <h4>3-way match — {match.status}</h4>
                  <p className="screen-meta">
                    PO total {match.po_total} · Invoiced {match.invoiced_total} · Variance{' '}
                    {match.total_variance_amount}
                  </p>
                  {match.lines.length > 0 && (
                    <table className="screen-table">
                      <thead>
                        <tr>
                          <th>SKU</th>
                          <th className="num">Ordered</th>
                          <th className="num">Received</th>
                          <th className="num">Invoiced</th>
                          <th className="num">Qty var.</th>
                          <th>Status</th>
                        </tr>
                      </thead>
                      <tbody>
                        {match.lines.map((m, i) => (
                          <tr key={i}>
                            <td className="mono">{m.vendor_sku}</td>
                            <td className="num">{fmtQty(m.po_qty)}</td>
                            <td className="num">{fmtQty(m.received_qty)}</td>
                            <td className="num">{fmtQty(m.invoiced_qty)}</td>
                            <td className="num">{fmtQty(m.qty_variance)}</td>
                            <td>{m.status}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  )}
                </div>
              )}
            </>
          )}
        </div>
      </div>
    </section>
  )
}
