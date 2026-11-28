// Shared JSON shapes — the edge HTTP API and the Tauri commands use
// the same types (Money serializes as {amount: "12.3400", currency:
// "Usd"}; decimals serialize as strings; never floats).

export type Money = { amount: string; currency: 'Usd' | 'Cad' }
// Card brands are sent as their own PascalCase kind; the server
// authorizes them as Card (3% surcharge) and ETransfer with no
// surcharge and no provider auth.
export type TenderKind =
  | 'Cash'
  | 'Card'
  | 'ChargeAccount'
  | 'GiftCard'
  | 'Visa'
  | 'Mastercard'
  | 'Amex'
  | 'Debit'
  | 'ETransfer'

export const TENDER_LABELS: Record<TenderKind, string> = {
  Cash: 'Cash',
  Card: 'Card',
  ChargeAccount: 'Charge to account',
  GiftCard: 'Gift card',
  Visa: 'Visa',
  Mastercard: 'Mastercard',
  Amex: 'American Express',
  Debit: 'Debit (Interac)',
  ETransfer: 'Interac e-Transfer',
}

/** Card-brand kinds authorize as Card server-side. */
export const CARD_BRAND_KINDS: TenderKind[] = ['Visa', 'Mastercard', 'Amex', 'Debit']

export interface SaleLineInput {
  product_id: string
  sku: string
  description: string
  qty: string
  uom: string
  unit_price: Money
  unit_cost: Money
}

export interface TenderInput {
  kind: TenderKind
  amount: Money | null // null = remaining balance
  /** Optional reference (e.g. e-Transfer reference / auth code). */
  reference?: string | null
}

export interface SaleInput {
  customer_id: string | null
  job_id: string | null
  lines: SaleLineInput[]
  tenders: TenderInput[]
  po_number: string | null
  price_level_id?: string | null
  job_name?: string | null
}

export interface PricePreviewLine {
  product_id: string
  description: string
  qty: string
  unit_price: Money
  line_total: Money
  tax_amount: Money
  eco_fee_amount: Money
}

export interface PricePreview {
  lines: PricePreviewLine[]
  subtotal: Money
  tax_total: Money
  eco_fee_total: Money
  pos_fee_total: Money
  grand_total: Money
}

export interface PaymentResult {
  status: 'Approved' | 'Declined' | 'Voided'
  authorization: string | null
  reference: string
  amount: Money
  surcharge: Money
  tender: TenderKind
}

export interface CompletedSale {
  transaction_id: string
  doc_number: string
  subtotal: Money
  tax_total: Money
  eco_fee_total: Money
  pos_fee_total: Money
  grand_total: Money
  change_due: Money
  payments: PaymentResult[]
  receipt_printed: boolean
}

export interface TillSession {
  id: string
  org_id: string
  store_id: string
  cashier_id: string
  opened_at: string
  closed_at: string | null
  opening_float: string
  counted_cash: string | null
  expected_cash: string | null
  status: string
}

export interface ZReport {
  till_id: string
  opened_at: string
  closed_at: string
  expected_cash: Money
  counted_cash: Money
  over_short: Money
  sale_count: number
  gross_sales: Money
  tax_total: Money
  cash_sales: Money
  card_sales: Money
  charge_sales: Money
  return_count: number
  return_total: Money
}

export interface SyncState {
  store_id: string
  last_batch_id: string | null
  last_synced_at: string | null
  pending_count: number
  dead_letter_count?: number
}

export interface ApplyReport {
  applied: number
  duplicates: number
  applied_ids?: string[]
  conflict_ids?: string[]
}

export interface ScanResult {
  barcode: string
  qty: string
  uom_code: string
}

// --- Counter-UI port: product / customer / invoice / tinting -----------

/** Raw wire shape of `GET /pos/transactions/{id}` (a `Transaction`
 * row flattened, with its lines — each carrying the resolved SKU —
 * and payments). Decimals are strings; datetimes are ISO strings.
 * The UI does not render this directly today; `getTransaction` is
 * kept correctly typed for the day it does. */
export interface TransactionViewLine {
  id: string
  transaction_id: string
  line_no: number
  product_id: string
  description: string
  qty: string
  uom_code: string
  unit_price: string
  base_unit_cost: string
  tint_unit_cost: string
  discount_amount: string
  tax_amount: string
  eco_fee_amount: string
  line_total: string
  tint_formula_id: string | null
  prompts_json: unknown
  vertical_spec_json: unknown
  linked_po_line_id: string | null
  fulfilled_qty: string
  sku: string | null
}

