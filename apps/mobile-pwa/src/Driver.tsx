import { useCallback, useEffect, useRef, useState } from 'react'
import { getDelivery, listDispatch, setDeliveryStatus, sha256Hex, submitPod } from './api'
import SignaturePad from './SignaturePad'
import type { DeliveryView } from './types'
import { fmtAddress } from './types'

function useNotice() {
  const [msg, setMsg] = useState<{ kind: 'ok' | 'error'; text: string } | null>(null)
  const show = useCallback((kind: 'ok' | 'error', text: string) => {
    setMsg({ kind, text })
  }, [])
  const node = msg ? <div className={msg.kind}>{msg.text}</div> : null
  return { show, node }
}

// The board only ever lists OPEN deliveries: the server drops
// delivered/cancelled rows before filtering (df-db
// list_dispatch_board retains draft/assigned/out_for_delivery),
// so those statuses are not offered as filters.
const STATUS_FILTERS = ['', 'draft', 'assigned', 'out_for_delivery']

export default function Driver() {
  const [deliveries, setDeliveries] = useState<DeliveryView[]>([])
  const [statusFilter, setStatusFilter] = useState('out_for_delivery')
  const [selectedId, setSelectedId] = useState('')
  const [selected, setSelected] = useState<DeliveryView | null>(null)
  const { show, node } = useNotice()

  const refresh = useCallback(async () => {
    try {
      const list = await listDispatch(statusFilter ? { status: statusFilter } : undefined)
      setDeliveries(list)
      if (list.length && !selectedId) setSelectedId(list[0].id)
    } catch (err) {
      show('error', err instanceof Error ? err.message : String(err))
    }
  }, [statusFilter, selectedId, show])

  useEffect(() => {
    void refresh()
  }, [refresh])

  useEffect(() => {
    if (!selectedId) {
      setSelected(null)
      return
    }
    let cancelled = false
    getDelivery(selectedId)
      .then((d) => {
        if (!cancelled) setSelected(d)
      })
      .catch((err) => {
        if (!cancelled) show('error', err instanceof Error ? err.message : String(err))
      })
    return () => {
      cancelled = true
    }
  }, [selectedId, show])

  async function markOut(id: string) {
    try {
      const res = await setDeliveryStatus(id, { status: 'out_for_delivery' })
      if (res.queued) {
        show('ok', 'Offline — status change queued in the outbox.')
      } else {
        show('ok', `Delivery ${id.slice(0, 8)} is now out for delivery.`)
      }
      await refresh()
    } catch (err) {
      show('error', err instanceof Error ? err.message : String(err))
    }
  }

  return (
    <div>
      <section className="card">
        <h2>Dispatch Board</h2>
        {node}
        <label className="field">
          Status
          <select value={statusFilter} onChange={(e) => setStatusFilter(e.target.value)}>
            {STATUS_FILTERS.map((s) => (
              <option key={s} value={s}>
                {s === '' ? 'all' : s}
              </option>
            ))}
          </select>
        </label>
        {deliveries.length === 0 ? (
          <p className="muted">
            No open deliveries with this status. The board only lists open
            deliveries (draft / assigned / out for delivery) — delivered
            ones leave the board. Create one from a sale in the counter POS
            or the back office (Delivery → create), then it appears here.
          </p>
        ) : (
          deliveries.map((d) => (
            <div
              className="list-item"
              key={d.id}
              onClick={() => setSelectedId(d.id)}
              style={{
                cursor: 'pointer',
                background: d.id === selectedId ? '#eef2ff' : undefined,
                borderRadius: 8,
                paddingLeft: 8,
                paddingRight: 8,
              }}
            >
              <div className="row">
                <span className="title mono">{d.id.slice(0, 8)}</span>
                <span className={`status ${d.status}`}>{d.status}</span>
                {d.priority > 0 && <span className="muted">priority {d.priority}</span>}
              </div>
              <div className="muted">{fmtAddress(d.address_json)}</div>
              {d.scheduled_date && <div className="muted">Scheduled {d.scheduled_date}</div>}
              {d.vehicle && <div className="muted">Vehicle: {d.vehicle}</div>}
            </div>
          ))
        )}
        <div className="row" style={{ marginTop: 10 }}>
          <button className="btn secondary" onClick={() => void refresh()}>
            Refresh
          </button>
        </div>
      </section>

      {selected && (
        <PodForm
          delivery={selected}
          onDone={() => void refresh()}
          onMarkOut={() => void markOut(selected.id)}
          show={show}
        />
      )}
    </div>
  )
}

