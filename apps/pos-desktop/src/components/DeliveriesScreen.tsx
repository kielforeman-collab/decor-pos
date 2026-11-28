// Deliveries — dispatch board, delivery creation, driver assignment,
// status transitions, packing slips (42-col text + ZPL), and Proof of
// Delivery close-out (signature + photo ref + SHA-256).

import { useCallback, useEffect, useState } from 'react'
import {
  assignDelivery,
  createDelivery,
  getDelivery,
  getPackingSlip,
  listDispatch,
  setDeliveryStatus,
  submitPod,
} from '../backofficeApi'
import type { DeliveryView, PackingSlipView } from '../backofficeTypes'

const STATUS_FILTERS = ['', 'draft', 'assigned', 'out_for_delivery', 'delivered', 'cancelled']

function today(): string {
  return new Date().toISOString().slice(0, 10)
}

function formatAddress(addr: unknown): string {
  if (!addr || typeof addr !== 'object') return '—'
  const a = addr as Record<string, unknown>
  return [a.street, a.city, a.region, a.postal].filter(Boolean).join(', ') || JSON.stringify(addr)
}

async function sha256Hex(text: string): Promise<string> {
  const bytes = new TextEncoder().encode(text)
  const digest = await crypto.subtle.digest('SHA-256', bytes)
  return Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, '0'))
    .join('')
}

