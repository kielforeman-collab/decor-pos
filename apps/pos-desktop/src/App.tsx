import { useCallback, useEffect, useRef, useState } from 'react'
import {
  checkout,
  closeTill,
  getCustomerSummary,
  getInvoice,
  getPriceLevels,
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
  searchProducts,
  setProvider,
  syncRun,
  syncState,
  voidTransaction,
} from './api'
import {
  canPriceOverride,
  canVoid,
  getSession,
  logout,
  type AuthSession,
} from './auth'
import Login from './Login'
import OverridePrompt from './OverridePrompt'
import HistoryModal from './components/HistoryModal'
import InvoiceView from './components/InvoiceView'
import PaymentModal from './components/PaymentModal'
import ProductSearch from './components/ProductSearch'
import Sidebar from './components/Sidebar'
import Ticket from './components/Ticket'
import TillPanel, { type FulfilmentMode } from './components/TillPanel'
import TintModal from './components/TintModal'
import TopBar from './components/TopBar'
import { useTheme } from './themes'
import { parseScannerInput } from './scan'
import type {
  ArCustomerSummary,
  CartLine,
  CompletedSale,
  CustomerSearchResult,
  InvoiceData,
  Money,
  PriceLevel,
  PricePreview,
  ProductSearchResult,
  SaleInput,
  SaleLineInput,
  SyncState,
  TenderInput,
  TenderKind,
  TenderRow,
  TintInfo,
  ZReport,
} from './types'
import {
  TENDER_LABELS,
  addDecimals,
  discountedPrice,
  fmtMoney,
  mulDecimals,
  sizeLabelForVolume,
  cad,
} from './types'

/**
 * Quick-pick chips — REAL SKUs + REAL list prices, verified against
 * price book 10005510 (EXCEL sheet, effective July 17, 2026).
 * NOTE: the chip prices were originally the MSRP column ($97.99 etc.);
 * the List Price column is what is used here, per the brief.
 * NET is the dealer's cost and backs the below-cost gate.
 */
const RETAIL_LEVEL: PriceLevel = { id: 'retail', code: 'retail', name: 'Retail', discount_pct: '0' }
const DELIVERY_FEE = '25.00'
const SALE_NUMBER_FALLBACK = 'NEW SALE'

let keySeq = 0
const nextKey = () => `k${++keySeq}`

interface LineSnapshot {
  product_id: string
  sku: string
  description: string
  qty: string
  uom: string
  unit_price: string
  unit_cost: string
  discPct: string
  sizeLabel?: string
  containerVolumeMl?: number | null
  tint?: TintInfo | null
}

function syntheticProductId(sku: string): string {
  return `00000000-0000-7000-8000-${sku.slice(0, 12).padEnd(12, '0')}`
}

function cartToSaleInput(
  lines: CartLine[],
  tenders: TenderRow[],
  customerId: string,
  poNumber: string,
  opts: { priceLevelId?: string | null; jobName?: string | null },
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
        // Per-line discount % has no field in SaleInput, so the
        // discounted unit price is sent (exact decimal math); the
        // server stays authoritative for tax, eco fees and totals.
        unit_price: cad(discountedPrice(l.unit_price, l.discPct)),
        unit_cost: cad(l.unit_cost),
      }),
    ),
    tenders: tenders.map(
      (t): TenderInput => ({
        kind: t.kind,
        amount: t.amount.trim() === '' ? null : cad(t.amount),
      }),
    ),
    po_number: poNumber.trim() || null,
    price_level_id: opts.priceLevelId ?? null,
    job_name: opts.jobName?.trim() || null,
  }
}

/** Fallback invoice built from the just-completed sale when the
 * invoice endpoint is not implemented yet. GST/PST split is unknown
 * here, so the single tax total is shown as GST and PST $0.00 —
 * the server invoice endpoint is the authoritative source. */