function PodForm({
  delivery,
  onDone,
  onMarkOut,
  show,
}: {
  delivery: DeliveryView
  onDone: () => void
  onMarkOut: () => void
  show: (kind: 'ok' | 'error', text: string) => void
}) {
  const [receivedBy, setReceivedBy] = useState('')
  const [signature, setSignature] = useState<string | null>(null)
  const [photo, setPhoto] = useState<{ name: string; sha256: string } | null>(null)
  const [busy, setBusy] = useState(false)
  const [done, setDone] = useState(false)
  const fileRef = useRef<HTMLInputElement>(null)

  const canSubmit = delivery.status === 'out_for_delivery' && !delivery.pod

  async function onPhoto(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0]
    if (!file) return
    const bytes = await file.arrayBuffer()
    const hash = await sha256Hex(bytes)
    setPhoto({ name: file.name, sha256: hash })
  }

  async function submit() {
    if (!receivedBy.trim()) {
      show('error', 'Enter who received the delivery.')
      return
    }
    if (!signature) {
      show('error', 'Capture a signature first.')
      return
    }
    if (!photo) {
      show('error', 'Attach a delivery photo first — the server refuses a POD without one.')
      return
    }
    setBusy(true)
    try {
      const b64 = signature.split(',')[1] ?? ''
      const res = await submitPod(delivery.id, {
        signature_b64: b64,
        photo_ref: `pod-photo/${delivery.id}/${photo.name}`,
        photo_sha256: photo.sha256,
        received_by: receivedBy.trim(),
      })
      if (res.queued) {
        show('ok', 'Offline — POD queued in the outbox. Sync when back online.')
      } else {
        show('ok', `POD submitted — delivery ${delivery.id.slice(0, 8)} is now delivered.`)
        setDone(true)
      }
      onDone()
    } catch (err) {
      show('error', err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <section className="card">
      <h2>Proof of Delivery — {delivery.id.slice(0, 8)}</h2>
      <p>
        <span className={`status ${delivery.status}`}>{delivery.status}</span>{' '}
        <span className="muted">{fmtAddress(delivery.address_json)}</span>
      </p>
      {delivery.pod ? (
        <div className="ok">
          POD already submitted — received by {delivery.pod.received_by} at{' '}
          {new Date(delivery.pod.signed_at).toLocaleString()}. Signature SHA-256:{' '}
          <span className="mono">{delivery.pod.signature_sha256.slice(0, 16)}…</span>
          {delivery.pod.photo_ref && (
            <>
              <br />
              Photo ref: <span className="mono">{delivery.pod.photo_ref}</span>
            </>
          )}
        </div>
      ) : !canSubmit ? (
        <div>
          <p className="muted">
            A POD can only be submitted while the delivery is out for delivery (server rule).
            This one is <strong>{delivery.status}</strong>.
          </p>
          {delivery.status === 'assigned' && (
            <button className="btn" onClick={onMarkOut}>
              Mark out for delivery
            </button>
          )}
        </div>
      ) : (
        <>
          <label className="field">
            Received by
            <input
              type="text"
              value={receivedBy}
              onChange={(e) => setReceivedBy(e.target.value)}
              placeholder="Name of the person who signed"
              autoComplete="off"
            />
          </label>
          <h3>Signature</h3>
          <SignaturePad onChange={setSignature} />
          <h3>Delivery photo</h3>
          <input
            ref={fileRef}
            type="file"
            accept="image/*"
            capture="environment"
            onChange={(e) => void onPhoto(e)}
            aria-label="Delivery photo"
          />
          {photo && (
            <p className="muted">
              {photo.name} — SHA-256 <span className="mono">{photo.sha256.slice(0, 16)}…</span>
            </p>
          )}
          <p className="muted">
            The photo file itself is not uploaded — the edge has no photo-upload endpoint.
            Only the file reference and its SHA-256 hash are recorded with the POD, so the
            photo on the device can be matched to the delivery later.
          </p>
          <div className="row">
            <button
              className="btn"
              onClick={() => void submit()}
              disabled={busy || done || !signature || !photo || !receivedBy.trim()}
            >
              Submit POD
            </button>
          </div>
        </>
      )}
    </section>
  )
}
