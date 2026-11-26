// Shared JSON shapes for the mobile edge API (Phase 5, v0.6).
// Money serializes as {amount: "12.3400", currency: "Usd"};
// decimals serialize as strings; never floats. Shapes mirror the
// back-office types where the same endpoint is used.

export type Money = { amount: string; currency: 'Usd' | 'Cad' }

export interface MobileKpis {
  date: string
  sales_today: Money
  sales_today_count: number
  open_quotes: number
  open_orders: number
  ar_open_total: Money
  deliveries_out_today: number
}

export interface PoLineView {
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
  vendor_id: string
  store_id: string
  status: string
  total: string
  lines: PoLineView[]
  created_at: string
}

export interface TransferLineView {
  product_id: string
  qty: string
  unit_cost: string
}

export interface Transfer {
  id: string
  from_store_id: string
  to_store_id: string
  status: string
  lines: TransferLineView[]
  created_at: string
}

export interface CountLineView {
  product_id: string
  counted_qty: string
  system_qty: string
  variance_qty: string
  unit_cost: string
}

export interface CountAdjustment {
  product_id: string
  on_hand_before: string
  counted_qty: string
  adjustment: string
}

export interface PhysicalCount {
  id: string
  store_id: string
  status: string
  lines: CountLineView[]
  adjustments: CountAdjustment[]
  created_at: string
  counted_at: string | null
}

export interface CrmActivity {
  id: string
  org_id: string
  store_id: string
  customer_id: string | null
  activity_type: string
  subject: string
  notes: string | null
  created_by: string | null
  due_at: string | null
  completed_at: string | null
  snoozed_until: string | null
  created_at: string
}

export interface DeliveryPod {
  id: string
  delivery_id: string
  signature_sha256: string
  signature_b64: string
  photo_ref: string | null
  photo_sha256: string | null
  photo_at: string | null
  received_by: string
  signed_at: string
  created_at: string
}

export interface DeliveryEvent {
  id: string
  delivery_id: string
  from_status: string
  to_status: string
  actor_user_id: string
  note: string | null
  created_at: string
}

export interface DeliveryView {
  id: string
  org_id: string
  store_id: string
  transaction_id: string | null
  customer_id: string | null
  driver_id: string | null
  status: string
  address_json: unknown
  scheduled_date: string | null
  priority: number
  vehicle: string | null
  pod: DeliveryPod | null
  events: DeliveryEvent[]
  created_at: string
  updated_at: string
}

export interface SaleLineInput {
  product_id: string
  sku: string
  description: string
  qty: string
  uom: string
  unit_price: Money
  unit_cost: Money
}

export interface SaleInput {
  customer_id: string | null
  job_id: string | null
  lines: SaleLineInput[]
  tenders: []
  po_number: string | null
}

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

export function fmtAddress(address: unknown): string {
  if (!address) return '—'
  if (typeof address === 'string') return address
  if (typeof address === 'object') {
    const a = address as Record<string, unknown>
    const parts = [a.street, a.city, a.region, a.postal_code].filter(
      (p) => typeof p === 'string' && p,
    )
    if (parts.length) return parts.join(', ')
    return JSON.stringify(address)
  }
  return String(address)
}