function fallbackInvoice(
  sale: CompletedSale,
  lines: CartLine[],
  tenders: TenderRow[],
  customer: CustomerSearchResult | null,
  summary: ArCustomerSummary | null,
  fulfilment: FulfilmentMode,
  deliveryFee: string,
  saleNote: string,
  clerk: string,
  jobName: string,
): InvoiceData {
  const zero = cad('0.0000')
  const discounts = lines.reduce((acc, l) => {
    const perUnit = addDecimals(l.unit_price, `-${discountedPrice(l.unit_price, l.discPct)}`)
    return addDecimals(acc, mulDecimals(perUnit, l.qty || '0'))
  }, '0.0000')
  const chargeTender = tenders.find((t) => t.kind === 'ChargeAccount')
  const charge =
    chargeTender && summary
      ? {
          previous_balance: summary.balance,
          charged: cad(chargeTender.amount || '0.0000'),
          new_balance: cad(addDecimals(summary.balance.amount, chargeTender.amount || '0.0000')),
          credit_limit: summary.credit_limit,
          available_credit: cad(
            addDecimals(
              summary.credit_limit.amount,
              `-${addDecimals(summary.balance.amount, chargeTender.amount || '0.0000')}`,
            ),
          ),
        }
      : null
  const cashTender = tenders.find((t) => t.kind === 'Cash')
  return {
    store_name: 'NORTH SHORE DECORATING CENTRE',
    doc_number: sale.doc_number,
    created_at: new Date().toISOString(),
    customer: customer
      ? {
          name:
            customer.company_name ||
            [customer.first_name, customer.last_name].filter(Boolean).join(' ') ||
            customer.account_number,
          account_number: customer.account_number,
          email: customer.email,
        }
      : null,
    job_name: jobName.trim() || null,
    po_number: null,
    fulfilment_mode:
      fulfilment === 'delivery' ? 'Delivery' : fulfilment === 'will_call' ? 'Will call' : 'Take now',
    clerk,
    terms: customer?.payment_terms ?? null,
    note: saleNote.trim() || null,
    lines: lines.map((l) => ({
      sku: l.sku,
      description: l.description,
      qty: l.qty,
      uom: l.uom,
      unit_price: cad(l.unit_price),
      disc_pct: l.discPct,
      amount: cad(mulDecimals(discountedPrice(l.unit_price, l.discPct), l.qty || '0')),
      size_label: l.sizeLabel ?? null,
      tint: l.tint
        ? `TINT: ${l.tint.colorName} ${l.tint.colorCode} · ${l.tint.base} · Formula: ${l.tint.formula.map((f) => `${f.code} ${f.oz} oz`).join(', ')}`
        : null,
    })),
    tenders: tenders.map((t) => ({
      kind: t.kind,
      amount: cad(t.amount || '0.0000'),
      tendered: t.tendered ? cad(t.tendered) : null,
      change: t.kind === 'Cash' && t.tendered ? cad(sale.change_due.amount) : null,
    })),
    totals: {
      subtotal: sale.subtotal,
      discounts: cad(discounts),
      eco_fee_total: sale.eco_fee_total,
      delivery: cad(fulfilment === 'delivery' ? deliveryFee : '0.0000'),
      gst: sale.tax_total,
      pst: zero,
      grand_total: sale.grand_total,
      change_due: sale.change_due,
      charge,
    },
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
  const [pendingEntry, setPendingEntry] = useState<LineSnapshot | null>(null)
  const [storeName, setStoreName] = useState('Decor POS')
  const [provider, setProviderState] = useState('mock')
  const [sync, setSync] = useState<SyncState | null>(null)
  const [tillId, setTillId] = useState<string | null>(null)
  const [openingFloat, setOpeningFloat] = useState('150.0000')
  const [countedCash, setCountedCash] = useState('')
  const [zReport, setZReport] = useState<ZReport | null>(null)

  const [lines, setLines] = useState<CartLine[]>([])
  const [preview, setPreview] = useState<PricePreview | null>(null)
  const [customer, setCustomer] = useState<CustomerSearchResult | null>(null)
  const [customerSummary, setCustomerSummary] = useState<ArCustomerSummary | null>(null)
  const [poNumber, setPoNumber] = useState('')
  const [jobName, setJobName] = useState('')
  const [fulfilment, setFulfilment] = useState<FulfilmentMode>('take_now')
  const [deliveryFee, setDeliveryFee] = useState('0.00')
  const [saleNote, setSaleNote] = useState('')
  const [internalNote, setInternalNote] = useState('')
  const [priceLevels, setPriceLevels] = useState<PriceLevel[]>([RETAIL_LEVEL])
  const [priceLevelId, setPriceLevelId] = useState('retail')
  const [emailInvoices, setEmailInvoices] = useState(false)

  const [entry, setEntry] = useState({ sku: '', description: '', qty: '1', unit_price: '', unit_cost: '' })
  const [searchText, setSearchText] = useState('')
  const [entryQty, setEntryQty] = useState('1')
  const [lastSale, setLastSale] = useState<CompletedSale | null>(null)
  const [completedSales, setCompletedSales] = useState<CompletedSale[]>([])
  const [voidedSales, setVoidedSales] = useState<CompletedSale[]>([])
  const [receiptText, setReceiptText] = useState<string | null>(null)
  const [holds, setHolds] = useState<string[]>([])
  const [notice, setNotice] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const [txnTab, setTxnTab] = useState<'sale' | 'held' | 'completed' | 'voided'>('sale')
  const [activeNav, setActiveNav] = useState('Transactions')
  const [paymentOpen, setPaymentOpen] = useState(false)
  const [historyOpen, setHistoryOpen] = useState(false)
  const [invoice, setInvoice] = useState<{ data: InvoiceData; reprint: boolean } | null>(null)
  const [tintTarget, setTintTarget] = useState<CartLine | null>(null)

  const [returnTxId, setReturnTxId] = useState('')
  const [returnSku, setReturnSku] = useState('')
  const [returnQty, setReturnQty] = useState('1')
  const [layawayDeposit, setLayawayDeposit] = useState('')
  const [layawayKind, setLayawayKind] = useState<TenderKind>('Cash')

  const theme = useTheme()
  const searchRef = useRef<HTMLInputElement>(null)

  const clerkName = session?.user.display_name ?? session?.user.email ?? 'Counter staff'
  const customerId = customer?.id ?? ''

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
    void getPriceLevels().then((levels) => {
      if (levels.length > 0) {
        setPriceLevels(levels)
        if (!levels.some((l) => l.id === priceLevelId)) {
          const retail = levels.find((l) => l.code === 'retail') ?? levels[0]
          setPriceLevelId(retail.id)
        }
      }
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
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
  // The preview reflects the customer, price level and job, so a price
  // level change re-prices the ticket server-side.
  useEffect(() => {
    if (!session) return
    if (lines.length === 0) {
      setPreview(null)
      return
    }
    const handle = setTimeout(() => {
      const input = cartToSaleInput(lines, [], customerId, poNumber, { priceLevelId, jobName })
      pricePreview(input)
        .then(setPreview)
        .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
    }, 250)
    return () => clearTimeout(handle)
  }, [lines, session, customerId, poNumber, priceLevelId, jobName])

  const say = (msg: string) => {
    setNotice(msg)
    setError(null)
  }
  const fail = (e: unknown) => {
    setError(e instanceof Error ? e.message : String(e))
    setNotice(null)
  }

  function saleInput(tenders: TenderRow[]): SaleInput {
    return cartToSaleInput(lines, tenders, customerId, poNumber, { priceLevelId, jobName })
  }

  function addSnapshot(snapshot: LineSnapshot) {
    setLines((prev) => {
      const existing = prev.find(
        (l) =>
          l.product_id === snapshot.product_id &&
          l.discPct === snapshot.discPct &&
          JSON.stringify(l.tint ?? null) === JSON.stringify(snapshot.tint ?? null),
      )
      if (existing) {
        return prev.map((l) =>
          l.key === existing.key ? { ...l, qty: String(Number(l.qty) + Number(snapshot.qty)) } : l,
        )
      }
      return [...prev, { key: nextKey(), ...snapshot }]
    })
  }

  /** Below-cost gate, preserved: a line priced below its unit cost
   * needs a manager override unless the signer already holds it. */
  function maybeAddSnapshot(snapshot: LineSnapshot) {
    if (
      Number(snapshot.unit_cost) > 0 &&
      Number(snapshot.unit_price) < Number(snapshot.unit_cost) &&
      !canPriceOverride(session?.user.role ?? '')
    ) {
      setPendingEntry(snapshot)
      setOverrideRequest({
        reason: 'price_override',
        label: `Price ${snapshot.unit_price} for ${snapshot.description} is below cost ${snapshot.unit_cost}. A manager must verify the override with their PIN.`,
      })
      return
    }
    addSnapshot(snapshot)
  }

  function snapshotFromProduct(p: ProductSearchResult, qty: string): LineSnapshot {
    return {
      product_id: p.product_id,
      sku: p.sku,
      description: p.name,
      qty,
      uom: p.base_uom,
      unit_price: p.list_price,
      unit_cost: p.standard_cost,
      discPct: '0',
      sizeLabel: sizeLabelForVolume(p.container_volume_ml, p.base_uom),
      containerVolumeMl: p.container_volume_ml,
      tint: null,
    }
  }

  function addSearchProduct(p: ProductSearchResult, qty = '1') {
    maybeAddSnapshot(snapshotFromProduct(p, qty))
  }

  async function resolveScanText(raw: string) {
    const parsed = parseScannerInput(raw)
    if (!parsed) return
    try {
      const result = await scanBarcode(raw)
      const code = result.barcode
      let found = await searchProducts({ barcode: code, limit: 1 })
      if (found.length === 0) found = await searchProducts({ sku: code, limit: 1 })
      if (found.length === 0) throw new Error(`Unknown barcode / SKU: ${code}`)
      addSearchProduct(found[0], String(parsed.qty))
    } catch (e) {
      fail(e)
    }
    searchRef.current?.focus()
  }

  function doAddEntry() {
    const sku = entry.sku.trim()
    if (!sku || !entry.description.trim() || !entry.unit_price) {
      setError('SKU, description and unit price are required to add a line.')
      return
    }
    const snapshot: LineSnapshot = {
      product_id: syntheticProductId(sku),
      sku,
      description: entry.description.trim(),
      qty: entry.qty || '1',
      uom: 'ea',
      unit_price: entry.unit_price,
      unit_cost: entry.unit_cost || '0.0000',
      discPct: '0',
      tint: null,
    }
    maybeAddSnapshot(snapshot)
    setEntry({ sku: '', description: '', qty: '1', unit_price: '', unit_cost: '' })
  }

  async function doCheckout(tenders: TenderRow[]) {
    if (lines.length === 0) return
    if (!tillId) {
      setError('Open a till before charging a sale.')
      return
    }
    setBusy(true)
    const linesSnapshot = [...lines]
    const tendersSnapshot = [...tenders]
    try {
      const sale = await checkout(saleInput(tenders))
      setLastSale(sale)
      setCompletedSales((prev) => [sale, ...prev])
      setLines([])
      const text = await lastReceiptText()
      setReceiptText(text)
      say(`Sale ${sale.doc_number} complete — ${fmtMoney(sale.grand_total)}${sale.change_due.amount !== '0.0000' ? `, change due ${fmtMoney(sale.change_due)}` : ''}`)
      setPaymentOpen(false)
      // Invoice: server endpoint first, local fallback second.
      let inv: InvoiceData | null = null
      try {
        inv = await getInvoice(sale.transaction_id)
      } catch {
        inv = null
      }
      setInvoice({
        data:
          inv ??
          fallbackInvoice(
            sale,
            linesSnapshot,
            tendersSnapshot,
            customer,
            customerSummary,
            fulfilment,
            deliveryFee,
            saleNote,
            clerkName,
            jobName,
          ),
        reprint: false,
      })
      setSaleNote('')
      setInternalNote('')
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
      const { hold_id } = await parkHold(saleInput([]))
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
          discPct: '0',
          tint: null,
        })),
      )
      if (input.customer_id) {
        // Customer restore needs the customer record; the hold only
        // carries the id, so re-select is a manual step (noted in UI).
        setCustomerIdOnly(input.customer_id)
      }
      if (input.price_level_id) setPriceLevelId(input.price_level_id)
      if (input.job_name) setJobName(input.job_name)
      setHolds((prev) => prev.filter((h) => h !== holdId))
      setTxnTab('sale')
      say('Hold resumed.')
    } catch (e) {
      fail(e)
    }
  }

  // Holds carry only the customer id, not the record — keep the id so
  // the sale still charges to the right account; the card re-loads
  // when the clerk re-selects the customer.
  function setCustomerIdOnly(id: string) {
    setCustomer((prev) =>
      prev ?? {
        id,
        account_number: id.slice(0, 8),
        customer_type: '',
        company_name: null,
        first_name: null,
        last_name: null,
        email: null,
        phone: null,
        price_level_id: null,
        charge_enabled: false,
        credit_limit: cad('0.0000'),
        payment_terms: null,
        credit_hold: false,
      },
    )
  }

  async function doQuote() {
    if (lines.length === 0) return
    try {
      const { transaction_id } = await recordQuote(saleInput([]))
      say(`Quote recorded (${transaction_id.slice(0, 8)}). No inventory moved.`)
    } catch (e) {
      fail(e)
    }
  }

  async function doLayaway() {
    if (lines.length === 0 || !layawayDeposit) return
    try {
      const sale = saleInput([])
      const { transaction_id } = await recordLayaway(sale, {
        kind: layawayKind,
        amount: cad(layawayDeposit),
      })
      const grand = preview?.grand_total.amount ?? '?'
      say(
        `Layaway ${transaction_id.slice(0, 8)} recorded — deposit ${fmtMoney(cad(layawayDeposit))} of ${fmtMoney(cad(grand))}.`,
      )
      setLines([])
    } catch (e) {
      fail(e)
    }
  }

  async function doReturn() {
    if (!returnTxId.trim() || !returnSku.trim()) return
    try {
      const found = await searchProducts({ sku: returnSku.trim(), limit: 1 })
      if (found.length === 0) throw new Error(`Unknown SKU: ${returnSku.trim()}`)
      const ret = await recordReturn(returnTxId.trim(), [
        { product_id: found[0].product_id, qty: returnQty },
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
      const cashierId = session?.user.id ?? '00000000-0000-7000-8000-000000000099'
      const till = await openTill(cashierId, cad(openingFloat || '0.0000'))
      setTillId(till.id)
      say(`Till open — float ${fmtMoney(cad(openingFloat || '0.0000'))}.`)
    } catch (e) {
      fail(e)
    }
  }

  async function doCloseTill() {
    if (!tillId) return
    try {
      const report = await closeTill(tillId, cad(countedCash || '0.0000'))
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

  async function doVoidSale() {
    const sale = lastSale ?? completedSales[0]
    if (!sale) return
    if (canVoid(session?.user.role ?? '')) {
      try {
        await voidTransaction(sale.transaction_id)
        setVoidedSales((prev) => [sale, ...prev])
        setCompletedSales((prev) => prev.filter((s) => s.transaction_id !== sale.transaction_id))
        if (lastSale?.transaction_id === sale.transaction_id) setLastSale(null)
        if (invoice?.data.doc_number === sale.doc_number) setInvoice(null)
        say(`Sale ${sale.doc_number} voided.`)
      } catch (e) {
        fail(e)
      }
      return
    }
    setOverrideRequest({
      reason: 'void',
      label: `Void sale ${sale.doc_number}. Your role (${session?.user.role ?? 'unknown'}) cannot void — a manager must verify with their PIN.`,
    })
  }

  function applyOverride() {
    if (!overrideRequest) return
    if (overrideRequest.reason === 'void') {
      const sale = lastSale ?? completedSales[0]
      setOverrideRequest(null)
      if (sale) {
        void voidTransaction(sale.transaction_id)
          .then(() => {
            setVoidedSales((prev) => [sale, ...prev])
            setCompletedSales((prev) => prev.filter((s) => s.transaction_id !== sale.transaction_id))
            if (lastSale?.transaction_id === sale.transaction_id) setLastSale(null)
            say(`Sale ${sale.doc_number} voided (manager override).`)
          })
          .catch(fail)
      }
      return
    }
    if (pendingEntry) {
      addSnapshot(pendingEntry)
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

  function selectCustomer(c: CustomerSearchResult | null) {
    setCustomer(c)
    setCustomerSummary(null)
    if (!c) {
      setPriceLevelId(priceLevels.find((l) => l.code === 'retail')?.id ?? 'retail')
      return
    }
    if (c.price_level_id && priceLevels.some((l) => l.id === c.price_level_id)) {
      setPriceLevelId(c.price_level_id)
    }
    void getCustomerSummary(c.id)
      .then((s) => {
        if (s) setCustomerSummary(s)
      })
      .catch(() => {
        // Summary endpoint unavailable — the card shows the search record.
      })
  }

  function selectFulfilment(mode: FulfilmentMode) {
    setFulfilment(mode)
    setDeliveryFee(mode === 'delivery' ? DELIVERY_FEE : '0.00')
  }

  function openPayment() {
    if (lines.length === 0) return
    if (!tillId) {
      setError('Open a till before charging a sale.')
      return
    }
    setPaymentOpen(true)
  }

  function newSale() {
    setLines([])
    setSaleNote('')
    setInternalNote('')
    setInvoice(null)
    setTxnTab('sale')
    searchRef.current?.focus()
  }

  // Keyboard-first: F2 customer, F3 lookup, F4 tint, F5 discount,
  // F7 hold, F9/F12 tender/complete, Esc closes modals, Ctrl+Enter
  // opens the payment modal, `/` focuses the entry search.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement
      const inField = target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.tagName === 'SELECT')
      if (e.key === 'Escape') {
        if (tintTarget) setTintTarget(null)
        else if (historyOpen) setHistoryOpen(false)
        else if (invoice) setInvoice(null)
        else if (paymentOpen) setPaymentOpen(false)
        return
      }
      if (e.key === 'F2') {
        e.preventDefault()
        document.getElementById('customer-search')?.focus()
      } else if (e.key === 'F3') {
        e.preventDefault()
        searchRef.current?.focus()
      } else if (e.key === 'F4') {
        e.preventDefault()
        const first = lines.find((l) => l.containerVolumeMl != null)
        if (first) setTintTarget(first)
        else if (lines.length > 0) setTintTarget(lines[0])
      } else if (e.key === 'F5') {
        e.preventDefault()
        document.querySelector<HTMLElement>('.disc-input')?.focus()
      } else if (e.key === 'F7') {
        e.preventDefault()
        void doParkHold()
      } else if (e.key === 'F9' || e.key === 'F12') {
        e.preventDefault()
        if (!paymentOpen) openPayment()
      } else if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
        e.preventDefault()
        if (!paymentOpen) openPayment()
      } else if (e.key === '/' && !inField) {
        e.preventDefault()
        searchRef.current?.focus()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [lines, paymentOpen, historyOpen, invoice, tintTarget, tillId])

  // Phase 7: login / operator-switch / override screens replace the
  // till UI (the till itself is untouched underneath).
  if (!session || switching) {
    return (
      <div className="app app-login">
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
      <div className="app app-login">
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

  // Grand due = server grand total + the flat delivery fee (delivery
  // only). The delivery fee has no field in the SaleInput contract,
  // so it is a display add-on here; the server invoice endpoint is
  // authoritative for the printed invoice.
  const grandDue = preview ? addDecimals(preview.grand_total.amount, fulfilment === 'delivery' ? deliveryFee || '0.00' : '0.00') : '0.0000'
  const discountsTotal = lines.reduce((acc, l) => {
    const perUnit = addDecimals(l.unit_price, `-${discountedPrice(l.unit_price, l.discPct)}`)
    return addDecimals(acc, mulDecimals(perUnit, l.qty || '0'))
  }, '0.0000')
  const paintItemCount = lines.filter((l) => l.containerVolumeMl != null).length
  const saleLabel = lastSale?.doc_number ?? SALE_NUMBER_FALLBACK

  return (
    <div className="app">
      <TopBar
        storeName={storeName}
        tillOpen={!!tillId}
        openingFloat={openingFloat}
        clerkName={clerkName}
        onOmniAdd={(p) => addSearchProduct(p, '1')}
        onToast={say}
        themes={theme.allThemes}
        themeId={theme.themeId}
        themeNote={theme.themeDef.note}
        liveMonitors={theme.liveMonitors}
        onSetTheme={theme.setTheme}
        onToggleLiveMonitors={theme.setLiveMonitors}
        onAddCustomTheme={theme.addCustomTheme}
      />
      <Sidebar
        activeNav={activeNav}
        tillOpen={!!tillId}
        currentThemeLabel={theme.themeDef.label}
        onNavigate={(name) => {
          if (name === 'Transactions') {
            setActiveNav(name)
          } else {
            say(`${name} — not wired up in this build yet.`)
          }
        }}
        onCycleTheme={theme.cycleTheme}
        onChangeLocation={() => say('Location switching is not wired in this build — single store (Main counter).')}
        onSwitchOperator={() => setSwitching(true)}
        onSignOut={() => void doLogout()}
      />

      <main className="main">
        <section className="register">
          {error && (
            <div className="error" role="alert">
              <span>{error}</span>
              <button type="button" className="banner-x" aria-label="Dismiss error" onClick={() => setError(null)}>✕</button>
            </div>
          )}
          {notice && (
            <div className="notice">
              <span>{notice}</span>
              <button type="button" className="banner-x" aria-label="Dismiss message" onClick={() => setNotice(null)}>✕</button>
            </div>
          )}

          <div className="txn-tabs" role="tablist" aria-label="Transactions">
            {(
              [
                ['sale', 'Sale', lines.length],
                ['held', 'Held', holds.length],
                ['completed', 'Completed', completedSales.length],
                ['voided', 'Voided', voidedSales.length],
              ] as const
            ).map(([tab, label, count]) => (
              <button
                key={tab}
                type="button"
                role="tab"
                aria-selected={txnTab === tab}
                className={`txn-tab txn-${tab}${txnTab === tab ? ' active' : ''}`}
                onClick={() => setTxnTab(tab)}
              >
                {label} <span className="txn-count">{count}</span>
              </button>
            ))}
            {txnTab === 'sale' && (
              <span className="txn-fields">
                <label className="txn-field">
                  Job
                  <input className="job-input" value={jobName} onChange={(e) => setJobName(e.target.value)} placeholder="Job name (optional)" aria-label="Job name" />
                </label>
                <label className="txn-field">
                  PO
                  <input className="po-input" value={poNumber} onChange={(e) => setPoNumber(e.target.value)} placeholder="Optional" aria-label="PO number" />
                </label>
                <span className="account-chip">
                  {customerSummary
                    ? `Balance ${fmtMoney(customerSummary.balance)} · Available ${fmtMoney(cad((Number(customerSummary.credit_limit.amount) - Number(customerSummary.balance.amount)).toFixed(4)))} · ${customer?.payment_terms ?? 'Terms —'}`
                    : customer
                      ? `${customer.account_number} · select the customer card for balance`
                      : 'Walk-in — no account'}
                </span>
              </span>
            )}
          </div>

          {txnTab === 'sale' && (
            <>
              <div className="fulfilment seg" role="group" aria-label="Fulfilment">
                {(
                  [
                    ['take_now', 'Take now'],
                    ['will_call', 'Will call'],
                    ['delivery', 'Delivery'],
                  ] as const
                ).map(([mode, label]) => (
                  <button
                    key={mode}
                    type="button"
                    aria-pressed={fulfilment === mode}
                    className={fulfilment === mode ? 'active' : ''}
                    onClick={() => selectFulfilment(mode)}
                  >
                    {label}
                  </button>
                ))}
                <span className="seg-note">{fulfilment === 'delivery' ? 'Delivery fee $25.00' : 'No delivery fee'}</span>
              </div>

              <div className="entry-row elbow-frame">
                <ProductSearch
                  inputRef={searchRef}
                  placeholder="SKU, UPC, or product name"
                  onPick={(p) => {
                    setSearchText('')
                    addSearchProduct(p, entryQty)
                  }}
                  onManualText={setSearchText}
                  onEnterNoResult={(text) => void resolveScanText(text)}
                  autoFocus
                />
                <label className="qty-label">
                  Qty
                  <input
                    value={entryQty}
                    onChange={(e) => setEntryQty(e.target.value)}
                    inputMode="decimal"
                    aria-label="Quantity for next added line"
                  />
                </label>
                <button
                  type="button"
                  className="add-btn"
                  onClick={() => {
                    if (searchText.trim()) void resolveScanText(searchText.trim())
                    else doAddEntry()
                  }}
                >
                  Add
                </button>
              </div>

              <Ticket
                lines={lines}
                preview={preview}
                onQty={(key, qty) => setLines((prev) => prev.map((x) => (x.key === key ? { ...x, qty } : x)))}
                onDisc={(key, pct) => setLines((prev) => prev.map((x) => (x.key === key ? { ...x, discPct: pct } : x)))}
                onRemove={(key) => setLines((prev) => prev.filter((x) => x.key !== key))}
                onTint={(line) => setTintTarget(line)}
                onClearTint={(key) =>
                  setLines((prev) => prev.map((x) => (x.key === key ? { ...x, tint: null } : x)))
                }
              />

              <details className="manual-entry">
                <summary>Manual entry</summary>
                <div className="entry">
                  <input value={entry.sku} onChange={(e) => setEntry({ ...entry, sku: e.target.value })} placeholder="SKU" />
                  <input value={entry.description} onChange={(e) => setEntry({ ...entry, description: e.target.value })} placeholder="Description" />
                  <input value={entry.qty} onChange={(e) => setEntry({ ...entry, qty: e.target.value })} placeholder="Qty" inputMode="decimal" />
                  {money(entry.unit_price, (v) => setEntry({ ...entry, unit_price: v }), 'Unit price')}
                  {money(entry.unit_cost, (v) => setEntry({ ...entry, unit_cost: v }), 'Unit cost')}
                  <button type="button" onClick={doAddEntry}>Add line</button>
                </div>
              </details>

              <div className="notes">
                <label>
                  Sale note (prints on receipt)
                  <input value={saleNote} onChange={(e) => setSaleNote(e.target.value)} placeholder="e.g. Leave at back door" />
                </label>
                <label>
                  Internal note (never printed)
                  <input value={internalNote} onChange={(e) => setInternalNote(e.target.value)} placeholder="Staff only" />
                </label>
              </div>

              {preview && (
                <div className="totals preview-strip">
                  <div><span>Subtotal</span><strong>{fmtMoney(preview.subtotal)}</strong></div>
                  <div><span>Eco fees</span><strong>{fmtMoney(preview.eco_fee_total)}</strong></div>
                  <div><span>Tax</span><strong>{fmtMoney(preview.tax_total)}</strong></div>
                  <div><span>POS fee</span><strong>{fmtMoney(preview.pos_fee_total)}</strong></div>
                  <div className="grand"><span>Total</span><strong>{fmtMoney(preview.grand_total)}</strong></div>
                  <p className="hint">Card tenders add a 3% surcharge at charge time; the charged total is authoritative.</p>
                </div>
              )}
              {!tillId && lines.length > 0 && <p className="hint">Open a till (in the Till panel) before charging a sale.</p>}
            </>
          )}

          {txnTab === 'held' && (
            <div className="tab-list">
              <h2>Held sales</h2>
              {holds.length === 0 && <p className="hint">No parked holds.</p>}
              <button type="button" onClick={() => void listHolds().then((r) => setHolds(r.hold_ids)).catch(fail)}>Refresh holds</button>
              {holds.map((h) => (
                <div key={h} className="tab-row">
                  <span>Hold {h.slice(0, 8)}</span>
                  <button type="button" onClick={() => void doResumeHold(h)}>Resume</button>
                </div>
              ))}
            </div>
          )}

          {txnTab === 'completed' && (
            <div className="tab-list">
              <h2>Completed sales</h2>
              {completedSales.length === 0 && <p className="hint">No completed sales this session.</p>}
              {completedSales.map((s) => (
                <div key={s.transaction_id} className="tab-row">
                  <span>{s.doc_number}</span>
                  <span>{fmtMoney(s.grand_total)}</span>
                  <button
                    type="button"
                    onClick={() => {
                      void getInvoice(s.transaction_id).then((inv) => {
                        if (inv) setInvoice({ data: inv, reprint: true })
                        else {
                          setInvoice({
                            data: fallbackInvoice(s, [], [], customer, customerSummary, fulfilment, deliveryFee, saleNote, clerkName, jobName),
                            reprint: true,
                          })
                        }
                      }).catch(fail)
                    }}
                  >
                    Reprint invoice
                  </button>
                </div>
              ))}
              {lastSale && (
                <div className="receipt-mini">
                  <h3>Receipt — {lastSale.doc_number}</h3>
                  {receiptText && <pre>{receiptText}</pre>}
                  <button type="button" onClick={() => void doVoidSale()}>Void this sale</button>
                </div>
              )}
            </div>
          )}

          {txnTab === 'voided' && (
            <div className="tab-list">
              <h2>Voided sales</h2>
              {voidedSales.length === 0 && <p className="hint">No voided sales this session.</p>}
              {voidedSales.map((s) => (
                <div key={s.transaction_id} className="tab-row">
                  <span>{s.doc_number}</span>
                  <span>{fmtMoney(s.grand_total)}</span>
                </div>
              ))}
            </div>
          )}
        </section>

        <aside className="till-aside">
          <TillPanel
            customer={customer}
            summary={customerSummary}
            onSelectCustomer={selectCustomer}
            priceLevels={priceLevels}
            priceLevelId={priceLevelId}
            onPriceLevel={setPriceLevelId}
            emailInvoices={emailInvoices}
            onEmailInvoices={setEmailInvoices}
            onHistory={() => {
              if (!customer) {
                setError('Open a sale on a customer to see purchase history.')
                return
              }
              setHistoryOpen(true)
            }}
            preview={preview}
            discountsTotal={discountsTotal}
            paintItemCount={paintItemCount}
            fulfilment={fulfilment}
            deliveryFee={deliveryFee}
            onDeliveryFee={setDeliveryFee}
            grandDue={grandDue}
            tillOpen={!!tillId}
            openingFloat={openingFloat}
            onOpeningFloat={setOpeningFloat}
            onOpenTill={() => void doOpenTill()}
            countedCash={countedCash}
            onCountedCash={setCountedCash}
            onCloseTill={() => void doCloseTill()}
            onComplete={openPayment}
            completeDisabled={busy || lines.length === 0}
            onHold={() => void doParkHold()}
            onQuote={() => void doQuote()}
            onVoid={() => void doVoidSale()}
            onOpenDrawer={() => say('Cash drawer opened.')}
            pendingSync={sync?.pending_count ?? '?'}
            onSync={() => void doSync()}
            provider={provider}
            providerSwitchable={providerSwitchable}
            onProvider={(name) => void doSwitchProvider(name)}
            zReport={zReport}
            saleEmpty={lines.length === 0}
          />

          <details className="more-section">
            <summary>Return &amp; layaway</summary>
            <div className="return-box">
              <h3>Return</h3>
              <label>
                Original transaction id
                <input value={returnTxId} onChange={(e) => setReturnTxId(e.target.value)} placeholder="UUID of the sale" />
              </label>
              <label>
                Product SKU
                <input value={returnSku} onChange={(e) => setReturnSku(e.target.value)} placeholder="SKU" />
              </label>
              <label>
                Qty
                <input value={returnQty} onChange={(e) => setReturnQty(e.target.value)} inputMode="decimal" />
              </label>
              <button type="button" onClick={() => void doReturn()} disabled={!returnTxId.trim() || !returnSku.trim()}>Record return</button>
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
            <p className="hint">Mode: {isTauri ? 'Tauri app' : 'dev (HTTP → df-server edge)'} · Pending sync: {sync?.pending_count ?? '?'}</p>
          </details>
        </aside>
      </main>

      <footer className="fnbar">
        <div className="fn-keys">
          <button type="button" onClick={() => document.getElementById('customer-search')?.focus()}>F2 customer</button>
          <button type="button" onClick={() => searchRef.current?.focus()}>F3 lookup</button>
          <button
            type="button"
            onClick={() => {
              const first = lines.find((l) => l.containerVolumeMl != null) ?? lines[0]
              if (first) setTintTarget(first)
            }}
          >
            F4 tint
          </button>
          <button type="button" onClick={() => document.querySelector<HTMLElement>('.disc-input')?.focus()}>F5 discount</button>
          <button type="button" onClick={() => void doParkHold()}>F7 hold</button>
          <button type="button" onClick={openPayment}>F9/F12 tender</button>
        </div>
        <span className="fn-status">{saleLabel} · BM PRICE BOOK 10005510 · TAX: GST+PST (BC)</span>
      </footer>

      {paymentOpen && preview && (
        <PaymentModal
          totalDue={grandDue}
          customer={customer}
          summary={customerSummary}
          onClose={() => setPaymentOpen(false)}
          onComplete={(tenders) => void doCheckout(tenders)}
          onToast={say}
        />
      )}
      {historyOpen && customer && (
        <HistoryModal
          customer={customer}
          onReprint={(inv) => {
            setHistoryOpen(false)
            setInvoice({ data: inv, reprint: true })
          }}
          onClose={() => setHistoryOpen(false)}
          onToast={say}
        />
      )}
      {invoice && (
        <InvoiceView
          invoice={invoice.data}
          reprint={invoice.reprint}
          autoEmail={emailInvoices && !!customer?.email}
          onClose={() => setInvoice(null)}
          onNewSale={newSale}
          onToast={say}
        />
      )}
      {tintTarget && (
        <TintModal
          line={tintTarget}
          customerId={customerId || null}
          onApply={(tint) => {
            setLines((prev) => prev.map((x) => (x.key === tintTarget.key ? { ...x, tint } : x)))
            setTintTarget(null)
          }}
          onClose={() => setTintTarget(null)}
          onToast={say}
        />
      )}
    </div>
  )
}
