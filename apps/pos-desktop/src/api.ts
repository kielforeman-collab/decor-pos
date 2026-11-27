// Edge client with two backends, one interface.
//
// * Native Tauri app: `window.__TAURI_INTERNALS__` is present and
//   calls go through `invoke()` to the in-process Edge (same JSON
//   shapes as the HTTP API). Provider switching and the captured
//   receipt text only exist here.
// * Dev path (plain browser): calls go to `/api/*`, proxied by Vite
//   to a `df-server edge` on 127.0.0.1:8080. The server's provider is
//   whatever it was started with (mock by default), so
//   `providerSwitchable` is false and `lastReceiptText()` is null —
//   the receipt preview is rendered from the checkout response.
//
// New counter-UI endpoints (GET /pos/products/search,
// /pricing/levels, /pos/customers/search, /pos/customers/{id}/…,
// /pos/transactions/{id}/invoice, POST /pos/tinting/formulas) are
// being implemented in crates/ in parallel: the list/get helpers
// below treat a 404 as "not implemented yet" and return an empty
// list / null so the UI keeps working with manual entry, instead of
// failing the whole screen.

import {
  UnauthorizedError,
  authHeaders,
  clearSession,
  notifyUnauthorized,
} from './auth'
import { parseScannerInput } from './scan'
import { cad } from './types'
import type {
  ApplyReport,
  ArCustomerSummary,
  CompletedSale,
  CustomerSearchResult,
  CustomerTransaction,
  InvoiceData,
  Money,
  PriceLevel,
  PricePreview,
  ProductSearchResult,
  SaleInput,
  ScanResult,
  SyncState,
  TenderInput,
  TenderKind,
  TillSession,
  TintColorEntry,
  TintColorantAmount,
  TintColorantEntry,
  TintFormulaEntry,
  TransactionView,
  ZReport,
} from './types'

export const isTauri =
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

type InvokeFn = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>

let invokeFn: Promise<InvokeFn> | null = null

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!invokeFn) {
    invokeFn = import('@tauri-apps/api/core').then((m) => m.invoke as InvokeFn)
  }
  return (await invokeFn)(cmd, args)
}

