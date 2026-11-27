import { useEffect, useMemo, useState } from 'react'
import { getCustomerTransactions, getInvoice } from '../api'
import {
  fmtMoney,
  cad,
  type CustomerSearchResult,
  type CustomerTransaction,
  type InvoiceData,
} from '../types'

interface HistoryModalProps {
  customer: CustomerSearchResult
  onReprint: (invoice: InvoiceData) => void
  onClose: () => void
  onToast: (msg: string) => void
}

function displayName(c: CustomerSearchResult): string {
  const person = [c.first_name, c.last_name].filter(Boolean).join(' ')
  return c.company_name || person || c.account_number
}

/** Reprint fallback when the invoice endpoint is not implemented yet:
 * totals come from the transaction record; GST/PST split is unknown
 * so it is shown as $0.00 with the grand total authoritative. */
function fallbackInvoice(tx: CustomerTransaction, customer: CustomerSearchResult): InvoiceData {
  const zero = cad('0.0000')
  return {
    store_name: 'NORTH SHORE DECORATING CENTRE',
    doc_number: tx.doc_number,
    created_at: tx.created_at,
    customer: {
      name: displayName(customer),
      account_number: customer.account_number,
      email: customer.email,
    },
    job_name: tx.job_name ?? null,
    po_number: tx.po_number ?? null,
    fulfilment_mode: null,
    clerk: null,
    terms: customer.payment_terms ?? null,
    note: null,
    lines: tx.lines.map((l) => ({
      sku: l.sku,
      description: l.description,
      qty: l.qty,
      uom: l.uom ?? 'ea',
      unit_price: l.unit_price,
      disc_pct: null,
      amount: l.line_total ?? l.amount ?? cad('0.0000'),
      size_label: l.size_label ?? null,
      tint: l.tint ?? null,
    })),
    tenders: [],
    totals: {
      subtotal: tx.grand_total,
      discounts: zero,
      eco_fee_total: zero,
      delivery: zero,
      gst: zero,
      pst: zero,
      grand_total: tx.grand_total,
      change_due: zero,
      charge: null,
    },
  }
}

export default function HistoryModal({ customer, onReprint, onClose, onToast }: HistoryModalProps) {
  const [filter, setFilter] = useState('')
  const [sales, setSales] = useState<CustomerTransaction[]>([])
  const [loading, setLoading] = useState(true)
  const [reprinting, setReprinting] = useState<string | null>(null)

  useEffect(() => {
    let live = true
    void getCustomerTransactions(customer.id, 25)
      .then((txs) => {
        if (live) setSales(txs)
      })
      .catch((e: unknown) => {
        if (live) onToast(e instanceof Error ? e.message : String(e))
      })
      .finally(() => {
        if (live) setLoading(false)
      })
    return () => {
      live = false
    }
  }, [customer.id, onToast])

  const filtered = useMemo(() => {
    const q = filter.trim().toLowerCase()
    if (!q) return sales
    return sales.filter(
      (s) =>
        s.doc_number.toLowerCase().includes(q) ||
        (s.job_name ?? '').toLowerCase().includes(q) ||
        s.lines.some(
          (l) => l.sku.toLowerCase().includes(q) || l.description.toLowerCase().includes(q),
        ),
    )
  }, [sales, filter])

  async function doReprint(tx: CustomerTransaction) {
    setReprinting(tx.id)
    try {
      const inv = await getInvoice(tx.id)
      onReprint(inv ?? fallbackInvoice(tx, customer))
    } catch (e) {
      onToast(e instanceof Error ? e.message : String(e))
    } finally {
      setReprinting(null)
    }
  }

  return (
    <div className="modal-overlay" role="dialog" aria-modal="true" aria-label="Purchase history">
      <div className="modal history-modal">
        <div className="modal-head">
          <h2>Purchase history — {displayName(customer)}</h2>
          <button type="button" className="modal-x" onClick={onClose} aria-label="Close history">✕</button>
        </div>
        <input
          className="history-filter"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="Filter by invoice #, SKU, product or job…"
          aria-label="Filter purchase history"
          autoFocus
        />
        {loading && <p className="hint">Loading history…</p>}
        {!loading && filtered.length === 0 && (
          <p className="hint">No sales found for this customer.</p>
        )}
        <div className="history-list">
          {filtered.map((s) => (
            <article key={s.id} className="history-sale">
              <div className="history-head">
                <span>{new Date(s.created_at).toLocaleString('en-CA')}</span>
                <span>{s.doc_number}</span>
                {s.job_name && <span>Job: {s.job_name}</span>}
                <strong>{fmtMoney(s.grand_total)}</strong>
                <button
                  type="button"
                  disabled={reprinting === s.id}
                  onClick={() => void doReprint(s)}
                >
                  Reprint invoice
                </button>
              </div>
              <div className="history-lines">
                {s.lines.map((l, i) => (
                  <div key={`${s.id}-${i}`} className="history-line">
                    <span className="hl-sku">{l.sku}</span>
                    <span className="hl-desc">
                      {l.description}
                      {l.size_label && <small> · {l.size_label}</small>}
                      {l.tint && <small> · {l.tint}</small>}
                    </span>
                    <span className="hl-qty">{l.qty} × {fmtMoney(l.unit_price)}</span>
                    <span className="hl-amt">{fmtMoney(l.line_total ?? l.amount ?? cad('0.0000'))}</span>
                  </div>
                ))}
              </div>
            </article>
          ))}
        </div>
        <div className="modal-foot">
          <button type="button" onClick={onClose}>Close</button>
        </div>
      </div>
    </div>
  )
}
