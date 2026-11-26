import { useCallback, useEffect, useState } from 'react'
import { createActivity, getKpis, listActivities, recordQuote } from './api'
import { DEMO_CATALOG } from './catalog'
import type { CrmActivity, MobileKpis, SaleLineInput } from './types'
import { fmtMoney } from './types'

// Demo portal contractor from apps/phase5-seed-demo.sql.
export const DEMO_CUSTOMER_ID = '00000000-0000-7000-8000-000000000101'

function useNotice() {
  const [msg, setMsg] = useState<{ kind: 'ok' | 'error'; text: string } | null>(null)
  const show = useCallback((kind: 'ok' | 'error', text: string) => {
    setMsg({ kind, text })
  }, [])
  const node = msg ? <div className={msg.kind}>{msg.text}</div> : null
  return { show, node }
}

export default function Sales({ storeId }: { storeId: string }) {
  return (
    <div>
      <Kpis storeId={storeId} />
      <Activities />
      <QuoteBuilder />
    </div>
  )
}

function Kpis({ storeId }: { storeId: string }) {
  const [kpis, setKpis] = useState<MobileKpis | null>(null)
  const [error, setError] = useState<string | null>(null)
  const { show, node } = useNotice()

  const refresh = useCallback(async () => {
    try {
      setKpis(await getKpis(storeId))
      setError(null)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [storeId])

  useEffect(() => {
    void refresh()
  }, [refresh])

  return (
    <section className="card">
      <h2>Today — {kpis?.date ?? '…'}</h2>
      {node}
      {error && <div className="error">{error}</div>}
      {kpis && (
        <div className="kpi-grid">
          <div className="kpi">
            <div className="label">Sales today</div>
            <div className="value">{fmtMoney(kpis.sales_today)}</div>
            <div className="note">
              {kpis.sales_today_count} sale{kpis.sales_today_count === 1 ? '' : 's'}
            </div>
          </div>
          <div className="kpi">
            <div className="label">Open AR</div>
            <div className="value">{fmtMoney(kpis.ar_open_total)}</div>
            <div className="note">charge-account balance</div>
          </div>
          <div className="kpi">
            <div className="label">Open quotes</div>
            <div className="value">{kpis.open_quotes}</div>
            <div className="note">waiting on customers</div>
          </div>
          <div className="kpi">
            <div className="label">Open orders</div>
            <div className="value">{kpis.open_orders}</div>
            <div className="note">portal + BOPIS, not completed</div>
          </div>
          <div className="kpi">
            <div className="label">Deliveries out</div>
            <div className="value">{kpis.deliveries_out_today}</div>
            <div className="note">out for delivery today</div>
          </div>
        </div>
      )}
      <div className="row" style={{ marginTop: 10 }}>
        <button className="btn secondary" onClick={() => void refresh()}>
          Refresh
        </button>
      </div>
    </section>
  )
}

const ACTIVITY_TYPES = ['call', 'visit', 'email', 'note', 'follow_up']

function Activities() {
  const [activities, setActivities] = useState<CrmActivity[]>([])
  const [customerId, setCustomerId] = useState(DEMO_CUSTOMER_ID)
  const [type, setType] = useState('call')
  const [subject, setSubject] = useState('')
  const [notes, setNotes] = useState('')
  const [busy, setBusy] = useState(false)
  const { show, node } = useNotice()

  const refresh = useCallback(async () => {
    try {
      setActivities(
        await listActivities(customerId.trim() ? { customer_id: customerId.trim() } : undefined),
      )
    } catch (err) {
      show('error', err instanceof Error ? err.message : String(err))
    }
  }, [customerId, show])

  useEffect(() => {
    void refresh()
  }, [refresh])

  async function add() {
    if (!subject.trim()) {
      show('error', 'Enter a subject for the activity.')
      return
    }
    setBusy(true)
    try {
      const res = await createActivity({
        customer_id: customerId.trim() || null,
        activity_type: type,
        subject: subject.trim(),
        notes: notes.trim() || null,
      })
      if (res.queued) {
        show('ok', 'Offline — activity queued in the outbox. Sync when back online.')
      } else {
        show('ok', 'Activity logged.')
        setSubject('')
        setNotes('')
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
      <h2>CRM Activities</h2>
      {node}
      <label className="field">
        Customer id
        <input
          type="text"
          value={customerId}
          onChange={(e) => setCustomerId(e.target.value)}
          placeholder="customer UUID (blank = all)"
        />
      </label>
      {activities.length === 0 ? (
        <p className="muted">No activities for this customer yet.</p>
      ) : (
        activities.slice(0, 20).map((a) => (
          <div className="list-item" key={a.id}>
            <div className="row">
              <span className="title">{a.subject}</span>
              <span className={`status ${a.completed_at ? 'delivered' : 'open'}`}>
                {a.completed_at ? 'done' : a.activity_type}
              </span>
            </div>
            {a.notes && <div className="muted">{a.notes}</div>}
            <div className="muted">{new Date(a.created_at).toLocaleString()}</div>
          </div>
        ))
      )}
      <h3>Log an activity</h3>
      <label className="field">
        Type
        <select value={type} onChange={(e) => setType(e.target.value)}>
          {ACTIVITY_TYPES.map((t) => (
            <option key={t} value={t}>
              {t}
            </option>
          ))}
        </select>
      </label>
      <label className="field">
        Subject
        <input type="text" value={subject} onChange={(e) => setSubject(e.target.value)} placeholder="e.g. Called about White Dove quote" />
      </label>
      <label className="field">
        Notes
        <textarea value={notes} onChange={(e) => setNotes(e.target.value)} />
      </label>
      <button className="btn" onClick={() => void add()} disabled={busy}>
        Log activity
      </button>
    </section>
  )
}

interface QuoteLine {
  product_id: string
  qty: string
}

function QuoteBuilder() {
  const [customerId, setCustomerId] = useState(DEMO_CUSTOMER_ID)
  const [poNumber, setPoNumber] = useState('')
  const [lines, setLines] = useState<QuoteLine[]>([{ product_id: DEMO_CATALOG[0].product_id, qty: '1' }])
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState<string | null>(null)
  const { show, node } = useNotice()

  function addLine() {
    setLines((ls) => [...ls, { product_id: DEMO_CATALOG[0].product_id, qty: '1' }])
  }

  function updateLine(i: number, patch: Partial<QuoteLine>) {
    setLines((ls) => ls.map((l, j) => (j === i ? { ...l, ...patch } : l)))
  }

  function removeLine(i: number) {
    setLines((ls) => ls.filter((_, j) => j !== i))
  }

  async function submit() {
    const saleLines: SaleLineInput[] = []
    for (const l of lines) {
      const product = DEMO_CATALOG.find((p) => p.product_id === l.product_id)!
      const qty = l.qty.trim()
      if (!qty || Number(qty) <= 0) continue
      saleLines.push({
        product_id: product.product_id,
        sku: product.sku,
        description: product.description,
        qty,
        uom: product.uom,
        unit_price: { amount: product.unit_price, currency: 'Usd' },
        unit_cost: { amount: product.unit_cost, currency: 'Usd' },
      })
    }
    if (!saleLines.length) {
      show('error', 'Add at least one line with a positive quantity.')
      return
    }
    setBusy(true)
    setResult(null)
    try {
      const res = await recordQuote({
        customer_id: customerId.trim() || null,
        job_id: null,
        lines: saleLines,
        tenders: [],
        po_number: poNumber.trim() || null,
      })
      if (res.queued) {
        show('ok', 'Offline — quote queued in the outbox. Sync when back online.')
      } else {
        setResult(`Quote recorded — transaction ${res.data.transaction_id}.`)
      }
    } catch (err) {
      show('error', err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <section className="card">
      <h2>New Quote</h2>
      {node}
      {result && <div className="ok">{result}</div>}
      <label className="field">
        Customer id
        <input type="text" value={customerId} onChange={(e) => setCustomerId(e.target.value)} />
      </label>
      <label className="field">
        PO number (optional)
        <input type="text" value={poNumber} onChange={(e) => setPoNumber(e.target.value)} />
      </label>
      {lines.map((l, i) => (
        <div className="row" key={i} style={{ marginBottom: 8 }}>
          <select
            value={l.product_id}
            onChange={(e) => updateLine(i, { product_id: e.target.value })}
            aria-label={`Quote line ${i + 1} product`}
            style={{ flex: 1, minHeight: 44 }}
          >
            {DEMO_CATALOG.map((p) => (
              <option key={p.product_id} value={p.product_id}>
                {p.sku} — {p.description} (${Number(p.unit_price).toFixed(2)})
              </option>
            ))}
          </select>
          <input
            type="number"
            min="0"
            step="any"
            inputMode="decimal"
            placeholder="Qty"
            style={{ width: 80, minHeight: 44 }}
            value={l.qty}
            onChange={(e) => updateLine(i, { qty: e.target.value })}
            aria-label={`Quote line ${i + 1} quantity`}
          />
          {lines.length > 1 && (
            <button className="btn secondary small" onClick={() => removeLine(i)} aria-label={`Remove line ${i + 1}`}>
              ✕
            </button>
          )}
        </div>
      ))}
      <div className="row">
        <button className="btn secondary" onClick={addLine}>
          + Add line
        </button>
        <button className="btn" onClick={() => void submit()} disabled={busy}>
          Record quote
        </button>
      </div>
      <p className="muted">
        Quotes are open transactions (no tender, no outbox entry) — they show up in the
        open-quotes KPI once recorded.
      </p>
    </section>
  )
}