export interface TransactionViewPayment {
  id: string
  transaction_id: string
  tender: string
  amount: string
  reference: string
  authorization: string | null
  created_at: string
}

export interface TransactionView {
  id: string
  org_id: string
  store_id: string
  till_session_id: string | null
  doc_number: string
  doc_type: string
  status: string
  customer_id: string | null
  job_id: string | null
  po_number: string | null
  job_name: string | null
  salesperson_id: string | null
  cashier_id: string | null
  currency: string
  subtotal: string
  tax_total: string
  eco_fee_total: string
  pos_fee_total: string
  grand_total: string
  deposit_paid: string
  fulfillment_mode: string
  scheduled_at: string | null
  completed_at: string | null
  created_at: string
  lines: TransactionViewLine[]
  payments: TransactionViewPayment[]
}

export interface ProductSearchResult {
  product_id: string
  sku: string
  name: string
  base_uom: string
  list_price: string // decimal string
  standard_cost: string // decimal string
  msrp: string | null
  map: string | null
  container_volume_ml: number | null
  on_hand: string | null
}

export interface PriceLevel {
  id: string
  code: string
  name: string
  discount_pct: string // e.g. "0.20"
}

export interface CustomerSearchResult {
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
  credit_limit: Money
  payment_terms: string | null
  credit_hold: boolean
}

export interface ArCustomerSummary {
  customer_id: string
  account_number: string
  company_name: string | null
  customer_type: string
  charge_enabled: boolean
  credit_limit: Money
  credit_hold: boolean
  balance: Money
  entry_count: number
  oldest_due_date: string | null
}

export interface CustomerTransactionLine {
  product_id?: string
  sku: string
  description: string
  qty: string
  uom?: string
  unit_price: Money
  line_total?: Money
  amount?: Money
  size_label?: string | null
  tint?: string | null
}

export interface CustomerTransaction {
  id: string
  transaction_id?: string
  doc_number: string
  created_at: string
  job_name?: string | null
  po_number?: string | null
  grand_total: Money
  total?: Money
  lines: CustomerTransactionLine[]
}

export interface InvoiceLine {
  sku: string
  description: string
  qty: string
  uom: string
  unit_price: Money
  disc_pct?: string | null
  amount: Money
  size_label?: string | null
  tint?: string | null
}

export interface InvoiceTender {
  kind: TenderKind
  label?: string
  amount: Money
  tendered?: Money | null
  change?: Money | null
  reference?: string | null
}

export interface InvoiceTotals {
  subtotal: Money
  discounts: Money
  eco_fee_total: Money
  delivery: Money
  gst: Money
  pst: Money
  grand_total: Money
  change_due: Money
  charge?: {
    previous_balance: Money
    charged: Money
    new_balance: Money
    credit_limit: Money
    available_credit: Money
  } | null
}

export interface InvoiceData {
  store_name: string
  doc_number: string
  created_at: string
  customer: {
    name: string
    account_number?: string | null
    email?: string | null
    address?: string | null
  } | null
  job_name: string | null
  po_number: string | null
  fulfilment_mode: string | null
  sale_number?: string | null
  clerk?: string | null
  terms?: string | null
  note?: string | null
  lines: InvoiceLine[]
  tenders: InvoiceTender[]
  totals: InvoiceTotals
}

export interface TintColorantAmount {
  code: string
  oz: string
  shots?: number
}

export interface TintColorEntry {
  id?: string
  color_code: string
  color_name: string
  manufacturer?: string | null
  collection?: string | null
  rgb_hex?: string | null
  recommended_base?: string | null
}

export interface TintColorantEntry {
  colorant_id?: string
  system_name?: string
  code: string
  name: string
  cost_per_fl_oz?: string
  shots_per_fl_oz?: number
}

export interface TintFormulaEntry {
  id?: string
  customer_id?: string | null
  color_code: string
  color_name: string
  base_product_id?: string | null
  container_volume_ml?: number | null
  colorants: TintColorantAmount[]
}

/** Tint recorded on a cart line. Ounces are real (stored formula or
 * clerk-entered from the BM formula source) — never seeded/hashed. */