export default function DeliveriesScreen({
  onToast,
}: {
  onToast: (msg: string) => void
}) {
  const [deliveries, setDeliveries] = useState<DeliveryView[]>([])
  const [selected, setSelected] = useState<DeliveryView | null>(null)
  const [packingSlip, setPackingSlip] = useState<PackingSlipView | null>(null)
  const [statusFilter, setStatusFilter] = useState('')
  const [dateFilter, setDateFilter] = useState('')
  const [loading, setLoading] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Create form
  const [creating, setCreating] = useState(false)
  const [txId, setTxId] = useState('')
  const [custId, setCustId] = useState('')
  const [street, setStreet] = useState('')
  const [city, setCity] = useState('')
  const [scheduledDate, setScheduledDate] = useState(today())
  const [vehicle, setVehicle] = useState('')

  // Assign / POD fields
  const [driverId, setDriverId] = useState('')
  const [assignVehicle, setAssignVehicle] = useState('')
  const [receivedBy, setReceivedBy] = useState('')
  const [signatureText, setSignatureText] = useState('')
  const [photoRef, setPhotoRef] = useState('')

  const load = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const list = await listDispatch({
        status: statusFilter || undefined,
        scheduled_date: dateFilter || undefined,
      })
      setDeliveries(list)
      setSelected((prev) => (prev ? list.find((d) => d.id === prev.id) ?? prev : null))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [statusFilter, dateFilter])

  useEffect(() => {
    void load()
  }, [load])

  async function selectDelivery(d: DeliveryView) {
    setPackingSlip(null)
    setAssignVehicle(d.vehicle ?? '')
    setDriverId(d.driver_id ?? '')
    try {
      const full = await getDelivery(d.id)
      setSelected(full)
    } catch {
      setSelected(d)
    }
  }

  async function doCreate() {
    if (!street.trim()) {
      setError('Street address is required.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const created = await createDelivery({
        transaction_id: txId.trim() || null,
        customer_id: custId.trim() || null,
        address_json: { street: street.trim(), city: city.trim() || undefined },
        scheduled_date: scheduledDate || null,
        vehicle: vehicle.trim() || null,
      })
      setCreating(false)
      setTxId('')
      setCustId('')
      setStreet('')
      setCity('')
      setVehicle('')
      onToast(`Delivery ${created.id.slice(0, 8)} created.`)
      await load()
      setSelected(created)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  async function doAssign() {
    if (!selected || !driverId.trim()) {
      setError('Driver UUID is required to assign a delivery.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const updated = await assignDelivery(selected.id, {
        driver_id: driverId.trim(),
        vehicle: assignVehicle.trim() || null,
      })
      setSelected(updated)
      onToast('Driver assigned.')
      await load()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  async function doStatus(status: string) {
    if (!selected) return
    setBusy(true)
    setError(null)
    try {
      const updated = await setDeliveryStatus(selected.id, { status })
      setSelected(updated)
      onToast(`Delivery status → ${updated.status}.`)
      await load()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  async function doPackingSlip() {
    if (!selected) return
    setBusy(true)
    setError(null)
    try {
      setPackingSlip(await getPackingSlip(selected.id))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  async function doSubmitPod() {
    if (!selected) return
    if (!receivedBy.trim() || !signatureText.trim() || !photoRef.trim()) {
      setError('POD close-out requires received-by name, signature, and photo reference.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const sigB64 = btoa(unescape(encodeURIComponent(signatureText.trim())))
      const photoSha = await sha256Hex(photoRef.trim())
      const updated = await submitPod(selected.id, {
        signature_b64: sigB64,
        photo_ref: photoRef.trim(),
        photo_sha256: photoSha,
        received_by: receivedBy.trim(),
      })
      setSelected(updated)
      setReceivedBy('')
      setSignatureText('')
      setPhotoRef('')
      onToast('POD recorded — delivery closed.')
      await load()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <section className="screen" aria-label="Deliveries">
      <div className="screen-head">
        <h2>Deliveries</h2>
        <div className="screen-actions">
          <select
            className="screen-input"
            style={{ width: 'auto' }}
            aria-label="Filter by status"
            value={statusFilter}
            onChange={(e) => setStatusFilter(e.target.value)}
          >
            {STATUS_FILTERS.map((s) => (
              <option key={s || 'all'} value={s}>
                {s ? s.replaceAll('_', ' ') : 'All statuses'}
              </option>
            ))}
          </select>
          <input
            type="date"
            className="screen-input"
            style={{ width: 'auto' }}
            aria-label="Filter by scheduled date"
            value={dateFilter}
            onChange={(e) => setDateFilter(e.target.value)}
          />
          <button type="button" className="screen-btn" disabled={loading} onClick={() => void load()}>
            {loading ? 'Loading…' : 'Refresh'}
          </button>
          <button
            type="button"
            className="screen-btn screen-btn-primary"
            onClick={() => setCreating((v) => !v)}
          >
            {creating ? 'Cancel' : '+ New delivery'}
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
          <h3>Create delivery</h3>
          <div className="screen-form-row">
            <label>
              Street address *
              <input className="screen-input" value={street} onChange={(e) => setStreet(e.target.value)} placeholder="123 Main St" />
            </label>
            <label>
              City
              <input className="screen-input" value={city} onChange={(e) => setCity(e.target.value)} placeholder="Vancouver" />
            </label>
            <label>
              Scheduled date
              <input type="date" className="screen-input" value={scheduledDate} onChange={(e) => setScheduledDate(e.target.value)} />
            </label>
            <label>
              Vehicle
              <input className="screen-input" value={vehicle} onChange={(e) => setVehicle(e.target.value)} placeholder="Van 1" />
            </label>
          </div>
          <div className="screen-form-row">
            <label>
              Transaction ID (optional)
              <input className="screen-input mono" value={txId} onChange={(e) => setTxId(e.target.value)} placeholder="UUID" />
            </label>
            <label>
              Customer ID (optional)
              <input className="screen-input mono" value={custId} onChange={(e) => setCustId(e.target.value)} placeholder="UUID" />
            </label>
            <button type="button" className="screen-btn screen-btn-primary" disabled={busy} onClick={() => void doCreate()}>
              Create delivery
            </button>
          </div>
        </div>
      )}

      <div className="screen-split">
        <div className="screen-panel">
          <h3>Dispatch board ({deliveries.length})</h3>
          {deliveries.length === 0 ? (
            <p className="screen-empty">No deliveries match the current filters.</p>
          ) : (
            <table className="screen-table screen-table-clickable">
              <thead>
                <tr>
                  <th>Date</th>
                  <th>Status</th>
                  <th>Vehicle</th>
                  <th>Address</th>
                </tr>
              </thead>
              <tbody>
                {deliveries.map((d) => (
                  <tr
                    key={d.id}
                    className={selected?.id === d.id ? 'selected' : ''}
                    onClick={() => void selectDelivery(d)}
                  >
                    <td className="mono">{d.scheduled_date ?? '—'}</td>
                    <td>{d.status.replaceAll('_', ' ')}</td>
                    <td>{d.vehicle ?? '—'}</td>
                    <td>{formatAddress(d.address_json)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>

        <div className="screen-panel">
          <h3>Delivery detail</h3>
          {!selected ? (
            <p className="screen-empty">Select a delivery from the dispatch board.</p>
          ) : (
            <>
              <dl className="screen-facts">
                <div><dt>ID</dt><dd className="mono">{selected.id.slice(0, 8)}…</dd></div>
                <div><dt>Status</dt><dd>{selected.status.replaceAll('_', ' ')}</dd></div>
                <div><dt>Scheduled</dt><dd className="mono">{selected.scheduled_date ?? '—'}</dd></div>
                <div><dt>Vehicle</dt><dd>{selected.vehicle ?? '—'}</dd></div>
                <div><dt>Address</dt><dd>{formatAddress(selected.address_json)}</dd></div>
                <div><dt>POD</dt><dd>{selected.pod ? `Signed by ${selected.pod.received_by}` : 'Pending'}</dd></div>
              </dl>

              <div className="screen-actions">
                {selected.status === 'assigned' && (
                  <button type="button" className="screen-btn screen-btn-primary" disabled={busy} onClick={() => void doStatus('out_for_delivery')}>
                    Mark out for delivery
                  </button>
                )}
                {selected.status !== 'delivered' && selected.status !== 'cancelled' && (
                  <button type="button" className="screen-btn" disabled={busy} onClick={() => void doStatus('cancelled')}>
                    Cancel delivery
                  </button>
                )}
                <button type="button" className="screen-btn" disabled={busy} onClick={() => void doPackingSlip()}>
                  Packing slip
                </button>
              </div>

              {selected.status !== 'delivered' && selected.status !== 'cancelled' && (
                <div className="screen-sub">
                  <h4>Assign driver</h4>
                  <div className="screen-form-row">
                    <label>
                      Driver ID (UUID)
                      <input className="screen-input mono" value={driverId} onChange={(e) => setDriverId(e.target.value)} placeholder="Driver UUID" />
                    </label>
                    <label>
                      Vehicle
                      <input className="screen-input" value={assignVehicle} onChange={(e) => setAssignVehicle(e.target.value)} placeholder="Van 2" />
                    </label>
                    <button type="button" className="screen-btn" disabled={busy} onClick={() => void doAssign()}>
                      Assign
                    </button>
                  </div>
                </div>
              )}

              {selected.status === 'out_for_delivery' && !selected.pod && (
                <div className="screen-sub">
                  <h4>Proof of Delivery close-out</h4>
                  <p className="screen-note">Deliveries can only close with both a recorded signature and photo reference.</p>
                  <div className="screen-form-row">
                    <label>
                      Received by *
                      <input className="screen-input" value={receivedBy} onChange={(e) => setReceivedBy(e.target.value)} placeholder="Foreman name" />
                    </label>
                    <label>
                      Signature payload *
                      <input className="screen-input" value={signatureText} onChange={(e) => setSignatureText(e.target.value)} placeholder="Signed name / stroke data" />
                    </label>
                    <label>
                      Photo reference *
                      <input className="screen-input" value={photoRef} onChange={(e) => setPhotoRef(e.target.value)} placeholder="photo://drop-001.jpg" />
                    </label>
                    <button type="button" className="screen-btn screen-btn-primary" disabled={busy} onClick={() => void doSubmitPod()}>
                      Submit POD &amp; deliver
                    </button>
                  </div>
                </div>
              )}

              {packingSlip && (
                <div className="screen-sub">
                  <h4>Packing slip ({packingSlip.lines.length} lines)</h4>
                  <pre className="screen-pre">{packingSlip.text}</pre>
                </div>
              )}

              {selected.events.length > 0 && (
                <div className="screen-sub">
                  <h4>Audit trail</h4>
                  <table className="screen-table">
                    <thead>
                      <tr>
                        <th>When</th>
                        <th>Transition</th>
                        <th>Note</th>
                      </tr>
                    </thead>
                    <tbody>
                      {selected.events.map((ev) => (
                        <tr key={ev.id}>
                          <td className="mono">{ev.created_at.slice(0, 19).replace('T', ' ')}</td>
                          <td>{ev.from_status} → {ev.to_status}</td>
                          <td>{ev.note ?? '—'}</td>
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
