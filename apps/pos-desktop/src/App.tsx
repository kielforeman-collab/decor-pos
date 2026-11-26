import { useCallback, useEffect, useRef, useState } from 'react'
import {
  checkout,
  closeTill,
  getStatus,
  isTauri,
  lastReceiptText,
  listHolds,
  openTill,
  parkHold,
  pricePreview,
  providerSwitchable,
  recordLayaway,
  recordQuote,
  recordReturn,
  resumeHold,
  scanBarcode,
  setProvider,
  syncRun,
  syncState,
  voidTransaction,
} from './api'
import { DEMO_CATALOG, findBySku } from './catalog'
import {
  canPriceOverride,
  canVoid,
  getSession,
  logout,
  type AuthSession,
} from './auth'
import Login from './Login'
import OverridePrompt from './OverridePrompt'
import type {
  CompletedSale,
  Money,
  PricePreview,
  SaleInput,
  SaleLineInput,
  SyncState,
  TenderInput,
  TenderKind,
  ZReport,
} from './types'
import { fmtMoney, usd } from './types'

interface CartLine {
  key: string
  product_id: string
  sku: string
  description: string
  qty: string
  uom: string
  unit_price: string
  unit_cost: string
}

interface TenderRow {
  key: string
  kind: TenderKind
  amount: string // '' = remaining balance
}

let keySeq = 0
const nextKey = () => `k${++keySeq}`

const TENDER_LABELS: Record<TenderKind, string> = {
  Cash: 'Cash',
  Card: 'Card',
  ChargeAccount: 'Charge account',
  GiftCard: 'Gift card',
}

function cartToSaleInput(
  lines: CartLine[],
  tenders: TenderRow[],
  customerId: string,
  poNumber: string,
): SaleInput {
  return {
    customer_id: customerId.trim() || null,
    job_id: null,
    lines: lines.map(
      (l): SaleLineInput => ({
        product_id: l.product_id,
        sku: l.sku,
        description: l.description,
        qty: l.qty,
        uom: l.uom,
        unit_price: usd(l.unit_price),
        unit_cost: usd(l.unit_cost),
      }),
    ),
    tenders: tenders.map(
      (t): TenderInput => ({
        kind: t.kind,
        amount: t.amount.trim() === '' ? null : usd(t.amount),
      }),
    ),
    po_number: poNumber.trim() || null,
  }
}

