// Shared JSON shapes — the edge HTTP API and the Tauri commands use
// the same types (Money serializes as {amount: "12.3400", currency:
// "Usd"}; decimals serialize as strings; never floats).

export type Money = { amount: string; currency: 'Usd' | 'Cad' }
export type TenderKind = 'Cash' | 'Card' | 'ChargeAccount' | 'GiftCard'

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
}

export interface SaleInput {
  customer_id: string | null
  job_id: string | null
  lines: SaleLineInput[]
  tenders: TenderInput[]
  po_number: string | null
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
}

export interface ApplyReport {
  applied: number
  duplicates: number
}

export interface ScanResult {
  barcode: string
  qty: string
  uom_code: string
}

export function usd(amount: string): Money {
  return { amount, currency: 'Usd' }
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
