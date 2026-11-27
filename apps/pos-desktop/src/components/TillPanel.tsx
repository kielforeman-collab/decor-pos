import { useEffect, useRef, useState } from 'react'
import { searchCustomers } from '../api'
import {
  fmtMoney,
  cad,
  type ArCustomerSummary,
  type CustomerSearchResult,
  type PriceLevel,
  type PricePreview,
  type ZReport,
} from '../types'

export type FulfilmentMode = 'take_now' | 'will_call' | 'delivery'

interface TillPanelProps {
  customer: CustomerSearchResult | null
  summary: ArCustomerSummary | null
  onSelectCustomer: (c: CustomerSearchResult | null) => void
  priceLevels: PriceLevel[]
  priceLevelId: string
  onPriceLevel: (id: string) => void
  emailInvoices: boolean
  onEmailInvoices: (v: boolean) => void
  onHistory: () => void
  preview: PricePreview | null
  discountsTotal: string
  paintItemCount: number
  fulfilment: FulfilmentMode
  deliveryFee: string
  onDeliveryFee: (v: string) => void
  grandDue: string
  tillOpen: boolean
  openingFloat: string
  onOpeningFloat: (v: string) => void
  onOpenTill: () => void
  countedCash: string
  onCountedCash: (v: string) => void
  onCloseTill: () => void
  onComplete: () => void
  completeDisabled: boolean
  onHold: () => void
  onQuote: () => void
  onVoid: () => void
  onOpenDrawer: () => void
  pendingSync: number | string
  onSync: () => void
  provider: string
  providerSwitchable: boolean
  onProvider: (name: string) => void
  zReport: ZReport | null
  saleEmpty: boolean
}

function displayName(c: CustomerSearchResult): string {
  const person = [c.first_name, c.last_name].filter(Boolean).join(' ')
  return c.company_name || person || c.account_number
}

