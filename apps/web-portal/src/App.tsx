import { useCallback, useEffect, useState } from 'react'
import {
  createReorder,
  getPortalCustomer,
  getPortalFormulas,
  getPortalInvoices,
  getPortalPayments,
  getPortalStatements,
  payBill,
} from './api'
import type {
  BillPayResult,
  PortalCustomerView,
  PortalPaymentRow,
  PortalReorderResponse,
  StatementRow,
  TintFormula,
  Transaction,
} from './types'
import { customerDisplayName, fmtDecimal, fmtMoney } from './types'

// The customer id IS the portal scope: there is no separate portal
// login, and every call re-checks `portal_enabled` server-side
// (403 otherwise).
const STORAGE_KEY = 'portal-customer-id'
const DEMO_CUSTOMER_ID = '00000000-0000-7000-8000-000000000101'

type Tab = 'overview' | 'invoices' | 'statements' | 'formulas' | 'billpay'

function readInitialCustomerId(): string | null {
  const param = new URLSearchParams(window.location.search).get('customer')
  if (param) {
    window.localStorage.setItem(STORAGE_KEY, param)
    return param
  }
  return window.localStorage.getItem(STORAGE_KEY)
}

export default function App() {
  const [customerId, setCustomerId] = useState<string | null>(readInitialCustomerId)
  const [signInInput, setSignInInput] = useState('')
  const [view, setView] = useState<PortalCustomerView | null>(null)
  const [tab, setTab] = useState<Tab>('overview')
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const [invoices, setInvoices] = useState<Transaction[] | null>(null)
  const [statements, setStatements] = useState<StatementRow[] | null>(null)
  const [formulas, setFormulas] = useState<TintFormula[] | null>(null)
  const [payments, setPayments] = useState<PortalPaymentRow[] | null>(null)

  // Per-formula reorder form state.
  const [reorderQty, setReorderQty] = useState<Record<string, string>>({})
  const [reorderFulfillment, setReorderFulfillment] = useState<
    Record<string, 'Pickup' | 'Delivery'>
  >({})
  const [reorders, setReorders] = useState<PortalReorderResponse[]>([])

  // Bill-pay form state.
  const [payAmount, setPayAmount] = useState('150.0000')
  const [payMethod, setPayMethod] = useState<'card' | 'ach'>('card')
  const [lastPayment, setLastPayment] = useState<BillPayResult | null>(null)

  const say = useCallback((msg: string | null) => {
    setNotice(msg)
    setError(null)
  }, [])
  const fail = useCallback((err: unknown) => {
    setError(err instanceof Error ? err.message : String(err))
    setNotice(null)
  }, [])

  const loadCustomer = useCallback(
    async (id: string) => {
      setBusy(true)
      try {
        const v = await getPortalCustomer(id)
        setView(v)
        setCustomerId(id)
        window.localStorage.setItem(STORAGE_KEY, id)
        say(null)
      } catch (err) {
        fail(err)
      } finally {
        setBusy(false)
      }
    },
    [fail, say],
  )

  // Sign in automatically when an id comes from ?customer= or storage.
  useEffect(() => {
    if (customerId && !view) {
      void loadCustomer(customerId)
    }
  }, [customerId, view, loadCustomer])

  const signIn = async () => {
    const id = signInInput.trim()
    if (!id) {
      setError('Enter a customer id.')
      return
    }
    await loadCustomer(id)
  }

  const signOut = () => {
    window.localStorage.removeItem(STORAGE_KEY)
    setCustomerId(null)
    setView(null)
    setInvoices(null)
    setStatements(null)
    setFormulas(null)
    setPayments(null)
    setReorders([])
    setLastPayment(null)
    setSignInInput('')
    setTab('overview')
    say(null)
  }

  const loadTab = useCallback(
    async (next: Tab) => {
      if (!customerId) return
      setTab(next)
      setBusy(true)
      try {
        if (next === 'invoices') setInvoices(await getPortalInvoices(customerId))
        if (next === 'statements') setStatements(await getPortalStatements(customerId))
        if (next === 'formulas') setFormulas(await getPortalFormulas(customerId))
        if (next === 'billpay') setPayments(await getPortalPayments(customerId))
        say(null)
      } catch (err) {
        fail(err)
      } finally {
        setBusy(false)
      }
    },
    [customerId, fail, say],
  )

  const submitReorder = async (formula: TintFormula) => {
    if (!customerId) return
    const qty = (reorderQty[formula.id] ?? '1').trim()
    if (!qty || Number(qty) <= 0) {
      setError('Quantity must be a positive number of gallons.')
      return
    }
    setBusy(true)
    try {
      const created = await createReorder({
        customer_id: customerId,
        formula_id: formula.id,
        qty,
        job_id: null,
        fulfillment: reorderFulfillment[formula.id] ?? 'Pickup',
      })
      setReorders((prev) => [created, ...prev])
      say(
        `Order ${created.doc_number} created — ${fmtMoney(created.grand_total)} ` +
          `(${created.fulfillment}).`,
      )
    } catch (err) {
      fail(err)
    } finally {
      setBusy(false)
    }
  }

  const submitPayment = async () => {
    if (!customerId || !view) return
    const amount = payAmount.trim()
    if (!amount || Number(amount) <= 0) {
      setError('Payment amount must be positive.')
      return
    }
    setBusy(true)
    try {
      // One fresh idempotency key per payment attempt: a retry of
      // the same attempt replays the original payment instead of
      // posting twice.
      const result = await payBill({
        customer_id: customerId,
        amount: { amount, currency: view.open_balance.currency },
        method: payMethod,
        idempotency_key: crypto.randomUUID(),
      })
      setLastPayment(result)
      // Re-fetch the customer so the open balance shown is the
      // post-payment balance.
      const refreshed = await getPortalCustomer(customerId)
      setView(refreshed)
      setPayments(await getPortalPayments(customerId))
      say(
        result.replayed
          ? `Payment replayed (already posted) — ${fmtMoney(result.amount)}. ` +
              `Open balance is now ${fmtMoney(refreshed.open_balance)}.`
          : `Payment posted — ${fmtMoney(result.amount)}. ` +
              `Open balance is now ${fmtMoney(refreshed.open_balance)}.`,
      )
    } catch (err) {
      fail(err)
    } finally {
      setBusy(false)
    }
  }

  if (!view) {
    return (
      <div className="app">
        <header>
          <h1>Contractor Portal</h1>
          <div className="status">
            <span>Decor POS — 24/7 contractor portal (v0.6)</span>
          </div>
        </header>
        <main>
          <section>
            <h2>Sign in with your customer id</h2>
            <p className="hint">
              There is no separate portal login: your customer id is the
              scope. Every request re-checks that portal access is enabled
              for that customer — a customer without portal access is
              refused (403).
            </p>
            <div className="entry">
              <input
                value={signInInput}
                onChange={(e) => setSignInInput(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') void signIn()
                }}
                placeholder="00000000-0000-7000-8000-000000000101"
                aria-label="Customer id"
              />
              <button onClick={() => void signIn()} disabled={busy}>
                Sign in
              </button>
            </div>
            <p className="hint">
              Demo contractor (portal-enabled, seeded):{' '}
              <button
                className="linklike"
                onClick={() => setSignInInput(DEMO_CUSTOMER_ID)}
              >
                {DEMO_CUSTOMER_ID}
              </button>{' '}
              — or open the portal with{' '}
              <code>?customer={DEMO_CUSTOMER_ID}</code>
            </p>
            {error && <div className="error">{error}</div>}
            {notice && <div className="notice">{notice}</div>}
          </section>
        </main>
        <footer>
          24/7 self-serve: invoices, statements, formula reorders and bill
          pay — no counter visit needed.
        </footer>
      </div>
    )
  }

  const { customer, open_balance } = view
  const tabs: { id: Tab; label: string }[] = [
    { id: 'overview', label: 'Overview' },
    { id: 'invoices', label: 'Invoices' },
    { id: 'statements', label: 'Statements' },
    { id: 'formulas', label: 'Formulas' },
    { id: 'billpay', label: 'Bill Pay' },
  ]

  return (
    <div className="app">
      <header>
        <h1>Contractor Portal</h1>
        <div className="status">
          <span>{customerDisplayName(customer)}</span>
          <span>Account {customer.account_number}</span>
          <span>
            Open balance: <strong>{fmtMoney(open_balance)}</strong>
          </span>
          <button onClick={signOut}>Sign out</button>
        </div>
      </header>
      <nav className="tabs">
        {tabs.map((t) => (
          <button
            key={t.id}
            className={tab === t.id ? 'active' : ''}
            onClick={() => void loadTab(t.id)}
            disabled={busy && tab !== t.id}
          >
            {t.label}
          </button>
        ))}
      </nav>
      <main>
        {error && <div className="error">{error}</div>}
        {notice && <div className="notice">{notice}</div>}

        {tab === 'overview' && (
          <section>
            <h2>Overview — {customerDisplayName(customer)}</h2>
            <div className="totals">
              <div>
                <span>Open balance</span>
                <strong>{fmtMoney(open_balance)}</strong>
              </div>
              <div>
                <span>Credit limit</span>
                <strong>{fmtDecimal(customer.credit_limit, open_balance.currency === 'Cad' ? 'CAD' : 'USD')}</strong>
              </div>
              <div>
                <span>Payment terms</span>
                <strong>{customer.payment_terms}</strong>
              </div>
              <div>
                <span>Account</span>
                <strong>{customer.account_number}</strong>
              </div>
              <div>
                <span>Customer type</span>
                <strong>{customer.customer_type}</strong>
              </div>
              {customer.email && (
                <div>
                  <span>Email</span>
                  <strong>{customer.email}</strong>
                </div>
              )}
              {customer.phone && (
                <div>
                  <span>Phone</span>
                  <strong>{customer.phone}</strong>
                </div>
              )}
              <div>
                <span>Customer id</span>
                <strong>{customer.id}</strong>
              </div>
            </div>
            <p className="hint">
              Charge account{customer.credit_hold ? ' — on credit hold' : ''}.
              Use Bill Pay to pay down the open balance, Formulas to reorder
              a colour you have bought before, and Invoices / Statements for
              the paper trail.
            </p>
          </section>
        )}

        {tab === 'invoices' && (
          <section>
            <h2>Invoices</h2>
            {invoices === null ? (
              <p className="hint">Loading…</p>
            ) : invoices.length === 0 ? (
              <p className="hint">No invoices yet.</p>
            ) : (
              <table>
                <thead>
                  <tr>
                    <th>Doc</th>
                    <th>Type</th>
                    <th>Status</th>
                    <th>Total</th>
                    <th>Created</th>
                  </tr>
                </thead>
                <tbody>
                  {invoices.map((tx) => (
                    <tr key={tx.id}>
                      <td>{tx.doc_number}</td>
                      <td>{tx.doc_type}</td>
                      <td>{tx.status}</td>
                      <td>{fmtDecimal(tx.grand_total, tx.currency)}</td>
                      <td>{tx.created_at}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </section>
        )}

        {tab === 'statements' && (
          <section>
            <h2>Statements</h2>
            {statements === null ? (
              <p className="hint">Loading…</p>
            ) : statements.length === 0 ? (
              <p className="hint">
                No statements yet — statements are frozen at month-end.
              </p>
            ) : (
              <table>
                <thead>
                  <tr>
                    <th>Period</th>
                    <th>Opening</th>
                    <th>Charges</th>
                    <th>Payments</th>
                    <th>Closing</th>
                  </tr>
                </thead>
                <tbody>
                  {statements.map((s) => (
                    <tr key={s.id}>
                      <td>
                        {s.period_start} → {s.period_end}
                      </td>
                      <td>{fmtDecimal(s.opening_balance)}</td>
                      <td>{fmtDecimal(s.charges_total)}</td>
                      <td>{fmtDecimal(s.payments_total)}</td>
                      <td>
                        <strong>{fmtDecimal(s.closing_balance)}</strong>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </section>
        )}

        {tab === 'formulas' && (
          <section>
            <h2>Formulas — your colour history</h2>
            <p className="hint">
              A reorder looks up the stored formula and re-creates the sale
              line for it — same base product, priced through the normal
              pricing path. Formula tracking only: the portal never drives
              a tint machine.
            </p>
            {formulas === null ? (
              <p className="hint">Loading…</p>
            ) : formulas.length === 0 ? (
              <p className="hint">No stored formulas yet.</p>
            ) : (
              formulas.map((f) => (
                <div className="card" key={f.id}>
                  <h3>
                    {f.color_code_snapshot} — {f.color_name_snapshot}
                  </h3>
                  <p className="hint">
                    Sheen: {f.sheen ?? '—'}
                    {f.room_or_usage ? ` · Use: ${f.room_or_usage}` : ''}
                    {' · '}Dispensed: {f.dispensed_at}
                  </p>
                  <div className="entry">
                    <label>
                      Qty (gal)
                      <input
                        value={reorderQty[f.id] ?? '1'}
                        onChange={(e) =>
                          setReorderQty((prev) => ({ ...prev, [f.id]: e.target.value }))
                        }
                      />
                    </label>
                    <label>
                      Fulfillment
                      <select
                        value={reorderFulfillment[f.id] ?? 'Pickup'}
                        onChange={(e) =>
                          setReorderFulfillment((prev) => ({
                            ...prev,
                            [f.id]: e.target.value as 'Pickup' | 'Delivery',
                          }))
                        }
                      >
                        <option value="Pickup">Pickup</option>
                        <option value="Delivery">Delivery</option>
                      </select>
                    </label>
                    <button onClick={() => void submitReorder(f)} disabled={busy}>
                      Reorder
                    </button>
                  </div>
                </div>
              ))
            )}
            {reorders.length > 0 && (
              <>
                <h3>Orders created this session</h3>
                <table>
                  <thead>
                    <tr>
                      <th>Order</th>
                      <th>Fulfillment</th>
                      <th>Total</th>
                      <th>Order id</th>
                    </tr>
                  </thead>
                  <tbody>
                    {reorders.map((r) => (
                      <tr key={r.order_id}>
                        <td>{r.doc_number}</td>
                        <td>{r.fulfillment}</td>
                        <td>{fmtMoney(r.grand_total)}</td>
                        <td>{r.order_id}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </>
            )}
          </section>
        )}

        {tab === 'billpay' && (
          <section>
            <h2>Bill Pay</h2>
            <p className="hint">
              Payments are authorized by the mock provider in dev — no live
              payment API, no card data stored — and no surcharge is applied
              to bill-pay in v0.6. A payment applies to your oldest open
              charges first (FIFO). Each attempt carries its own
              idempotency key, so a retried payment posts only once.
            </p>
            <div className="entry">
              <label>
                Amount ({open_balance.currency === 'Cad' ? 'CAD' : 'USD'})
                <input value={payAmount} onChange={(e) => setPayAmount(e.target.value)} />
              </label>
              <label>
                Method
                <select
                  value={payMethod}
                  onChange={(e) => setPayMethod(e.target.value as 'card' | 'ach')}
                >
                  <option value="card">Card</option>
                  <option value="ach">ACH</option>
                </select>
              </label>
              <button onClick={() => void submitPayment()} disabled={busy}>
                Pay {payAmount ? fmtDecimal(payAmount, open_balance.currency === 'Cad' ? 'CAD' : 'USD') : ''}
              </button>
            </div>
            <p className="hint">
              Open balance before payment: <strong>{fmtMoney(open_balance)}</strong>
            </p>
            {lastPayment && (
              <div className="card">
                <h3>
                  Payment {fmtMoney(lastPayment.amount)} — {lastPayment.method}
                  {lastPayment.replayed ? ' (replayed — already posted)' : ''}
                </h3>
                <div className="totals">
                  <div>
                    <span>Payment entry</span>
                    <strong>{lastPayment.payment_entry_id}</strong>
                  </div>
                  <div>
                    <span>Provider reference</span>
                    <strong>{lastPayment.provider_reference}</strong>
                  </div>
                </div>
                <h3>FIFO allocations (oldest charge first)</h3>
                {lastPayment.allocations.length === 0 ? (
                  <p className="hint">No open charges to allocate against.</p>
                ) : (
                  <table>
                    <thead>
                      <tr>
                        <th>Charge entry</th>
                        <th>Allocated</th>
                      </tr>
                    </thead>
                    <tbody>
                      {lastPayment.allocations.map(([chargeId, amount]) => (
                        <tr key={chargeId}>
                          <td>{chargeId}</td>
                          <td>{fmtDecimal(amount, lastPayment.amount.currency === 'Cad' ? 'CAD' : 'USD')}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </div>
            )}
            <h3>Past payments</h3>
            {payments === null ? (
              <p className="hint">Loading…</p>
            ) : payments.length === 0 ? (
              <p className="hint">No portal payments yet.</p>
            ) : (
              <table>
                <thead>
                  <tr>
                    <th>When</th>
                    <th>Amount</th>
                    <th>Method</th>
                    <th>Provider reference</th>
                  </tr>
                </thead>
                <tbody>
                  {payments.map((p) => (
                    <tr key={p.idempotency_key}>
                      <td>{p.created_at}</td>
                      <td>{fmtDecimal(p.amount)}</td>
                      <td>{p.method}</td>
                      <td>{p.provider_reference}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </section>
        )}
      </main>
      <footer>
        Contractor Portal v0.6 — scoped by customer id; every call
        re-checks portal access server-side.
      </footer>
    </div>
  )
}