export interface TintInfo {
  colorCode: string
  colorName: string
  colorHex?: string | null
  base: string
  formula: TintColorantAmount[]
}

export interface CartLine {
  key: string
  product_id: string
  sku: string
  description: string
  qty: string
  uom: string
  /** List (retail) unit price, decimal string. */
  unit_price: string
  unit_cost: string
  /** Line discount %, "0"–"90". Applied to unit_price when sending. */
  discPct: string
  sizeLabel?: string
  containerVolumeMl?: number | null
  tint?: TintInfo | null
}

export interface TenderRow {
  key: string
  kind: TenderKind
  /** Applied amount, decimal string. '' = remaining balance (legacy). */
  amount: string
  /** Cash only: amount actually tendered (may exceed applied). */
  tendered?: string
}

/** The counter serves the BC store, so every amount the UI builds is
 * CAD — the edge server must run with `--currency CAD`. Sending USD
 * money is rejected by the server ("line prices must use the store
 * currency"), so there is no `cad()` helper: use `cad()` for all
 * UI-built amounts. */
export const STORE_CURRENCY: Money['currency'] = 'Cad'

export function cad(amount: string): Money {
  return { amount, currency: STORE_CURRENCY }
}

/** Display-only formatting (2 decimals); all math stays server-side. */
export function fmtMoney(m: Money | null | undefined): string {
  if (!m) return '$0.00'
  const n = Number(m.amount)
  const symbol = m.currency === 'Cad' ? 'CA$' : '$'
  const sign = n < 0 ? '-' : ''
  return `${sign}${symbol}${Math.abs(n).toFixed(2)}`
}

export function fmtQty(qty: string): string {
  const n = Number(qty)
  return Number.isInteger(n) ? String(n) : String(n)
}

/**
 * Exact decimal math for the one client-side price transform the API
 * forces: a per-line discount % has no field in SaleLineInput, so the
 * discounted unit price is sent and the server stays authoritative
 * for tax, eco fees and totals. Integer (BigInt) arithmetic on
 * 4-decimal-place units — no floats.
 */
export function discountedPrice(listPrice: string, discPct: string): string {
  const pct = Number(discPct || '0')
  if (!Number.isFinite(pct) || pct <= 0) return listPrice
  if (pct >= 100) return '0.0000'
  const parse4 = (s: string): bigint => {
    const neg = s.trim().startsWith('-')
    const [i, f = ''] = s.trim().replace('-', '').split('.')
    const v = BigInt(i || '0') * 10000n + BigInt((f + '0000').slice(0, 4) || '0')
    return neg ? -v : v
  }
  const price4 = parse4(listPrice)
  // pct in hundredths of a percent, exact for up to 2dp inputs.
  const pct100 = BigInt(Math.round(pct * 100))
  const num = price4 * (10000n - pct100)
  // Round half-up to 4dp units.
  const rounded = (num + 5000n) / 10000n
  const sign = rounded < 0n ? '-' : ''
  const abs = rounded < 0n ? -rounded : rounded
  return `${sign}${(abs / 10000n).toString()}.${(abs % 10000n).toString().padStart(4, '0')}`
}

/** Decimal-string addition (exact, 4dp) for display sums of inputs. */
export function addDecimals(a: string, b: string): string {
  const parse4 = (s: string): bigint => {
    const neg = s.trim().startsWith('-')
    const [i, f = ''] = s.trim().replace('-', '').split('.')
    const v = BigInt(i || '0') * 10000n + BigInt((f + '0000').slice(0, 4) || '0')
    return neg ? -v : v
  }
  const sum = parse4(a) + parse4(b)
  const sign = sum < 0n ? '-' : ''
  const abs = sum < 0n ? -sum : sum
  return `${sign}${(abs / 10000n).toString()}.${(abs % 10000n).toString().padStart(4, '0')}`
}

