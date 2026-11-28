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

import { apiUrl, isTauri, tauriInvoke as invoke } from './apiBase'
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
  DashboardAggregate,
  InvoiceData,
  KpiDashboard,
  Money,
  PriceLevel,
  PricePreview,
  ProductImportResponse,
  ProductImportRow,
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

export { isTauri }

async function http<T>(path: string, body?: unknown): Promise<T> {
  const res = await fetch(await apiUrl(path), {
    method: body === undefined ? 'GET' : 'POST',
    // Every call carries the signed-in Bearer session token in both
    // browser dev mode (Vite `/api` proxy) and the installed native
    // Tauri app (embedded loopback `df-edge` HTTP server), so RBAC and
    // real cashier attribution are enforced identically everywhere.
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
  store_id: string
  store_name: string
  provider: string
  pending_outbox: number
}> {
  if (isTauri) return invoke('get_status')
  const health = await http<{ store_id: string; status: string }>('/health')
  const sync = await http<SyncState>('/pos/sync/state')
  return {
    store_id: health.store_id,
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
  return http('/pos/carts/price-preview', input)
}

export async function checkout(input: SaleInput): Promise<CompletedSale> {
  return http('/pos/transactions', input)
}

export async function openTill(
  cashier_id: string,
  opening_float: Money,
): Promise<TillSession> {
  return http('/pos/tills/open', { cashier_id, opening_float })
}

export async function closeTill(till_id: string, counted_cash: Money): Promise<ZReport> {
  return http(`/pos/tills/${till_id}/close`, { counted_cash })
}

export async function parkHold(input: SaleInput): Promise<{ hold_id: string }> {
  return http('/pos/holds', input)
}

export async function listHolds(): Promise<{ hold_ids: string[] }> {
  return http('/pos/holds')
}

export async function resumeHold(hold_id: string): Promise<SaleInput> {
  return http(`/pos/holds/${hold_id}/resume`, {})
}

export async function recordQuote(input: SaleInput): Promise<{ transaction_id: string }> {
  return http('/pos/quotes', input)
}

export async function recordLayaway(
  sale: SaleInput,
  deposit: TenderInput,
): Promise<{ transaction_id: string }> {
  return http('/pos/layaways', { sale, deposit })
}

export async function recordReturn(
  transaction_id: string,
  lines: { product_id: string; qty: string }[],
): Promise<CompletedSale> {
  return http(`/pos/transactions/${transaction_id}/return`, lines)
}

export async function voidTransaction(
  transaction_id: string,
): Promise<{ transaction_id: string; status: string }> {
  return http(`/pos/transactions/${transaction_id}/void`, {})
}

export async function syncState(): Promise<SyncState> {
  return http('/pos/sync/state')
}

export async function syncRun(): Promise<ApplyReport> {
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
  const found = await httpGetOptional<ProductSearchResult[]>(`/pos/products/search?${qs}`)
  return found ?? []
}

export async function getPriceLevels(): Promise<PriceLevel[]> {
  return (await httpGetOptional<PriceLevel[]>('/pricing/levels')) ?? []
}

export async function searchCustomers(q: string): Promise<CustomerSearchResult[]> {
  const found = await httpGetOptional<RawCustomer[]>(`/pos/customers/search?q=${encodeURIComponent(q)}`)
  return (found ?? []).map(mapCustomer)
}

export async function getCustomerSummary(id: string): Promise<ArCustomerSummary | null> {
  return httpGetOptional<ArCustomerSummary>(`/ar/customers/${encodeURIComponent(id)}/summary`)
}

export async function getCustomerTransactions(
  id: string,
  limit = 25,
): Promise<CustomerTransaction[]> {
  const found = await httpGetOptional<RawTransactionView[]>(
    `/pos/customers/${encodeURIComponent(id)}/transactions?limit=${limit}`,
  )
  return (found ?? []).map(mapCustomerTransaction)
}

export async function getTransaction(id: string): Promise<TransactionView | null> {
  return httpGetOptional<TransactionView>(`/pos/transactions/${encodeURIComponent(id)}`)
}

export async function getInvoice(id: string): Promise<InvoiceData | null> {
  const raw = await httpGetOptional<RawInvoice>(`/pos/transactions/${encodeURIComponent(id)}/invoice`)
  return raw ? mapInvoice(raw) : null
}

export async function listTintColors(query = '', manufacturer = ''): Promise<TintColorEntry[]> {
  const params = new URLSearchParams()
  if (manufacturer) params.set('manufacturer', manufacturer)
  if (query) params.set('query', query)
  return (await httpGetOptional<TintColorEntry[]>(`/pos/tinting/colors?${params.toString()}`)) ?? []
}

export async function listTintColorants(): Promise<TintColorantEntry[]> {
  return (await httpGetOptional<TintColorantEntry[]>('/pos/tinting/colorants')) ?? []
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
  try {
    const saved = await http<RawTintFormula>('/pos/tinting/formulas', body)
    return mapTintFormula(saved)
  } catch (e) {
    if (e instanceof Error && e.message === 'HTTP 404') return null
    throw e
  }
}

// --- stores / orders / transfers / accounts ---

export async function setCustomerPriceLevel(
  id: string,
  priceLevelId: string | null,
): Promise<CustomerSearchResult> {
  const raw = await http<RawCustomer>(`/pos/customers/${id}/price-level`, {
    price_level_id: priceLevelId,
  })
  return mapCustomer(raw)
}

export interface Store {
  id: string
  org_id: string
  code: string
  name: string
  country_code: string | null
  region_code: string | null
  postal_code: string | null
  currency: string
  tax_zone_id: string | null
  timezone: string
  is_warehouse: boolean
}

export interface InventoryBalance {
  product_id: string
  sku: string
  name: string
  base_uom: string
  // Plain decimal strings on the wire (df_db::InventoryBalance
  // carries Decimals, not Money).
  list_price: string
  standard_cost: string
  msrp: string | null
  map: string | null
  on_hand: string
  committed: string
  on_order: string
  in_transit: string
}

export interface SuggestedLine {
  product_id: string
  sku: string
  description: string
  qty: string
  uom: string
  unit_cost: string
  on_hand: string
  on_order: string
  min_qty: string
  max_qty: string
  velocity_per_day: string
}

// One preview PO per vendor — the real PO number is assigned on create.
export interface SuggestedPurchaseOrder {
  po_number: string
  vendor_id: string
  lines: SuggestedLine[]
}

export interface PurchaseOrderLine {
  id: string
  line_no: number
  product_id: string
  sku: string
  description: string
  qty: string
  received_qty: string
  uom: string
  unit_cost: string
}

export interface PurchaseOrderView {
  id: string
  po_number: string
  store_id: string
  vendor_id: string
  status: string
  total: string
  created_at: string
  lines: PurchaseOrderLine[]
}

export interface TransferLine {
  product_id: string
  qty: string
  unit_cost: string
}

export interface Transfer {
  id: string
  from_store_id: string
  to_store_id: string
  status: string
  created_at: string
  lines: TransferLine[]
}

export interface AgingCustomer {
  customer_id: string
  account_number: string
  balance: Money
  // [current, 31–60, 61–90, 91–120, 120+]
  buckets: [Money, Money, Money, Money, Money]
}

export interface AgingView {
  as_of: string
  totals: [Money, Money, Money, Money, Money]
  customers: AgingCustomer[]
}

export interface CollectionsCustomer {
  customer_id: string
  account_number: string
  company_name: string | null
  balance: Money
  credit_limit: Money
  credit_hold: boolean
  overdue: Money
  oldest_due_date: string | null
}

export interface CollectionsView {
  as_of: string
  customers: CollectionsCustomer[]
}

export interface PostedArPayment {
  payment_entry_id: string
  customer_id: string
  amount: Money
  method: string
  // (charge entry id, allocated amount) pairs, oldest first.
  allocations: [string, string][]
}

export interface EdiDocumentRow {
  id: string
  doc_type: string
  direction: string
  vendor_id: string
  related_po_id: string | null
  received_at: string
}

export interface PoAcknowledgmentRow {
  id: string
  po_id: string
  vendor_id: string
  acknowledgement_number: string
  status: string
  received_at: string
}

export interface MatchLine {
  po_line_no: number | null
  vendor_sku: string
  po_qty: string
  received_qty: string
  invoiced_qty: string
  po_unit_cost: string
  invoiced_unit_price: string
  qty_variance: string
  price_variance_per_unit: string
  total_variance_amount: string
  status: string
}

export interface InvoiceMatchReport {
  id: string
  po_number: string
  invoice_number: string
  shipment_id: string
  status: string
  lines: MatchLine[]
  po_total: string
  invoiced_total: string
  total_variance_amount: string
}

export async function listStores(): Promise<Store[]> {
  return http<Store[]>('/stores')
}

export async function inventoryBalances(storeId: string): Promise<InventoryBalance[]> {
  return http<InventoryBalance[]>(`/inventory/balances?store_id=${storeId}`)
}

export async function suggestReplenishment(
  storeId: string | null,
  days?: number,
  leadTimeDays?: number,
): Promise<SuggestedPurchaseOrder[]> {
  return http<SuggestedPurchaseOrder[]>('/purchasing/replenishment/suggest', {
    store_id: storeId,
    days: days ?? null,
    lead_time_days: leadTimeDays ?? null,
  })
}

export async function listPurchaseOrders(storeId: string): Promise<PurchaseOrderView[]> {
  return http<PurchaseOrderView[]>(`/purchasing/orders?store_id=${storeId}`)
}

export interface PoLineInput {
  product_id: string
  qty: string
  uom?: string
  unit_cost?: string
}

export async function createPurchaseOrder(
  vendorId: string,
  storeId: string | null,
  lines: PoLineInput[],
): Promise<PurchaseOrderView> {
  return http<PurchaseOrderView>('/purchasing/orders', {
    vendor_id: vendorId,
    store_id: storeId,
    lines,
  })
}

export async function receivePurchaseOrder(
  id: string,
  lines: { product_id: string; qty: string }[],
): Promise<PurchaseOrderView> {
  return http<PurchaseOrderView>(`/purchasing/orders/${id}/receive`, { lines })
}

export async function listTransfers(storeId: string): Promise<Transfer[]> {
  return http<Transfer[]>(`/inventory/transfers?store_id=${storeId}`)
}

export async function createTransfer(
  fromStoreId: string,
  toStoreId: string,
  lines: TransferLine[],
): Promise<Transfer> {
  return http<Transfer>('/inventory/transfers', {
    from_store_id: fromStoreId,
    to_store_id: toStoreId,
    lines,
  })
}

export async function shipTransfer(id: string): Promise<Transfer> {
  return http<Transfer>(`/inventory/transfers/${id}/ship`, {})
}

export async function receiveTransfer(id: string): Promise<Transfer> {
  return http<Transfer>(`/inventory/transfers/${id}/receive`, {})
}

export async function getArAging(storeId: string): Promise<AgingView> {
  return http<AgingView>(`/ar/aging?store_id=${storeId}`)
}

export async function getArCollections(storeId: string): Promise<CollectionsView> {
  return http<CollectionsView>(`/ar/collections?store_id=${storeId}`)
}

export async function postArPayment(
  customerId: string,
  amount: Money,
  method: string,
): Promise<PostedArPayment> {
  return http<PostedArPayment>('/ar/payments', { customer_id: customerId, amount, method })
}

export async function sendPurchaseOrder(
  id: string,
  outDir?: string,
): Promise<{ po_id: string; path: string; raw: string }> {
  return http<{ po_id: string; path: string; raw: string }>(`/edi/purchase-orders/${id}/send`, {
    out_dir: outDir ?? null,
  })
}

export async function listEdiDocuments(poId: string): Promise<EdiDocumentRow[]> {
  return http<EdiDocumentRow[]>(`/edi/documents?po_id=${poId}`)
}

export async function listPoAcknowledgments(poId: string): Promise<PoAcknowledgmentRow[]> {
  return http<PoAcknowledgmentRow[]>(`/edi/acknowledgments?po_id=${poId}`)
}

export async function threeWayMatch(poId: string): Promise<InvoiceMatchReport> {
  return http<InvoiceMatchReport>(`/edi/three-way-match?po_id=${poId}`)
}

// --- products: import, reorder points -------------------------------------
// The import endpoint upserts by SKU (new SKUs created, existing SKUs
// get new prices); the spreadsheet itself is parsed in the browser
// (see productImport.ts) and only the parsed rows are sent.

export async function importProducts(
  rows: ProductImportRow[],
): Promise<ProductImportResponse> {
  return http<ProductImportResponse>('/pos/inventory/import', { rows })
}

export interface ReorderPoint {
  id: string
  store_id: string
  product_id: string
  min_qty: string
  max_qty: string
  reorder_qty: string
}

export async function listReorderPoints(storeId: string): Promise<ReorderPoint[]> {
  return (await httpGetOptional<ReorderPoint[]>(`/inventory/reorder-points?store_id=${storeId}`)) ?? []
}

export async function setReorderPoint(
  storeId: string,
  productId: string,
  minQty: string,
  maxQty: string,
  reorderQty: string,
): Promise<ReorderPoint> {
  const body = {
    product_id: productId,
    store_id: storeId,
    min_qty: minQty,
    max_qty: maxQty,
    reorder_qty: reorderQty,
  }
  return http<ReorderPoint>('/inventory/reorder-points', body)
}

// --- vendors ---------------------------------------------------------------

export interface Vendor {
  id: string
  name: string
  display_name: string | null
  contact_name: string | null
  email: string | null
  phone: string | null
  edi_enabled: boolean
  po_count: number
  open_po_count: number
  last_po_at: string | null
}

export interface VendorDetail {
  vendor: Vendor
  purchase_orders: PurchaseOrderView[]
}

export async function listVendors(): Promise<Vendor[]> {
  return (await httpGetOptional<Vendor[]>('/vendors')) ?? []
}

export async function getVendor(id: string): Promise<VendorDetail | null> {
  return httpGetOptional<VendorDetail>(`/vendors/${encodeURIComponent(id)}`)
}

export async function createVendor(body: {
  name: string
  display_name?: string | null
  contact_name?: string | null
  email?: string | null
  phone?: string | null
  edi_enabled?: boolean
}): Promise<Vendor> {
  return http<Vendor>('/vendors', body)
}

export async function updateVendor(
  id: string,
  patch: {
    name?: string
    display_name?: string | null
    contact_name?: string | null
    email?: string | null
    phone?: string | null
    edi_enabled?: boolean
  },
): Promise<Vendor> {
  return http<Vendor>(`/vendors/${encodeURIComponent(id)}`, patch)
}

export interface TransportRecord {
  id: string
  vendor_id: string
  direction: string
  transport: string
  file_name: string
  doc_type: string | null
  status: string
  error: string | null
  created_at: string
}

export async function listTransportRecords(vendorId: string): Promise<TransportRecord[]> {
  return (await httpGetOptional<TransportRecord[]>(`/edi/transport?vendor_id=${encodeURIComponent(vendorId)}`)) ?? []
}

// --- BI dashboards ---------------------------------------------------------

export async function getKpiDashboard(
  storeId: string,
  start: string,
  end: string,
): Promise<KpiDashboard | null> {
  const q = new URLSearchParams({ store_id: storeId, start, end, as_of: end })
  return httpGetOptional<KpiDashboard>(`/bi/kpi?${q}`)
}

export async function getDashboardAggregate(
  storeId: string,
  start: string,
  end: string,
): Promise<DashboardAggregate | null> {
  const q = new URLSearchParams({ store_id: storeId, start, end })
  return httpGetOptional<DashboardAggregate>(`/bi/dashboard?${q}`)
}