export default function App() {
  // Phase 7: no session → login screen. The session is restored from
  // localStorage on reload; a 401 from any API call clears it (see
  // api.ts) and the 'decor-pos-unauthorized' listener below returns
  // here.
  const [session, setSession] = useState<AuthSession | null>(() => getSession())
  const [switching, setSwitching] = useState(false)
  const [overrideRequest, setOverrideRequest] = useState<{
    reason: 'void' | 'price_override'
    label: string
  } | null>(null)
  const [pendingEntry, setPendingEntry] = useState<{
    sku: string
    description: string
    qty: string
    unit_price: string
    unit_cost: string
  } | null>(null)
  const [storeName, setStoreName] = useState('Decor POS')
  const [provider, setProviderState] = useState('mock')
  const [sync, setSync] = useState<SyncState | null>(null)
  const [tillId, setTillId] = useState<string | null>(null)
  const [openingFloat, setOpeningFloat] = useState('150.0000')
  const [countedCash, setCountedCash] = useState('')
  const [zReport, setZReport] = useState<ZReport | null>(null)

  const [lines, setLines] = useState<CartLine[]>([])
  const [preview, setPreview] = useState<PricePreview | null>(null)
  const [tenders, setTenders] = useState<TenderRow[]>([{ key: nextKey(), kind: 'Cash', amount: '' }])
  const [customerId, setCustomerId] = useState('')
  const [poNumber, setPoNumber] = useState('')

  const [scan, setScan] = useState('')
  const [entry, setEntry] = useState({ sku: '', description: '', qty: '1', unit_price: '', unit_cost: '' })
  const [lastSale, setLastSale] = useState<CompletedSale | null>(null)
  const [receiptText, setReceiptText] = useState<string | null>(null)
  const [holds, setHolds] = useState<string[]>([])
  const [notice, setNotice] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const [returnTxId, setReturnTxId] = useState('')
  const [returnSku, setReturnSku] = useState(DEMO_CATALOG[0].sku)
  const [returnQty, setReturnQty] = useState('1')
  const [layawayDeposit, setLayawayDeposit] = useState('')
  const [layawayKind, setLayawayKind] = useState<TenderKind>('Cash')

  const scanRef = useRef<HTMLInputElement>(null)

  const refreshStatus = useCallback(async () => {
    try {
      const status = await getStatus()
      setStoreName(status.store_name)
      setProviderState(status.provider)
      const state = await syncState()
      setSync(state)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [])

  useEffect(() => {
    if (!session) return
    refreshStatus()
  }, [refreshStatus, session])

  useEffect(() => {
    const onUnauthorized = () => {
      setSession(null)
      setError('Session expired — sign in again.')
    }
    window.addEventListener('decor-pos-unauthorized', onUnauthorized)
    return () => window.removeEventListener('decor-pos-unauthorized', onUnauthorized)
  }, [])

  // Live totals: preview with no tenders (surcharge is computed at
  // charge time from the selected tenders; the server is authoritative).
  useEffect(() => {
    if (!session) return
    if (lines.length === 0) {
      setPreview(null)
      return
    }
    const handle = setTimeout(() => {
      const input = cartToSaleInput(lines, [], '', '')
      pricePreview(input)
        .then(setPreview)
        .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
    }, 250)
    return () => clearTimeout(handle)
  }, [lines, session])

  // Keyboard-first: `/` focuses the scanner, Ctrl+Enter charges.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === '/' && document.activeElement !== scanRef.current) {
        e.preventDefault()
        scanRef.current?.focus()
      }
      if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
        e.preventDefault()
        void doCheckout()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [lines, tenders, customerId, poNumber, tillId])

  const say = (msg: string) => {
    setNotice(msg)
    setError(null)
  }
  const fail = (e: unknown) => {
    setError(e instanceof Error ? e.message : String(e))
    setNotice(null)
  }

  function addLine(product: {
    product_id: string
    sku: string
    description: string
    uom: string
    unit_price: string
    unit_cost: string
  }, qty: string) {
    setLines((prev) => {
      const existing = prev.find((l) => l.product_id === product.product_id)
      if (existing) {
        return prev.map((l) =>
          l.key === existing.key
            ? { ...l, qty: String(Number(l.qty) + Number(qty)) }
            : l,
        )
      }
      return [...prev, { key: nextKey(), ...product, qty }]
    })
  }

  async function doScan() {
    const raw = scan.trim()
    if (!raw) return
    setScan('')
    try {
      const result = await scanBarcode(raw)
      const product = DEMO_CATALOG.find((p) => p.barcode === result.barcode)
      if (!product) throw new Error(`Unknown barcode: ${result.barcode}`)
      addLine(product, result.qty)
    } catch (e) {
      fail(e)
    }
    scanRef.current?.focus()
  }

  function commitEntry(
    known: ReturnType<typeof findBySku>,
    snapshot: { sku: string; description: string; qty: string; unit_price: string; unit_cost: string },
  ) {
    addLine(
      {
        product_id: known?.product_id ?? `00000000-0000-7000-8000-${snapshot.sku.slice(0, 12).padEnd(12, '0')}`,
        sku: snapshot.sku,
        description: snapshot.description,
        uom: known?.uom ?? 'ea',
        unit_price: snapshot.unit_price,
        unit_cost: snapshot.unit_cost,
      },
      snapshot.qty,
    )
    setEntry({ sku: '', description: '', qty: '1', unit_price: '', unit_cost: '' })
  }

  function doAddEntry() {
    const sku = entry.sku.trim()
    if (!sku || !entry.description.trim() || !entry.unit_price) {
      setError('SKU, description and unit price are required to add a line.')
      return
    }
    const known = findBySku(sku)
    const unitCost = entry.unit_cost || known?.unit_cost || '0.0000'
    const snapshot = {
      sku,
      description: entry.description.trim(),
      qty: entry.qty || '1',
      unit_price: entry.unit_price,
      unit_cost: unitCost,
    }
    // Below-cost price on a known product: manager override, unless
    // the signed-in user already holds PosPriceOverride.
    if (
      known &&
      Number(unitCost) > 0 &&
      Number(entry.unit_price) < Number(unitCost) &&
      !canPriceOverride(session?.user.role ?? '')
    ) {
      requestPriceOverride(snapshot)
      return
    }
    commitEntry(known, snapshot)
  }

  async function doCheckout() {
    if (lines.length === 0) return
    if (!tillId) {
      setError('Open a till before charging a sale.')
      return
    }
    setBusy(true)
    try {
      const sale = await checkout(cartToSaleInput(lines, tenders, customerId, poNumber))
      setLastSale(sale)
      setLines([])
      setTenders([{ key: nextKey(), kind: 'Cash', amount: '' }])
      const text = await lastReceiptText()
      setReceiptText(text)
      say(`Sale ${sale.doc_number} complete — ${fmtMoney(sale.grand_total)}${sale.change_due.amount !== '0.0000' ? `, change due ${fmtMoney(sale.change_due)}` : ''}`)
      await refreshStatus()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doParkHold() {
    if (lines.length === 0) return
    try {
      const { hold_id } = await parkHold(cartToSaleInput(lines, tenders, customerId, poNumber))
      setLines([])
      setHolds((prev) => [...prev, hold_id])
      say(`Hold parked (${hold_id.slice(0, 8)}).`)
    } catch (e) {
      fail(e)
    }
  }

  async function doResumeHold(holdId: string) {
    try {
      const input = await resumeHold(holdId)
      setLines(
        input.lines.map((l) => ({
          key: nextKey(),
          product_id: l.product_id,
          sku: l.sku,
          description: l.description,
          qty: l.qty,
          uom: l.uom,
          unit_price: l.unit_price.amount,
          unit_cost: l.unit_cost.amount,
        })),
      )
      setTenders(
        input.tenders.map((t) => ({
          key: nextKey(),
          kind: t.kind,
          amount: t.amount ? t.amount.amount : '',
        })),
      )
      setCustomerId(input.customer_id ?? '')
      setHolds((prev) => prev.filter((h) => h !== holdId))
      say('Hold resumed.')
    } catch (e) {
      fail(e)
    }
  }

  async function doQuote() {
    if (lines.length === 0) return
    try {
      const { transaction_id } = await recordQuote(cartToSaleInput(lines, [], customerId, poNumber))
      say(`Quote recorded (${transaction_id.slice(0, 8)}). No inventory moved.`)
    } catch (e) {
      fail(e)
    }
  }

  async function doLayaway() {
    if (lines.length === 0 || !layawayDeposit) return
    try {
      const sale = cartToSaleInput(lines, [], customerId, poNumber)
      const { transaction_id } = await recordLayaway(sale, {
        kind: layawayKind,
        amount: usd(layawayDeposit),
      })
      const grand = preview?.grand_total.amount ?? '?'
      say(
        `Layaway ${transaction_id.slice(0, 8)} recorded — deposit ${fmtMoney(usd(layawayDeposit))} of ${fmtMoney(usd(grand))}.`,
      )
      setLines([])
    } catch (e) {
      fail(e)
    }
  }

  async function doReturn() {
    if (!returnTxId.trim()) return
    const product = findBySku(returnSku)
    if (!product) {
      setError(`Unknown SKU: ${returnSku}`)
      return
    }
    try {
      const ret = await recordReturn(returnTxId.trim(), [
        { product_id: product.product_id, qty: returnQty },
      ])
      setLastSale(ret)
      say(`Return ${ret.doc_number} recorded — ${fmtMoney(ret.grand_total)}.`)
      await refreshStatus()
    } catch (e) {
      fail(e)
    }
  }

  async function doOpenTill() {
    try {
      // Phase 7: the signed-in operator opens the till under their own
      // user id (was a fixed demo id before login existed).
      const cashierId = session?.user.id ?? '00000000-0000-7000-8000-000000000099'
      const till = await openTill(cashierId, usd(openingFloat || '0.0000'))
      setTillId(till.id)
      say(`Till open — float ${fmtMoney(usd(openingFloat || '0.0000'))}.`)
    } catch (e) {
      fail(e)
    }
  }

  async function doCloseTill() {
    if (!tillId) return
    try {
      const report = await closeTill(tillId, usd(countedCash || '0.0000'))
      setZReport(report)
      setTillId(null)
      say('Till closed — Z-report below.')
    } catch (e) {
      fail(e)
    }
  }

  async function doSync() {
    try {
      const report = await syncRun()
      say(`Sync complete — ${report.applied} applied, ${report.duplicates} duplicates.`)
      await refreshStatus()
    } catch (e) {
      fail(e)
    }
  }

  async function doLogout() {
    await logout()
    setSession(null)
    setTillId(null)
    setLastSale(null)
    setLines([])
    setPreview(null)
    setNotice(null)
    setError(null)
  }

  function handleLoggedIn(next: AuthSession) {
    setSession(next)
    setSwitching(false)
    setNotice(null)
    setError(null)
  }

  // Phase 7: voiding needs PosVoid (Manager/Admin). Anyone else gets
  // the manager override prompt instead of a direct void. The prompt
  // is UI-side only — the backend still enforces the void permission.
  async function doVoidSale() {
    if (!lastSale) return
    if (canVoid(session?.user.role ?? '')) {
      try {
        await voidTransaction(lastSale.transaction_id)
        say(`Sale ${lastSale.doc_number} voided.`)
      } catch (e) {
        fail(e)
      }
      return
    }
    setOverrideRequest({
      reason: 'void',
      label: `Void sale ${lastSale.doc_number}. Your role (${session?.user.role ?? 'unknown'}) cannot void — a manager must verify with their PIN.`,
    })
  }

  // Phase 7: a line price entered below its unit cost is a price
  // override (below margin). Managers/admins apply it directly; anyone
  // else must pass the manager override prompt, and the line is not
  // added until the override verifies.
  function requestPriceOverride(entry: {
    sku: string
    description: string
    qty: string
    unit_price: string
    unit_cost: string
  }) {
    setPendingEntry(entry)
    setOverrideRequest({
      reason: 'price_override',
      label: `Price ${entry.unit_price} for ${entry.description} is below cost ${entry.unit_cost}. A manager must verify the override with their PIN.`,
    })
  }

  function applyOverride() {
    if (!overrideRequest) return
    if (overrideRequest.reason === 'void') {
      const sale = lastSale
      setOverrideRequest(null)
      if (sale) {
        void voidTransaction(sale.transaction_id)
          .then(() => say(`Sale ${sale.doc_number} voided (manager override).`))
          .catch(fail)
      }
      return
    }
    if (pendingEntry) {
      const known = findBySku(pendingEntry.sku)
      commitEntry(known, pendingEntry)
    }
    setPendingEntry(null)
    setOverrideRequest(null)
    say('Price override verified by manager.')
  }

  function cancelOverride() {
    setOverrideRequest(null)
    setPendingEntry(null)
  }

  async function doSwitchProvider(name: string) {
    try {
      const current = await setProvider(name)
      setProviderState(current)
      say(`Payment provider: ${current}.`)
    } catch (e) {
      fail(e)
    }
  }

  // Phase 7: login / operator-switch / override screens replace the
  // till UI (the till itself is untouched underneath).
  if (!session || switching) {
    return (
      <div className="app">
        <header>
          <h1>{storeName} — Counter POS</h1>
        </header>
        <main className="login-main">
          <Login
            title={switching ? 'Switch operator' : 'Sign in — Counter POS'}
            subtitle={
              switching
                ? 'The new operator signs in with email + password, or with their counter PIN / badge.'
                : 'Sign in with your email + password, or switch operator with a counter PIN / badge.'
            }
            allowPin
            onLoggedIn={handleLoggedIn}
            onCancel={switching ? () => setSwitching(false) : undefined}
          />
        </main>
        <footer>
          <p>Sales work offline against the local edge; sync when HQ is reachable.</p>
        </footer>
      </div>
    )
  }

  if (overrideRequest) {
    return (
      <div className="app">
        <header>
          <h1>{storeName} — Counter POS</h1>
          <div className="status">
            <span>
              {session.user.display_name ?? session.user.email} ({session.user.role})
            </span>
          </div>
        </header>
        <main className="login-main">
          <OverridePrompt
            label={overrideRequest.label}
            reasonCode={overrideRequest.reason}
            onVerified={applyOverride}
            onCancel={cancelOverride}
          />
        </main>
      </div>
    )
  }

  const money = (v: string, onChange: (v: string) => void, placeholder = '0.0000') => (
    <input value={v} onChange={(e) => onChange(e.target.value)} placeholder={placeholder} inputMode="decimal" />
  )

  return (
    <div className="app">
      <header>
        <h1>{storeName} — Counter POS</h1>
        <div className="status">
          <span>
            {session.user.display_name ?? session.user.email} ({session.user.role})
          </span>
          <span>Till: {tillId ? `open (${tillId.slice(0, 8)})` : 'closed'}</span>
          <span>Pending sync: {sync?.pending_count ?? '?'}</span>
          <span>Provider: {provider}</span>
          <span>Mode: {isTauri ? 'Tauri app' : 'dev (HTTP → df-server edge)'}</span>
          <button type="button" onClick={() => setSwitching(true)}>
            Switch operator
          </button>
          <button type="button" onClick={() => void doLogout()}>
            Log out
          </button>
        </div>
      </header>

      {error && <div className="error" role="alert">{error}</div>}
      {notice && <div className="notice">{notice}</div>}

      <main>
        <section className="checkout">
          <h2>Checkout</h2>
          <label className="scan">
            Scan / barcode
            <input
              ref={scanRef}
              value={scan}
              onChange={(e) => setScan(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void doScan()
              }}
              placeholder="Scan or type a barcode, Enter to add (2*barcode = qty 2)"
              autoFocus
            />
          </label>
          <div className="quick-pick">
            {DEMO_CATALOG.map((p) => (
              <button key={p.sku} type="button" onClick={() => addLine(p, '1')}>
                {p.description} — {fmtMoney(usd(p.unit_price))}
              </button>
            ))}
          </div>

          <div className="entry">
            <input value={entry.sku} onChange={(e) => setEntry({ ...entry, sku: e.target.value })} placeholder="SKU" />
            <input value={entry.description} onChange={(e) => setEntry({ ...entry, description: e.target.value })} placeholder="Description" />
            <input value={entry.qty} onChange={(e) => setEntry({ ...entry, qty: e.target.value })} placeholder="Qty" inputMode="decimal" />
            {money(entry.unit_price, (v) => setEntry({ ...entry, unit_price: v }), 'Unit price')}
            {money(entry.unit_cost, (v) => setEntry({ ...entry, unit_cost: v }), 'Unit cost')}
            <button type="button" onClick={doAddEntry}>Add line</button>
          </div>

          <table>
            <thead>
              <tr><th>Qty</th><th>Description</th><th>Unit</th><th>Line total</th><th /></tr>
            </thead>
            <tbody>
              {lines.map((l) => (
                <tr key={l.key}>
                  <td>
                    <input
                      value={l.qty}
                      onChange={(e) => setLines((prev) => prev.map((x) => (x.key === l.key ? { ...x, qty: e.target.value } : x)))}
                      inputMode="decimal"
                      aria-label={`Quantity for ${l.description}`}
                    />
                  </td>
                  <td>{l.description} <small>{l.sku}</small></td>
                  <td>{fmtMoney(usd(l.unit_price))}</td>
                  <td>{fmtMoney(usd(String(Number(l.qty || '0') * Number(l.unit_price || '0'))))}</td>
                  <td>
                    <button type="button" onClick={() => setLines((prev) => prev.filter((x) => x.key !== l.key))}>Remove</button>
                  </td>
                </tr>
              ))}
              {lines.length === 0 && (
                <tr><td colSpan={5}>No lines yet — scan, quick-pick, or enter a line.</td></tr>
              )}
            </tbody>
          </table>

          {preview && (
            <div className="totals">
              <div><span>Subtotal</span><strong>{fmtMoney(preview.subtotal)}</strong></div>
              <div><span>Eco fees</span><strong>{fmtMoney(preview.eco_fee_total)}</strong></div>
              <div><span>Tax</span><strong>{fmtMoney(preview.tax_total)}</strong></div>
              <div><span>POS fee</span><strong>{fmtMoney(preview.pos_fee_total)}</strong></div>
              <div className="grand"><span>Total</span><strong>{fmtMoney(preview.grand_total)}</strong></div>
              <p className="hint">Card tenders add a 3% surcharge at charge time; the charged total is authoritative.</p>
            </div>
          )}

          <div className="sale-meta">
            <label>
              Customer id (charge account)
              <input value={customerId} onChange={(e) => setCustomerId(e.target.value)} placeholder="UUID, required for charge" />
            </label>
            <label>
              PO number
              <input value={poNumber} onChange={(e) => setPoNumber(e.target.value)} placeholder="Optional" />
            </label>
          </div>

          <div className="tenders">
            <h3>Tenders (split tender: add rows; blank amount = remaining balance)</h3>
            {tenders.map((t) => (
              <div key={t.key} className="tender-row">
                <select value={t.kind} onChange={(e) => setTenders((prev) => prev.map((x) => (x.key === t.key ? { ...x, kind: e.target.value as TenderKind } : x)))}>
                  {(Object.keys(TENDER_LABELS) as TenderKind[]).map((k) => (
                    <option key={k} value={k}>{TENDER_LABELS[k]}</option>
                  ))}
                </select>
                <input value={t.amount} onChange={(e) => setTenders((prev) => prev.map((x) => (x.key === t.key ? { ...x, amount: e.target.value } : x)))} placeholder="Amount (blank = rest)" inputMode="decimal" />
                <button type="button" onClick={() => setTenders((prev) => prev.filter((x) => x.key !== t.key))}>Remove</button>
              </div>
            ))}
            <button type="button" onClick={() => setTenders((prev) => [...prev, { key: nextKey(), kind: 'Card', amount: '' }])}>Add tender</button>
          </div>

          <div className="actions">
            <button type="button" onClick={() => void doCheckout()} disabled={busy || lines.length === 0 || !tillId}>
              Charge (Ctrl+Enter)
            </button>
            <button type="button" onClick={() => void doParkHold()} disabled={lines.length === 0}>Park hold</button>
            <button type="button" onClick={() => void doQuote()} disabled={lines.length === 0}>Quote</button>
            <button type="button" onClick={() => setLines([])} disabled={lines.length === 0}>Clear cart</button>
          </div>
          {!tillId && <p className="hint">Open a till (below) before charging a sale.</p>}
        </section>

        <aside>
          {lastSale && (
            <section className="receipt">
              <h2>Receipt — {lastSale.doc_number}</h2>
              <div className="totals">
                <div><span>Subtotal</span><strong>{fmtMoney(lastSale.subtotal)}</strong></div>
                <div><span>Tax</span><strong>{fmtMoney(lastSale.tax_total)}</strong></div>
                <div><span>Eco fees</span><strong>{fmtMoney(lastSale.eco_fee_total)}</strong></div>
                <div><span>POS fee</span><strong>{fmtMoney(lastSale.pos_fee_total)}</strong></div>
                <div className="grand"><span>Total</span><strong>{fmtMoney(lastSale.grand_total)}</strong></div>
                {lastSale.payments.map((p) => (
                  <div key={p.reference}><span>{TENDER_LABELS[p.tender] ?? p.tender}</span><strong>{fmtMoney(p.amount)} <small>{p.reference}</small></strong></div>
                ))}
                {lastSale.change_due.amount !== '0.0000' && (
                  <div><span>Change due</span><strong>{fmtMoney(lastSale.change_due)}</strong></div>
                )}
                <p className="hint">Printed: {lastSale.receipt_printed ? 'yes' : 'no printer attached'}</p>
              </div>
              {receiptText && <pre>{receiptText}</pre>}
              <button type="button" onClick={() => void doVoidSale()}>
                Void this sale
              </button>
            </section>
          )}

          <section>
            <h2>Holds, quotes &amp; layaway</h2>
            <div className="holds">
              <button type="button" onClick={() => void listHolds().then((r) => setHolds(r.hold_ids)).catch(fail)}>Refresh holds</button>
              {holds.length === 0 && <p className="hint">No parked holds.</p>}
              {holds.map((h) => (
                <div key={h}>
                  <span>{h.slice(0, 8)}</span>
                  <button type="button" onClick={() => void doResumeHold(h)}>Resume</button>
                </div>
              ))}
            </div>
            <div className="layaway">
              <label>
                Layaway deposit
                {money(layawayDeposit, setLayawayDeposit, 'Deposit amount')}
              </label>
              <select value={layawayKind} onChange={(e) => setLayawayKind(e.target.value as TenderKind)}>
                {(Object.keys(TENDER_LABELS) as TenderKind[]).map((k) => (
                  <option key={k} value={k}>{TENDER_LABELS[k]}</option>
                ))}
              </select>
              <button type="button" onClick={() => void doLayaway()} disabled={lines.length === 0 || !layawayDeposit}>
                Create layaway
              </button>
            </div>
          </section>

          <section>
            <h2>Return</h2>
            <label>
              Original transaction id
              <input value={returnTxId} onChange={(e) => setReturnTxId(e.target.value)} placeholder="UUID of the sale" />
            </label>
            <label>
              Product
              <select value={returnSku} onChange={(e) => setReturnSku(e.target.value)}>
                {DEMO_CATALOG.map((p) => (
                  <option key={p.sku} value={p.sku}>{p.description}</option>
                ))}
              </select>
            </label>
            <label>
              Qty
              <input value={returnQty} onChange={(e) => setReturnQty(e.target.value)} inputMode="decimal" />
            </label>
            <button type="button" onClick={() => void doReturn()} disabled={!returnTxId.trim()}>Record return</button>
          </section>

          <section>
            <h2>Till &amp; sync</h2>
            {!tillId ? (
              <div>
                <label>
                  Opening float
                  {money(openingFloat, setOpeningFloat)}
                </label>
                <button type="button" onClick={() => void doOpenTill()}>Open till</button>
              </div>
            ) : (
              <div>
                <label>
                  Counted cash
                  {money(countedCash, setCountedCash)}
                </label>
                <button type="button" onClick={() => void doCloseTill()}>Close till (Z-report)</button>
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
            <div className="sync">
              <span>Pending outbox: {sync?.pending_count ?? '?'}</span>
              <button type="button" onClick={() => void doSync()}>Sync now</button>
            </div>
            <label>
              Payment provider
              <select
                value={providerSwitchable ? provider : 'mock'}
                disabled={!providerSwitchable}
                onChange={(e) => void doSwitchProvider(e.target.value)}
              >
                <option value="mock">Mock terminal</option>
                <option value="standalone">Standalone terminal</option>
              </select>
            </label>
            {!providerSwitchable && (
              <p className="hint">Provider switching needs the native Tauri app; the dev server uses its mock terminal.</p>
            )}
          </section>
        </aside>
      </main>

      <footer>
        <p>Shortcuts: <kbd>/</kbd> focus scanner · <kbd>Ctrl+Enter</kbd> charge · <kbd>Esc</kbd> clears the scan box. Sales work offline against the local edge; sync when HQ is reachable.</p>
      </footer>
    </div>
  )
}