/** Exact decimal multiplication (4dp in, 4dp out, half-up) — display sums only. */
export function mulDecimals(a: string, b: string): string {
  const parse4 = (s: string): bigint => {
    const neg = s.trim().startsWith('-')
    const [i, f = ''] = s.trim().replace('-', '').split('.')
    const v = BigInt(i || '0') * 10000n + BigInt((f + '0000').slice(0, 4) || '0')
    return neg ? -v : v
  }
  const prod = parse4(a) * parse4(b) // 8dp units
  const rounded = (prod + 5000n) / 10000n // back to 4dp units, half-up
  const sign = rounded < 0n ? '-' : ''
  const abs = rounded < 0n ? -rounded : rounded
  return `${sign}${(abs / 10000n).toString()}.${(abs % 10000n).toString().padStart(4, '0')}`
}

/** Short size label from a container volume (ml), per the invoice spec. */
export function sizeLabelForVolume(ml: number | null | undefined, uom: string): string | undefined {
  if (ml == null) return undefined
  if (ml >= 17000) return '5-gal pail'
  if (ml >= 3000) return 'Gallon'
  if (ml >= 800) return 'Quart'
  if (ml > 0) return 'Sample pot'
  return undefined
}

/** Decimal-string compare: -1 | 0 | 1. */
export function cmpDecimal(a: string, b: string): number {
  const n = (s: string): bigint => {
    const neg = s.trim().startsWith('-')
    const [i, f = ''] = s.trim().replace('-', '').split('.')
    const v = BigInt(i || '0') * 10000n + BigInt((f + '0000').slice(0, 4) || '0')
    return neg ? -v : v
  }
  const d = n(a) - n(b)
  return d < 0n ? -1 : d > 0n ? 1 : 0
}

// --- Product import (ported from web-backoffice types.ts) -----------------
// One parsed row of a Benjamin Moore price list. Category, UPC, MSRP
// and MAP are not stored (no columns for them on the edge).
export interface ProductImportRow {
  row: number
  sku: string
  name: string
  category: string
  line: string
  color: string
  size: string
  upc: string
  msrp: string
  map: string
  net_price: string
  list_price: string
  brand_code: string
}

export interface ProductImportFailure {
  row: number
  sku: string
  reason: string
}

export interface ProductImportResponse {
  created: number
  updated: number
  failed: number
  failures: ProductImportFailure[]
}

// --- BI dashboards (ported from web-backoffice types.ts) -----------------
// Shapes mirror the edge JSON (df-reports kpi). Decimals serialize as
// strings; Money as {amount, currency}.
export interface KpiSales {
  sale_count: number
  return_count: number
  gross_sales: Money
  returns: Money
  net_sales: Money
  tax_total: Money
  eco_fee_total: Money
  grand_total: Money
  avg_ticket: Money | null
}

export interface KpiMargin {
  revenue: Money
  cogs: Money
  gross_profit: Money
  margin_pct: string | null
}

export interface KpiInventory {
  on_hand_value: Money
  avg_inventory_value: Money
  turns: string | null
}

export interface FormulaColorUsage {
  color_code: string
  color_name: string
  formulas: number
}

export interface FormulaUsage {
  formulas_created: number
  per_color: FormulaColorUsage[]
  total_tint_cost: Money
}

export interface AgingSummary {
  current: Money
  days_31_60: Money
  days_61_90: Money
  days_91_120: Money
  over_120: Money
}

export interface KpiDashboard {
  org_id: string
  store_id: string
  start: string
  end: string
  as_of: string
  currency: 'Usd' | 'Cad'
  sales: KpiSales
  margin: KpiMargin
  inventory: KpiInventory
  ar_aging: AgingSummary
  total_open_ar: Money
  formulas: FormulaUsage
}

// --- Dashboard aggregate (GET /bi/dashboard contract) --------------------
export interface DashboardAggregate {
  store_id: string
  start: string
  end: string
  sales: {
    sale_count: number
    return_count: number
    gross_sales: Money
    net_sales: Money
    grand_total: Money
    avg_ticket: Money | null
  }
  daily: { date: string; net_sales: Money; sale_count: number }[]
  by_category: { category: string; net_sales: Money; share_pct: number }[]
  tenders: { tender: string; amount: Money; count: number; share_pct: number }[]
  top_products: { product_id: string; sku: string; name: string; qty: string; revenue: Money }[]
  top_customers: { customer_id: string; name: string; net_sales: Money; sale_count: number }[]
  tint: {
    formulas_created: number
    total_tint_cost: Money
    per_color: FormulaColorUsage[]
  }
  low_stock: { product_id: string; sku: string; name: string; on_hand: string; min_qty: string | null }[]
}
