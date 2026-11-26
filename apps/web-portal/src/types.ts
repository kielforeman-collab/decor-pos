// Shared JSON shapes for the edge contractor-portal API (Phase 5, v0.6).
// Money serializes as {amount: "12.3400", currency: "Usd"};
// decimals serialize as strings; datetimes as RFC 3339 strings.
// Never floats, never parseFloat for arithmetic — amounts are
// displayed as the server sent them.
//
// NOTE: statements return raw StatementRow decimals (e.g.
// opening_balance: "400.0000"), not Money objects — that is what
// the edge actually sends; the portal types them as strings.

export type Money = { amount: string; currency: 'Usd' | 'Cad' }

export type CurrencyCode = 'Usd' | 'Cad'

export interface Customer {
  id: string
  org_id: string
  account_number: string
  customer_type: string
  company_name: string | null
  first_name: string | null
  last_name: string | null
  email: string | null
  phone: string | null
  price_level_id: string | null
  salesperson_id: string | null
  tax_exempt_cert: string | null
  tax_exempt_expires: string | null
  charge_enabled: boolean
  credit_limit: string
  payment_terms: string
  terms_days: number | null
  finance_rate_pct_monthly: string | null
  credit_hold: boolean
  require_po_number: boolean
  require_job_name: boolean
  portal_enabled: boolean
}

export interface PortalCustomerView {
  customer: Customer
  open_balance: Money
}

export interface Transaction {
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
}

export interface StatementRow {
  id: string
  customer_id: string
  period_start: string
  period_end: string
  opening_balance: string
  charges_total: string
  payments_total: string
  finance_charge_total: string
  closing_balance: string
  aging_json: Record<string, string>
  pdf_sha256: string | null
  created_at: string
}

export interface TintFormula {
  id: string
  org_id: string
  store_id: string
  customer_id: string | null
  job_id: string | null
  color_id: string | null
  color_code_snapshot: string
  color_name_snapshot: string
  base_product_id: string
  container_volume_ml: number
  sheen: string | null
  room_or_usage: string | null
  is_custom_match: boolean
  colorants_json: unknown
  total_tint_cost: string
  tint_surcharge: string
  dispenser_name: string | null
  external_formula_ref: string | null
  dispensed_at: string
}

export interface PortalPaymentRow {
  idempotency_key: string
  customer_id: string
  payment_entry_id: string
  amount: string
  method: string
  provider_reference: string
  created_at: string
}

export interface PortalReorderResponse {
  order_id: string
  doc_number: string
  formula_id: string
  fulfillment: string
  grand_total: Money
}

export interface BillPayResult {
  payment_entry_id: string
  customer_id: string
  amount: Money
  method: string
  provider_reference: string
  /** (charge entry id, allocated amount), oldest charge first. */
  allocations: [string, string][]
  /** True when this call replayed an earlier payment. */
  replayed: boolean
}

/** Display a Money value exactly as the server sent it. */
export function fmtMoney(m: Money | null | undefined): string {
  if (!m) return '—'
  const code = m.currency === 'Cad' ? 'CAD' : 'USD'
  return `${m.amount} ${code}`
}

/** Display a decimal-string amount exactly as the server sent it. */
export function fmtDecimal(amount: string | null | undefined, currency = 'USD'): string {
  if (amount === null || amount === undefined) return '—'
  return `${amount} ${currency}`
}

export function customerDisplayName(c: Customer): string {
  if (c.company_name) return c.company_name
  const name = [c.first_name, c.last_name].filter(Boolean).join(' ')
  return name || c.account_number
}
