// Accounts — charge accounts. Ports the back-office 'ar' tab
// (aging, collections) and the customer side of 'crm' (search,
// price level, purchase history). Taking a payment posts to the
// AR ledger through the new POST /ar/payments route — a recorded
// ledger posting with a method label, not a card authorization
// (the portal's bill-pay route is portal-gated and forces card
// auth, so it is not used here). History and invoice reprint reuse
// the register's HistoryModal / InvoiceView.

import { useCallback, useEffect, useState } from 'react'
import {
  getArAging,
  getArCollections,
  getCustomerSummary,
  getPriceLevels,
  postArPayment,
  searchCustomers,
  setCustomerPriceLevel,
  type AgingView,
  type CollectionsView,
} from '../api'
import { cad, fmtMoney, type ArCustomerSummary, type CustomerSearchResult, type InvoiceData, type PriceLevel } from '../types'
import HistoryModal from './HistoryModal'
import InvoiceView from './InvoiceView'

const PAYMENT_METHODS = ['Cash', 'Card', 'E-Transfer', 'Cheque']

function displayName(c: CustomerSearchResult): string {
  return c.company_name ?? [c.first_name, c.last_name].filter(Boolean).join(' ') ?? c.account_number
}

export default function AccountsScreen({
  storeId,
  onToast,
}: {
  storeId: string | null
  onToast: (msg: string) => void
}) {
  const [aging, setAging] = useState<AgingView | null>(null)
  const [collections, setCollections] = useState<CollectionsView | null>(null)
  const [arTab, setArTab] = useState<'aging' | 'collections'>('aging')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const [query, setQuery] = useState('')
  const [results, setResults] = useState<CustomerSearchResult[]>([])
  const [searching, setSearching] = useState(false)
  const [selected, setSelected] = useState<CustomerSearchResult | null>(null)
  const [summary, setSummary] = useState<ArCustomerSummary | null>(null)
  const [levels, setLevels] = useState<PriceLevel[]>([])

  const [payAmount, setPayAmount] = useState('')
  const [payMethod, setPayMethod] = useState('Cash')
  const [busy, setBusy] = useState(false)

  const [historyOpen, setHistoryOpen] = useState(false)
  const [invoice, setInvoice] = useState<{ data: InvoiceData; reprint: boolean } | null>(null)

  const loadAr = useCallback(async () => {
    if (!storeId) return
    setLoading(true)
    setError(null)
    try {
      const [a, c] = await Promise.all([getArAging(storeId), getArCollections(storeId)])
      setAging(a)
      setCollections(c)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [storeId])

  useEffect(() => {
    void loadAr()
    void getPriceLevels().then(setLevels).catch(() => setLevels([]))
  }, [loadAr])

  const loadSummary = useCallback(async (id: string) => {
    try {
      setSummary(await getCustomerSummary(id))
    } catch {
      setSummary(null)
    }
  }, [])

  const fail = (e: unknown) => setError(e instanceof Error ? e.message : String(e))

  async function doSearch(q: string) {
    setQuery(q)
    if (q.trim().length < 2) {
      setResults([])
      return
    }
    setSearching(true)
    try {
      setResults(await searchCustomers(q.trim()))
    } catch {
      setResults([])
    } finally {
      setSearching(false)
    }
  }

  function selectCustomer(c: CustomerSearchResult) {
    setSelected(c)
    setSummary(null)
    setPayAmount('')
    void loadSummary(c.id)
  }

  async function selectById(id: string, name: string) {
    // Aging rows carry only an id + name — resolve the full
    // customer record through search before opening the detail.
    const found = await searchCustomers(name)
    const match = found.find((c) => c.id === id) ?? found[0]
    if (match) selectCustomer(match)
    else setError(`Could not load ${name} — try searching for them below.`)
  }

  async function doSetPriceLevel(levelId: string) {
    if (!selected) return
    setBusy(true)
    setError(null)
    try {
      const updated = await setCustomerPriceLevel(selected.id, levelId || null)
      setSelected(updated)
      onToast(`Price level set to ${levels.find((l) => l.id === levelId)?.name ?? 'Retail (default)'}.`)
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doPayment() {
    if (!selected) return
    const amount = Number(payAmount)
    if (!Number.isFinite(amount) || amount <= 0) {
      setError('Enter a payment amount above zero.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const posted = await postArPayment(selected.id, cad(payAmount), payMethod)
      setPayAmount('')
      await loadSummary(selected.id)
      await loadAr()
      onToast(
        `Payment of ${fmtMoney(posted.amount)} (${posted.method}) recorded — applied to ${posted.allocations.length} open item${posted.allocations.length === 1 ? '' : 's'}.`,
      )
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  if (!storeId) return <section className="screen"><p className="screen-empty">Loading store…</p></section>

  const balance = summary?.balance
  const levelName = levels.find((l) => l.id === selected?.price_level_id)?.name ?? 'Retail (default)'

  return (
    <section className="screen" aria-label="Accounts">
      <div className="screen-head">
        <h2>Accounts</h2>
        <div className="screen-actions">
          <button type="button" className="screen-btn" disabled={loading} onClick={() => void loadAr()}>
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

      <div className="screen-panel">
        <div className="screen-tabs" role="tablist" aria-label="Receivables">
          <button
            type="button"
            role="tab"
            aria-selected={arTab === 'aging'}
            className={arTab === 'aging' ? 'active' : ''}
            onClick={() => setArTab('aging')}
          >
            Aging {aging ? `— ${fmtMoney(cad(aging.totals.reduce((sum, m) => String(Number(sum) + Number(m.amount)), '0')))}` : ''}
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={arTab === 'collections'}
            className={arTab === 'collections' ? 'active' : ''}
            onClick={() => setArTab('collections')}
          >
            Collections {collections ? `— ${collections.customers.length} account${collections.customers.length === 1 ? '' : 's'}` : ''}
          </button>
        </div>
        {loading && !aging ? (
          <p className="screen-empty">Loading…</p>
        ) : arTab === 'aging' ? (
          !aging || aging.customers.length === 0 ? (
            <p className="screen-empty">No open charge balances.</p>
          ) : (
            <table className="screen-table screen-table-clickable">
              <thead>
                <tr>
                  <th>Account</th>
                  <th className="num">Total</th>
                  <th className="num">Current</th>
                  <th className="num">31–60</th>
                  <th className="num">61–90</th>
                  <th className="num">91–120</th>
                  <th className="num">120+</th>
                </tr>
              </thead>
              <tbody>
                {aging.customers.map((c) => (
                  <tr key={c.customer_id} onClick={() => void selectById(c.customer_id, c.account_number)}>
                    <td className="mono">{c.account_number}</td>
                    <td className="num">{fmtMoney(c.balance)}</td>
                    <td className="num">{fmtMoney(c.buckets[0])}</td>
                    <td className="num">{fmtMoney(c.buckets[1])}</td>
                    <td className="num">{fmtMoney(c.buckets[2])}</td>
                    <td className="num">{fmtMoney(c.buckets[3])}</td>
                    <td className="num">{fmtMoney(c.buckets[4])}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )
        ) : !collections || collections.customers.length === 0 ? (
          <p className="screen-empty">Nothing in collections.</p>
        ) : (
          <table className="screen-table screen-table-clickable">
            <thead>
              <tr>
                <th>Customer</th>
                <th className="num">Balance</th>
                <th className="num">Overdue</th>
                <th>Oldest invoice</th>
              </tr>
            </thead>
            <tbody>
              {collections.customers.map((c) => (
                <tr key={c.customer_id} onClick={() => void selectById(c.customer_id, c.company_name ?? c.account_number)}>
                  <td>{c.company_name ?? c.account_number}</td>
                  <td className="num">{fmtMoney(c.balance)}</td>
                  <td className="num">{fmtMoney(c.overdue)}</td>
                  <td>{c.oldest_due_date ? new Date(c.oldest_due_date).toLocaleDateString() : '—'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      <div className="screen-split">
        <div className="screen-panel">
          <h3>Find a customer</h3>
          <input
            type="search"
            className="screen-input"
            value={query}
            placeholder="Search by name, company, account #…"
            aria-label="Search customers"
            onChange={(e) => void doSearch(e.target.value)}
          />
          {searching ? (
            <p className="screen-empty">Searching…</p>
          ) : query.trim().length >= 2 && results.length === 0 ? (
            <p className="screen-empty">No customers match “{query.trim()}”.</p>
          ) : (
            <ul className="screen-list">
              {results.map((c) => (
                <li key={c.id}>
                  <button
                    type="button"
                    className={selected?.id === c.id ? 'selected' : ''}
                    onClick={() => selectCustomer(c)}
                  >
                    <span>{displayName(c)}</span>
                    <span className="mono">{c.account_number}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>

        <div className="screen-panel">
          <h3>Account detail</h3>
          {!selected ? (
            <p className="screen-empty">Search above, or pick a row in the aging table.</p>
          ) : (
            <>
              <p className="screen-meta">
                {displayName(selected)} · <span className="mono">{selected.account_number}</span>
                {selected.credit_hold ? ' · CREDIT HOLD' : ''}
                {!selected.charge_enabled ? ' · Charge disabled' : ''}
              </p>
              <dl className="screen-facts">
                <div>
                  <dt>Balance</dt>
                  <dd>{balance ? fmtMoney(balance) : '—'}</dd>
                </div>
                <div>
                  <dt>Credit limit</dt>
                  <dd>{fmtMoney(selected.credit_limit)}</dd>
                </div>
                <div>
                  <dt>Terms</dt>
                  <dd>{selected.payment_terms ?? '—'}</dd>
                </div>
                <div>
                  <dt>Price level</dt>
                  <dd>{levelName}</dd>
                </div>
                {summary?.oldest_due_date && (
                  <div>
                    <dt>Oldest due</dt>
                    <dd>{new Date(summary.oldest_due_date).toLocaleDateString()}</dd>
                  </div>
                )}
              </dl>

              <div className="screen-form-row">
                <label>
                  Price level
                  <select
                    className="screen-input"
                    value={selected.price_level_id ?? ''}
                    disabled={busy}
                    onChange={(e) => void doSetPriceLevel(e.target.value)}
                  >
                    <option value="">Retail (default)</option>
                    {levels.map((l) => (
                      <option key={l.id} value={l.id}>
                        {l.name}
                      </option>
                    ))}
                  </select>
                </label>
                <button
                  type="button"
                  className="screen-btn"
                  onClick={() => setHistoryOpen(true)}
                >
                  Purchase history
                </button>
              </div>

              <div className="screen-sub">
                <h4>Take a payment</h4>
                <div className="screen-form-row">
                  <label>
                    Amount
                    <input
                      type="text"
                      inputMode="decimal"
                      className="screen-input screen-qty"
                      value={payAmount}
                      placeholder="0.00"
                      disabled={busy}
                      aria-label="Payment amount"
                      onChange={(e) => setPayAmount(e.target.value)}
                    />
                  </label>
                  <label>
                    Method
                    <select
                      className="screen-input"
                      value={payMethod}
                      disabled={busy}
                      onChange={(e) => setPayMethod(e.target.value)}
                    >
                      {PAYMENT_METHODS.map((m) => (
                        <option key={m} value={m}>{m}</option>
                      ))}
                    </select>
                  </label>
                  <button
                    type="button"
                    className="screen-btn screen-btn-primary"
                    disabled={busy || !payAmount}
                    onClick={() => void doPayment()}
                  >
                    Record payment
                  </button>
                </div>
                <p className="screen-note">
                  Recorded against the account ledger (oldest invoices first). This is not a card
                  authorization.
                </p>
              </div>
            </>
          )}
        </div>
      </div>

      {historyOpen && selected && (
        <HistoryModal
          customer={selected}
          onReprint={(inv) => {
            setHistoryOpen(false)
            setInvoice({ data: inv, reprint: true })
          }}
          onClose={() => setHistoryOpen(false)}
          onToast={onToast}
        />
      )}
      {invoice && (
        <InvoiceView
          invoice={invoice.data}
          reprint={invoice.reprint}
          autoEmail={false}
          onClose={() => setInvoice(null)}
          onNewSale={() => setInvoice(null)}
          onToast={onToast}
        />
      )}
    </section>
  )
}
