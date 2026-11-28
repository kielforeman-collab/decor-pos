// Reports — operational & BI reports: daily usage, low stock,
// PaintCare / Eco remittance (with CSV download), ad-hoc transaction
// inquiry, and scheduled email outbox.

import { useCallback, useEffect, useState } from 'react'
import {
  getDailyUsageReport,
  getEcoRemittance,
  getEcoRemittanceCsv,
  getLowStockReport,
  listEmailOutbox,
  runInquiry,
} from '../backofficeApi'
import type {
  DailyUsageReport,
  EcoRemittanceReport,
  EmailDelivery,
  InquiryRow,
  LowStockReport,
} from '../backofficeTypes'
import { fmtMoney, fmtQty } from '../types'

type ReportTab = 'daily' | 'eco' | 'inquiry' | 'outbox'

function today(): string {
  return new Date().toISOString().slice(0, 10)
}

function monthStart(): string {
  return `${today().slice(0, 7)}-01`
}

export default function ReportsScreen({
  storeId,
  onToast,
}: {
  storeId: string | null
  onToast: (msg: string) => void
}) {
  const [tab, setTab] = useState<ReportTab>('daily')
  const [date, setDate] = useState(today())
  const [start, setStart] = useState(monthStart())
  const [end, setEnd] = useState(today())
  const [docType, setDocType] = useState('')
  const [customerId, setCustomerId] = useState('')
  const [productId, setProductId] = useState('')

  const [daily, setDaily] = useState<DailyUsageReport | null>(null)
  const [lowStock, setLowStock] = useState<LowStockReport | null>(null)
  const [eco, setEco] = useState<EcoRemittanceReport | null>(null)
  const [inquiryRows, setInquiryRows] = useState<InquiryRow[]>([])
  const [outbox, setOutbox] = useState<EmailDelivery[]>([])

  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const loadDaily = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const [d, l] = await Promise.all([getDailyUsageReport(date), getLowStockReport(date)])
      setDaily(d)
      setLowStock(l)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [date])

  const loadEco = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      setEco(await getEcoRemittance(start, end))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [start, end])

  const loadInquiry = useCallback(async () => {
    if (!storeId) return
    setLoading(true)
    setError(null)
    try {
      setInquiryRows(
        await runInquiry({
          storeId,
          start,
          end,
          docType: docType || undefined,
          customerId: customerId.trim() || undefined,
          productId: productId.trim() || undefined,
        }),
      )
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [storeId, start, end, docType, customerId, productId])

  const loadOutbox = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      setOutbox(await listEmailOutbox())
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    if (tab === 'daily') void loadDaily()
    else if (tab === 'eco') void loadEco()
    else if (tab === 'inquiry') void loadInquiry()
    else void loadOutbox()
  }, [tab, loadDaily, loadEco, loadInquiry, loadOutbox])

  async function downloadEcoCsv() {
    setError(null)
    try {
      const csv = await getEcoRemittanceCsv(start, end)
      const blob = new Blob([csv], { type: 'text/csv;charset=utf-8' })
      const url = URL.createObjectURL(blob)
      const a = document.createElement('a')
      a.href = url
      a.download = `eco-remittance-${start}-to-${end}.csv`
      a.click()
      URL.revokeObjectURL(url)
      onToast('Eco remittance CSV downloaded.')
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <section className="screen" aria-label="Reports">
      <div className="screen-head">
        <h2>Reports</h2>
        <div className="screen-tabs" role="tablist">
          {(
            [
              ['daily', 'Daily & low stock'],
              ['eco', 'PaintCare / Eco'],
              ['inquiry', 'Ad-hoc inquiry'],
              ['outbox', 'Email outbox'],
            ] as [ReportTab, string][]
          ).map(([id, label]) => (
            <button
              key={id}
              type="button"
              role="tab"
              aria-selected={tab === id}
              className={tab === id ? 'active' : ''}
              onClick={() => setTab(id)}
            >
              {label}
            </button>
          ))}
        </div>
      </div>

      {error && (
        <div className="error" role="alert">
          <span>{error}</span>
          <button type="button" className="banner-x" aria-label="Dismiss error" onClick={() => setError(null)}>✕</button>
        </div>
      )}

      {tab === 'daily' && (
        <>
          <div className="screen-form-row">
            <label>
              Report date
              <input type="date" className="screen-input" value={date} onChange={(e) => setDate(e.target.value)} />
            </label>
            <button type="button" className="screen-btn screen-btn-primary" disabled={loading} onClick={() => void loadDaily()}>
              {loading ? 'Loading…' : 'Load'}
            </button>
          </div>
          <div className="screen-split">
            <div className="screen-panel">
              <h3>Daily usage</h3>
              {daily ? (
                <pre className="screen-pre">{JSON.stringify(daily, null, 2)}</pre>
              ) : (
                <p className="screen-empty">No daily usage loaded.</p>
              )}
            </div>
            <div className="screen-panel">
              <h3>Low stock</h3>
              {lowStock ? (
                <pre className="screen-pre">{JSON.stringify(lowStock, null, 2)}</pre>
              ) : (
                <p className="screen-empty">No low-stock report loaded.</p>
              )}
            </div>
          </div>
        </>
      )}

      {tab === 'eco' && (
        <div className="screen-panel">
          <h3>PaintCare / Eco remittance</h3>
          <div className="screen-form-row">
            <label>
              Start
              <input type="date" className="screen-input" value={start} onChange={(e) => setStart(e.target.value)} />
            </label>
            <label>
              End
              <input type="date" className="screen-input" value={end} onChange={(e) => setEnd(e.target.value)} />
            </label>
            <button type="button" className="screen-btn screen-btn-primary" disabled={loading} onClick={() => void loadEco()}>
              Load
            </button>
            <button type="button" className="screen-btn" onClick={() => void downloadEcoCsv()}>
              Download CSV
            </button>
          </div>
          {!eco || eco.rows.length === 0 ? (
            <p className="screen-empty">No eco-fee transactions in this range.</p>
          ) : (
            <>
              <p className="screen-meta">Total eco fees: <strong>{fmtMoney(eco.total)}</strong></p>
              <table className="screen-table">
                <thead>
                  <tr>
                    <th>Jurisdiction</th>
                    <th>Store</th>
                    <th>Category</th>
                    <th className="num">Txns</th>
                    <th className="num">Units</th>
                    <th className="num">Total</th>
                  </tr>
                </thead>
                <tbody>
                  {eco.rows.map((r, idx) => (
                    <tr key={`${r.store_id}-${r.eco_fee_category ?? 'none'}-${idx}`}>
                      <td>{r.country_code}-{r.region_code}</td>
                      <td className="mono">{r.store_code}</td>
                      <td>{r.eco_fee_category ?? '—'}</td>
                      <td className="num">{r.transaction_count}</td>
                      <td className="num">{fmtQty(r.units)}</td>
                      <td className="num">{fmtMoney(r.eco_fee_total)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </>
          )}
        </div>
      )}

      {tab === 'inquiry' && (
        <div className="screen-panel">
          <h3>Ad-hoc transaction inquiry</h3>
          <div className="screen-form-row">
            <label>
              Start
              <input type="date" className="screen-input" value={start} onChange={(e) => setStart(e.target.value)} />
            </label>
            <label>
              End
              <input type="date" className="screen-input" value={end} onChange={(e) => setEnd(e.target.value)} />
            </label>
            <label>
              Doc type
              <select className="screen-input" value={docType} onChange={(e) => setDocType(e.target.value)}>
                <option value="">All</option>
                <option value="sale">Sale</option>
                <option value="return">Return</option>
                <option value="quote">Quote</option>
                <option value="layaway">Layaway</option>
              </select>
            </label>
            <label>
              Customer UUID (optional)
              <input className="screen-input mono" value={customerId} onChange={(e) => setCustomerId(e.target.value)} placeholder="UUID" />
            </label>
            <label>
              Product UUID (optional)
              <input className="screen-input mono" value={productId} onChange={(e) => setProductId(e.target.value)} placeholder="UUID" />
            </label>
            <button type="button" className="screen-btn screen-btn-primary" disabled={loading} onClick={() => void loadInquiry()}>
              Run inquiry
            </button>
          </div>
          {inquiryRows.length === 0 ? (
            <p className="screen-empty">No matching transactions.</p>
          ) : (
            <table className="screen-table">
              <thead>
                <tr>
                  <th>Doc #</th>
                  <th>Type</th>
                  <th>Status</th>
                  <th>Date</th>
                  <th className="num">Lines</th>
                  <th className="num">Grand total</th>
                </tr>
              </thead>
              <tbody>
                {inquiryRows.map((r) => (
                  <tr key={r.transaction_id}>
                    <td className="mono">{r.doc_number}</td>
                    <td>{r.doc_type}</td>
                    <td>{r.status}</td>
                    <td className="mono">{r.created_at.slice(0, 19).replace('T', ' ')}</td>
                    <td className="num">{r.line_count}</td>
                    <td className="num">{fmtMoney(r.grand_total)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      )}

      {tab === 'outbox' && (
        <div className="screen-panel">
          <h3>Scheduled email outbox ({outbox.length})</h3>
          {outbox.length === 0 ? (
            <p className="screen-empty">No scheduled email deliveries recorded.</p>
          ) : (
            <table className="screen-table">
              <thead>
                <tr>
                  <th>Report</th>
                  <th>Recipient</th>
                  <th>Subject</th>
                  <th>File</th>
                  <th>Status</th>
                  <th>Created</th>
                </tr>
              </thead>
              <tbody>
                {outbox.map((m) => (
                  <tr key={m.id}>
                    <td>{m.report_kind}</td>
                    <td>{m.recipient}</td>
                    <td>{m.subject}</td>
                    <td className="mono">{m.file_name}</td>
                    <td>{m.status}</td>
                    <td className="mono">{m.created_at.slice(0, 19).replace('T', ' ')}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      )}
    </section>
  )
}