async function http<T>(path: string, body?: unknown): Promise<T> {
  const res = await fetch(`/api${path}`, {
    method: body === undefined ? 'GET' : 'POST',
    // Phase 7: every call carries the session token. In Tauri mode the
    // invoke() calls below cannot carry a Bearer token (local IPC, not
    // HTTP) — enforcement for those is the Tauri command layer's job,
    // not this wrapper's; the login screen still gates the UI.
    headers: { 'content-type': 'application/json', ...authHeaders() },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  const json = await res.json().catch(() => null)
  if (!res.ok) {
    if (res.status === 401) {
      clearSession()
      notifyUnauthorized()
      throw new UnauthorizedError()
    }
    throw new Error(typeof json?.error === 'string' ? json.error : `HTTP ${res.status}`)
  }
  return json as T
}

/** GET that returns `null` on 404 (endpoint not implemented yet) or an
 * empty body — same empty-response tolerance as `http()`. */
async function httpGetOptional<T>(path: string): Promise<T | null> {
  try {
    return await http<T>(path)
  } catch (e) {
    if (e instanceof Error && e.message === 'HTTP 404') return null
    throw e
  }
}

export const providerSwitchable = isTauri

export async function getStatus(): Promise<{
  store_name: string
  provider: string
  pending_outbox: number
}> {
  if (isTauri) return invoke('get_status')
  const health = await http<{ store_id: string; status: string }>('/health')
  const sync = await http<SyncState>('/pos/sync/state')
  return {
    store_name: `Store ${health.store_id.slice(0, 8)}`,
    provider: 'mock (df-server edge)',
    pending_outbox: sync.pending_count,
  }
}

export async function setProvider(name: string): Promise<string> {
  if (!isTauri) throw new Error('Provider switching needs the native Tauri app')
  return invoke('set_provider', { name })
}

export async function pricePreview(input: SaleInput): Promise<PricePreview> {
  if (isTauri) return invoke('price_preview', { input })
  return http('/pos/carts/price-preview', input)
}

export async function checkout(input: SaleInput): Promise<CompletedSale> {
  if (isTauri) return invoke('checkout', { input })
  return http('/pos/transactions', input)
}

export async function openTill(
  cashier_id: string,
  opening_float: Money,
): Promise<TillSession> {
  if (isTauri) return invoke('open_till', { cashier_id, opening_float })
  return http('/pos/tills/open', { cashier_id, opening_float })
}

export async function closeTill(till_id: string, counted_cash: Money): Promise<ZReport> {
  if (isTauri) return invoke('close_till', { till_id, counted_cash })
  return http(`/pos/tills/${till_id}/close`, { counted_cash })
}

export async function parkHold(input: SaleInput): Promise<{ hold_id: string }> {
  if (isTauri) {
    const hold_id = await invoke<string>('park_hold', { input })
    return { hold_id }
  }
  return http('/pos/holds', input)
}

export async function listHolds(): Promise<{ hold_ids: string[] }> {
  if (isTauri) {
    const hold_ids = await invoke<string[]>('list_holds')
    return { hold_ids }
  }
  return http('/pos/holds')
}

export async function resumeHold(hold_id: string): Promise<SaleInput> {
  if (isTauri) return invoke('resume_hold', { hold_id })
  return http(`/pos/holds/${hold_id}/resume`, {})
}

export async function recordQuote(input: SaleInput): Promise<{ transaction_id: string }> {
  if (isTauri) {
    const transaction_id = await invoke<string>('record_quote', { input })
    return { transaction_id }
  }
  return http('/pos/quotes', input)
}

export async function recordLayaway(
  sale: SaleInput,
  deposit: TenderInput,
): Promise<{ transaction_id: string }> {
  if (isTauri) {
    const transaction_id = await invoke<string>('record_layaway', {
      input: sale,
      deposit,
    })
    return { transaction_id }
  }
  return http('/pos/layaways', { sale, deposit })
}

export async function recordReturn(
  transaction_id: string,
  lines: { product_id: string; qty: string }[],
): Promise<CompletedSale> {
  if (isTauri) return invoke('record_return', { original_tx_id: transaction_id, lines })
  return http(`/pos/transactions/${transaction_id}/return`, lines)
}

export async function voidTransaction(
  transaction_id: string,
): Promise<{ transaction_id: string; status: string }> {
  if (isTauri) return invoke('void_transaction', { id: transaction_id })
  return http(`/pos/transactions/${transaction_id}/void`, {})
}

export async function syncState(): Promise<SyncState> {
  if (isTauri) return invoke('sync_state')
  return http('/pos/sync/state')
}

export async function syncRun(): Promise<ApplyReport> {
  if (isTauri) return invoke('sync_now')
  return http('/pos/sync/run', {})
}

/** Last receipt text captured by the mock printer (Tauri only). */
export async function lastReceiptText(): Promise<string | null> {
  if (!isTauri) return null
  return invoke('last_receipt_text')
}

/**
 * Scan one barcode input. Native: the real df-hw parser in Rust.
 * Dev path: the TS mirror in scan.ts (barcode == SKU); the caller
 * resolves the barcode against product search.
 */
export async function scanBarcode(input: string): Promise<ScanResult> {
  if (isTauri) {
    // No demo catalog anymore — the Rust parser resolves the barcode
    // itself; an empty catalog list means "no local overrides".
    return invoke('scan_barcode', { input, catalog: [] })
  }
  const parsed = parseScannerInput(input)
  if (!parsed) throw new Error('Empty barcode')
  return { barcode: parsed.barcode, qty: String(parsed.qty), uom_code: 'ea' }
}

// --- Counter-UI port: products, pricing, customers, invoices, tint ---

// --- Wire-shape mapping -------------------------------------------------
// The edge returns Decimal money as bare strings ("49.1200") and model
// rows (Customer, TransactionView, InvoiceView, TintFormula) whose
// field names differ from the UI types. Map at this boundary so the
// components keep working with Money objects and the field names they
// expect; without it the invoice/history screens render $NaN and the
// history filter crashes on a missing `sku`. The Tauri path returns
// the same JSON shapes, so both paths go through the same mappers.

type Dec = string | number | null | undefined

function decToMoney(v: Dec): Money {
  return cad(v == null ? '0.0000' : String(v))
}

interface RawCustomer {
  id: string
  account_number: string
  customer_type: string
  company_name: string | null
  first_name: string | null
  last_name: string | null
  email: string | null
  phone: string | null
  price_level_id: string | null
  charge_enabled: boolean
  credit_limit: Dec
  payment_terms: string | null
  credit_hold: boolean
}

export function mapCustomer(raw: RawCustomer): CustomerSearchResult {
  return {
    id: raw.id,
    account_number: raw.account_number,
    customer_type: raw.customer_type,
    company_name: raw.company_name ?? null,
    first_name: raw.first_name ?? null,
    last_name: raw.last_name ?? null,
    email: raw.email ?? null,
    phone: raw.phone ?? null,
    price_level_id: raw.price_level_id ?? null,
    charge_enabled: raw.charge_enabled,
    credit_limit: decToMoney(raw.credit_limit),
    payment_terms: raw.payment_terms ?? null,
    credit_hold: raw.credit_hold,
  }
}

interface RawTransactionLine {
  product_id?: string
  sku?: string | null
  description: string
  qty: Dec
  uom?: string | null
  uom_code?: string | null
  unit_price: Dec
  line_total?: Dec
  amount?: Dec
  size_label?: string | null
  tint?: string | null
}

interface RawTransactionView {
  id: string
  doc_number: string
  created_at: string
  job_name?: string | null
  po_number?: string | null
  grand_total: Dec
  lines?: RawTransactionLine[] | null
}

export function mapCustomerTransaction(raw: RawTransactionView): CustomerTransaction {
  return {
    id: raw.id,
    doc_number: raw.doc_number,
    created_at: raw.created_at,
    job_name: raw.job_name ?? null,
    po_number: raw.po_number ?? null,
    grand_total: decToMoney(raw.grand_total),
    lines: (raw.lines ?? []).map((l) => ({
      product_id: l.product_id,
      sku: l.sku ?? '',
      description: l.description,
      qty: String(l.qty ?? '0'),
      uom: l.uom ?? l.uom_code ?? 'ea',
      unit_price: decToMoney(l.unit_price),
      line_total: decToMoney(l.line_total ?? l.amount ?? '0'),
      size_label: l.size_label ?? null,
      tint: typeof l.tint === 'string' ? l.tint : null,
    })),
  }
}

interface RawInvoiceTint {
  color_name: string
  color_code: string
  base: string
  formula: { code: string; oz: Dec }[]
}

interface RawInvoiceLine {
  sku: string
  description: string
  qty: Dec
  uom: string
  unit_price: Dec
  discount_amount?: Dec
  line_total: Dec
  size_label?: string | null
  tint?: RawInvoiceTint | null
}

interface RawInvoiceTender {
  label: string
  amount: Dec
  tendered: Dec
  change: Dec
}

interface RawInvoiceTotals {
  subtotal: Dec
  discounts: Dec
  eco_fee_total: Dec
  delivery: Dec
  gst: Dec
  pst: Dec
  grand_total: Dec
  change_due: Dec
  charge?: {
    previous_balance: Dec
    charged: Dec
    new_balance: Dec
    credit_limit: Dec
    available: Dec
  } | null
}

interface RawInvoice {
  store_name: string
  doc_number: string
  invoice_number?: string | null
  created_at: string
  customer: {
    name?: string | null
    account_number?: string | null
    email?: string | null
  } | null
  job_name?: string | null
  po_number?: string | null
  fulfilment_mode?: string | null
  lines: RawInvoiceLine[]
  tenders: RawInvoiceTender[]
  totals: RawInvoiceTotals
}

function tenderKindFromLabel(label: string): TenderKind {
  const l = label.toLowerCase()
  if (l.includes('visa')) return 'Visa'
  if (l.includes('mastercard')) return 'Mastercard'
  if (l.includes('amex') || l.includes('american express')) return 'Amex'
  if (l.includes('debit') || l.includes('interac')) return 'Debit'
  if (l.includes('cash')) return 'Cash'
  if (l.includes('charge')) return 'ChargeAccount'
  if (l.includes('gift')) return 'GiftCard'
  if (l.includes('e-transfer') || l.includes('etransfer')) return 'ETransfer'
  return 'Card'
}

export function mapInvoice(raw: RawInvoice): InvoiceData {
  const charge = raw.totals.charge
  return {
    store_name: raw.store_name,
    doc_number: raw.doc_number,
    created_at: raw.created_at,
    customer: raw.customer
      ? {
          name: raw.customer.name ?? 'Walk-in',
          account_number: raw.customer.account_number ?? null,
          email: raw.customer.email ?? null,
        }
      : null,
    job_name: raw.job_name ?? null,
    po_number: raw.po_number ?? null,
    fulfilment_mode: raw.fulfilment_mode ?? null,
    sale_number: raw.invoice_number ?? null,
    clerk: null,
    terms: null,
    note: null,
    lines: raw.lines.map((l) => {
      const disc = Number(l.discount_amount ?? 0)
      const gross = Number(l.line_total ?? 0) + disc
      return {
        sku: l.sku,
        description: l.description,
        qty: String(l.qty ?? '0'),
        uom: l.uom,
        unit_price: decToMoney(l.unit_price),
        // Level discounts are baked into unit_price by the server, so
        // discount_amount only covers explicit per-line discounts.
        disc_pct: disc > 0 && gross > 0 ? String((disc / gross) * 100) : null,
        amount: decToMoney(l.line_total),
        size_label: l.size_label ?? null,
        tint: l.tint
          ? `TINT: ${l.tint.color_name} ${l.tint.color_code} · ${l.tint.base} · Formula: ${l.tint.formula.map((f) => `${f.code} ${f.oz} oz`).join(', ')}`
          : null,
      }
    }),
    tenders: raw.tenders.map((t) => ({
      kind: tenderKindFromLabel(t.label),
      label: t.label,
      amount: decToMoney(t.amount),
      tendered: decToMoney(t.tendered),
      change: decToMoney(t.change),
      reference: null,
    })),
    totals: {
      subtotal: decToMoney(raw.totals.subtotal),
      discounts: decToMoney(raw.totals.discounts),
      eco_fee_total: decToMoney(raw.totals.eco_fee_total),
      delivery: decToMoney(raw.totals.delivery),
      gst: decToMoney(raw.totals.gst),
      pst: decToMoney(raw.totals.pst),
      grand_total: decToMoney(raw.totals.grand_total),
      change_due: decToMoney(raw.totals.change_due),
      charge: charge
        ? {
            previous_balance: decToMoney(charge.previous_balance),
            charged: decToMoney(charge.charged),
            new_balance: decToMoney(charge.new_balance),
            credit_limit: decToMoney(charge.credit_limit),
            available_credit: decToMoney(charge.available),
          }
        : null,
    },
  }
}

interface RawTintFormula {
  id?: string
  customer_id?: string | null
  job_id?: string | null
  color_code?: string | null
  color_code_snapshot?: string | null
  color_name?: string | null
  color_name_snapshot?: string | null
  base_product_id?: string | null
  container_volume_ml?: number | null
  colorants?: TintColorantAmount[] | null
  colorants_json?: { code: string; oz: Dec; shots?: number | string | null }[] | null
}

export function mapTintFormula(raw: RawTintFormula): TintFormulaEntry {
  const colorants =
    raw.colorants ??
    (raw.colorants_json ?? []).map((c) => ({
      code: c.code,
      oz: String(c.oz ?? '0'),
      shots: typeof c.shots === 'number' ? c.shots : Number(c.shots ?? 0),
    }))
  return {
    id: raw.id,
    customer_id: raw.customer_id ?? null,
    color_code: raw.color_code ?? raw.color_code_snapshot ?? '',
    color_name: raw.color_name ?? raw.color_name_snapshot ?? '',
    base_product_id: raw.base_product_id ?? null,
    container_volume_ml: raw.container_volume_ml ?? null,
    colorants,
  }
}

export async function searchProducts(opts: {
  q?: string
  sku?: string
  barcode?: string
  limit?: number
}): Promise<ProductSearchResult[]> {
  const params = new URLSearchParams()
  if (opts.q) params.set('q', opts.q)
  if (opts.sku) params.set('sku', opts.sku)
  if (opts.barcode) params.set('barcode', opts.barcode)
  if (opts.limit) params.set('limit', String(opts.limit))
  const qs = params.toString()
  if (isTauri) {
    try {
      return await invoke<ProductSearchResult[]>('search_products', {
        q: opts.q ?? null,
        sku: opts.sku ?? null,
        barcode: opts.barcode ?? null,
        limit: opts.limit ?? 50,
      })
    } catch {
      return []
    }
  }
  const found = await httpGetOptional<ProductSearchResult[]>(`/pos/products/search?${qs}`)
  return found ?? []
}

export async function getPriceLevels(): Promise<PriceLevel[]> {
  if (isTauri) {
    try {
      return await invoke<PriceLevel[]>('list_price_levels')
    } catch {
      return []
    }
  }
  return (await httpGetOptional<PriceLevel[]>('/pricing/levels')) ?? []
}

export async function searchCustomers(q: string): Promise<CustomerSearchResult[]> {
  if (isTauri) {
    try {
      const found = await invoke<RawCustomer[]>('search_customers', { q })
      return found.map(mapCustomer)
    } catch {
      return []
    }
  }
  const found = await httpGetOptional<RawCustomer[]>(`/pos/customers/search?q=${encodeURIComponent(q)}`)
  return (found ?? []).map(mapCustomer)
}

export async function getCustomerSummary(id: string): Promise<ArCustomerSummary | null> {
  if (isTauri) {
    try {
      return await invoke<ArCustomerSummary>('customer_summary', { id })
    } catch {
      return null
    }
  }
  return httpGetOptional<ArCustomerSummary>(`/ar/customers/${encodeURIComponent(id)}/summary`)
}

export async function getCustomerTransactions(
  id: string,
  limit = 25,
): Promise<CustomerTransaction[]> {
  if (isTauri) {
    try {
      const found = await invoke<RawTransactionView[]>('customer_transactions', { id, limit })
      return found.map(mapCustomerTransaction)
    } catch {
      return []
    }
  }
  const found = await httpGetOptional<RawTransactionView[]>(
    `/pos/customers/${encodeURIComponent(id)}/transactions?limit=${limit}`,
  )
  return (found ?? []).map(mapCustomerTransaction)
}

export async function getTransaction(id: string): Promise<TransactionView | null> {
  if (isTauri) {
    try {
      return await invoke<TransactionView>('get_transaction', { id })
    } catch {
      return null
    }
  }
  return httpGetOptional<TransactionView>(`/pos/transactions/${encodeURIComponent(id)}`)
}

export async function getInvoice(id: string): Promise<InvoiceData | null> {
  if (isTauri) {
    try {
      const raw = await invoke<RawInvoice>('get_invoice', { id })
      return mapInvoice(raw)
    } catch {
      return null
    }
  }
  const raw = await httpGetOptional<RawInvoice>(`/pos/transactions/${encodeURIComponent(id)}/invoice`)
  return raw ? mapInvoice(raw) : null
}

export async function listTintColors(query = '', manufacturer = ''): Promise<TintColorEntry[]> {
  const params = new URLSearchParams()
  if (manufacturer) params.set('manufacturer', manufacturer)
  if (query) params.set('query', query)
  if (isTauri) {
    try {
      return await invoke<TintColorEntry[]>('list_tint_colors', { manufacturer, query })
    } catch {
      return []
    }
  }
  return (await httpGetOptional<TintColorEntry[]>(`/pos/tinting/colors?${params.toString()}`)) ?? []
}

export async function listTintColorants(): Promise<TintColorantEntry[]> {
  if (isTauri) {
    try {
      const res = await invoke<{ levels: TintColorantEntry[] }>('list_tint_colorants')
      return res.levels ?? []
    } catch {
      return []
    }
  }
  const res = await httpGetOptional<{ levels: TintColorantEntry[] }>('/pos/tinting/colorants')
  return res?.levels ?? []
}

export async function listTintFormulas(opts: {
  query?: string
  customer_id?: string
  job_id?: string
}): Promise<TintFormulaEntry[]> {
  const params = new URLSearchParams()
  if (opts.query) params.set('query', opts.query)
  if (opts.customer_id) params.set('customer_id', opts.customer_id)
  if (opts.job_id) params.set('job_id', opts.job_id)
  if (isTauri) {
    try {
      const found = await invoke<RawTintFormula[]>('list_tint_formulas', {
        query: opts.query ?? null,
        customer_id: opts.customer_id ?? null,
        job_id: opts.job_id ?? null,
      })
      return found.map(mapTintFormula)
    } catch {
      return []
    }
  }
  const found = await httpGetOptional<RawTintFormula[]>(`/pos/tinting/formulas?${params.toString()}`)
  return (found ?? []).map(mapTintFormula)
}

export async function saveTintFormula(body: {
  customer_id?: string | null
  job_id?: string | null
  color_code: string
  color_name: string
  base_product_id: string
  container_volume_ml: number
  colorants: TintColorantAmount[]
}): Promise<TintFormulaEntry | null> {
  if (isTauri) {
    try {
      const saved = await invoke<RawTintFormula>('create_tint_formula', { ...body })
      return mapTintFormula(saved)
    } catch {
      return null
    }
  }
  try {
    const saved = await http<RawTintFormula>('/pos/tinting/formulas', body)
    return mapTintFormula(saved)
  } catch (e) {
    if (e instanceof Error && e.message === 'HTTP 404') return null
    throw e
  }
}