export default function TillPanel(props: TillPanelProps) {
  const {
    customer, summary, onSelectCustomer, priceLevels, priceLevelId, onPriceLevel,
    emailInvoices, onEmailInvoices, onHistory, preview, discountsTotal, paintItemCount,
    fulfilment, deliveryFee, onDeliveryFee, grandDue, tillOpen, openingFloat, onOpeningFloat,
    onOpenTill, countedCash, onCountedCash, onCloseTill, onComplete, completeDisabled,
    onHold, onQuote, onVoid, onOpenDrawer, pendingSync, onSync, provider, providerSwitchable,
    onProvider, zReport, saleEmpty,
  } = props

  const [custQuery, setCustQuery] = useState('')
  const [custResults, setCustResults] = useState<CustomerSearchResult[]>([])
  const [custOpen, setCustOpen] = useState(false)
  const custBoxRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const q = custQuery.trim()
    if (q.length < 2) {
      setCustResults([])
      setCustOpen(false)
      return
    }
    const h = window.setTimeout(() => {
      void searchCustomers(q).then((r) => {
        setCustResults(r)
        setCustOpen(true)
      })
    }, 250)
    return () => window.clearTimeout(h)
  }, [custQuery])

  useEffect(() => {
    const onDoc = (e: MouseEvent) => {
      if (custBoxRef.current && !custBoxRef.current.contains(e.target as Node)) setCustOpen(false)
    }
    document.addEventListener('mousedown', onDoc)
    return () => document.removeEventListener('mousedown', onDoc)
  }, [])

  const level = priceLevels.find((l) => l.id === priceLevelId)
  const limit = summary?.credit_limit ?? customer?.credit_limit ?? null
  const balance = summary?.balance ?? null
  const limitNum = limit ? Number(limit.amount) : 0
  const balanceNum = balance ? Number(balance.amount) : 0
  const availNum = limitNum - balanceNum
  const limitPct = limitNum > 0 ? Math.min(100, Math.max(0, (balanceNum / limitNum) * 100)) : 0

  return (
    <div className="till-panel">
      <section className="till-section customer-card">
        <h2>Customer</h2>
        <div className="cust-search" ref={custBoxRef}>
          <input
            id="customer-search"
            value={customer ? displayName(customer) : custQuery}
            onChange={(e) => {
              if (customer) onSelectCustomer(null)
              setCustQuery(e.target.value)
            }}
            onFocus={() => {
              if (custResults.length > 0) setCustOpen(true)
            }}
            placeholder="Walk-in — search customers…"
            aria-label="Customer search"
          />
          {customer && (
            <button type="button" className="walkin-btn" onClick={() => { onSelectCustomer(null); setCustQuery('') }}>
              Walk-in
            </button>
          )}
          {custOpen && (
            <div className="search-results" role="listbox">
              {custResults.length === 0 && <div className="search-status">No customers match.</div>}
              {custResults.map((c) => (
                <button
                  key={c.id}
                  type="button"
                  className="search-result"
                  onClick={() => {
                    onSelectCustomer(c)
                    setCustQuery('')
                    setCustOpen(false)
                  }}
                >
                  <span className="sr-sku">{c.account_number}</span>
                  <span className="sr-name">{displayName(c)}</span>
                  <span className="sr-price">{c.customer_type}</span>
                </button>
              ))}
            </div>
          )}
        </div>
        {customer ? (
          <div className="account-block">
            <div className="cust-tier">{level?.name ?? 'Retail'} · {customer.account_number}</div>
            <div className="cust-name">{displayName(customer)}</div>
            <div className="cust-sub">{customer.email ?? customer.phone ?? 'No contact on file'}</div>
            {summary && (
              <>
                <div className="limit-row">
                  <span>Balance {fmtMoney(summary.balance)}</span>
                  <span>Limit {fmtMoney(summary.credit_limit)}</span>
                </div>
                <div className="limit-bar" aria-label={`Credit used ${limitPct.toFixed(0)} percent`}>
                  <span style={{ width: `${limitPct}%` }} />
                </div>
                <div className="limit-row">
                  <span>Available credit {fmtMoney(cad(availNum.toFixed(4)))}</span>
                  <span>Terms {customer.payment_terms ?? '—'}</span>
                </div>
                {summary.credit_hold && <div className="credit-hold">CREDIT HOLD — charging is blocked</div>}
              </>
            )}
            <div className="cust-rows">
              <span>Saved card</span>
              <span>—</span>
            </div>
            {customer.email && (
              <label className="email-invoices">
                <input type="checkbox" checked={emailInvoices} onChange={(e) => onEmailInvoices(e.target.checked)} />
                Email invoices to {customer.email}
              </label>
            )}
            <button type="button" className="history-btn" onClick={onHistory}>
              Purchase history
            </button>
          </div>
        ) : (
          <p className="hint">Walk-in sale — no account. Charge to account needs a customer.</p>
        )}
        <label className="price-level">
          Price level
          <select value={priceLevelId} onChange={(e) => onPriceLevel(e.target.value)}>
            {priceLevels.map((l) => (
              <option key={l.id} value={l.id}>
                {l.name} ({Number(l.discount_pct) > 0 ? `${Math.round(Number(l.discount_pct) * 100)}% off list` : 'list'})
              </option>
            ))}
          </select>
        </label>
      </section>

      <section className="till-section till-totals">
        <h2>Totals</h2>
        {preview ? (
          <>
            <div className="total-row"><span>Subtotal</span><strong>{fmtMoney(preview.subtotal)}</strong></div>
            {Number(discountsTotal) !== 0 && (
              <div className="total-row"><span>Discounts</span><strong>−{fmtMoney(cad(discountsTotal))}</strong></div>
            )}
            <div className="total-row"><span>Eco fees ({paintItemCount} paint item{paintItemCount === 1 ? '' : 's'})</span><strong>{fmtMoney(preview.eco_fee_total)}</strong></div>
            <div className="total-row">
              <span>Delivery</span>
              <input
                className="delivery-input"
                value={deliveryFee}
                inputMode="decimal"
                aria-label="Delivery fee"
                disabled={fulfilment !== 'delivery'}
                onChange={(e) => onDeliveryFee(e.target.value.replace(/[^0-9.]/g, ''))}
              />
            </div>
            <div className="total-row"><span>Tax (GST+PST)</span><strong>{fmtMoney(preview.tax_total)}</strong></div>
            {Number(preview.pos_fee_total.amount) !== 0 && (
              <div className="total-row"><span>POS fee</span><strong>{fmtMoney(preview.pos_fee_total)}</strong></div>
            )}
            <div className="total-row grand"><span>Total due</span><strong>{fmtMoney(cad(grandDue))}</strong></div>
          </>
        ) : (
          <p className="hint">Add lines to see totals — tax, eco fees and the total come from the server.</p>
        )}
      </section>

      <section className="till-section till-actions">
        <div className="actions-grid">
          <button type="button" onClick={onVoid} disabled={saleEmpty}>Void</button>
          <button type="button" onClick={onHold} disabled={saleEmpty}>Hold sale (F7)</button>
          <button type="button" onClick={onQuote} disabled={saleEmpty}>Save as quote</button>
          <button type="button" onClick={onOpenDrawer}>Open drawer</button>
          <button type="button" className="complete-btn" onClick={onComplete} disabled={completeDisabled}>
            Complete sale · {fmtMoney(cad(grandDue))}
          </button>
        </div>
        <div className="till-foot">
          <span>Drawer: {tillOpen ? `Open · ${fmtMoney(cad(openingFloat || '0.0000'))} float` : 'Closed'}</span>
          <span>Terminal 1: Ready</span>
        </div>
      </section>

      <details className="till-section till-sync">
        <summary>Till &amp; sync</summary>
        {!tillOpen ? (
          <div className="till-open">
            <label>
              Opening float
              <input value={openingFloat} onChange={(e) => onOpeningFloat(e.target.value)} inputMode="decimal" />
            </label>
            <button type="button" onClick={onOpenTill}>Open till</button>
          </div>
        ) : (
          <div className="till-open">
            <label>
              Counted cash
              <input value={countedCash} onChange={(e) => onCountedCash(e.target.value)} inputMode="decimal" />
            </label>
            <button type="button" onClick={onCloseTill}>Close till (Z-report)</button>
          </div>
        )}
        {zReport && (
          <div className="totals">
            <div><span>Sales</span><strong>{zReport.sale_count} — {fmtMoney(zReport.gross_sales)}</strong></div>
            <div><span>Cash</span><strong>{fmtMoney(zReport.cash_sales)}</strong></div>
            <div><span>Card</span><strong>{fmtMoney(zReport.card_sales)}</strong></div>
            <div><span>Charge</span><strong>{fmtMoney(zReport.charge_sales)}</strong></div>
            <div><span>Returns</span><strong>{zReport.return_count} — {fmtMoney(zReport.return_total)}</strong></div>
            <div><span>Expected cash</span><strong>{fmtMoney(zReport.expected_cash)}</strong></div>
            <div><span>Counted</span><strong>{fmtMoney(zReport.counted_cash)}</strong></div>
            <div className="grand"><span>Over / short</span><strong>{fmtMoney(zReport.over_short)}</strong></div>
          </div>
        )}
        <div className="sync-row">
          <span>Pending outbox: {pendingSync}</span>
          <button type="button" onClick={onSync}>Sync now</button>
        </div>
        <label>
          Payment provider
          <select value={providerSwitchable ? provider : 'mock'} disabled={!providerSwitchable} onChange={(e) => onProvider(e.target.value)}>
            <option value="mock">Mock terminal</option>
            <option value="standalone">Standalone terminal</option>
          </select>
        </label>
        {!providerSwitchable && <p className="hint">Provider switching needs the native Tauri app; the dev server uses its mock terminal.</p>}
      </details>
    </div>
  )
}
