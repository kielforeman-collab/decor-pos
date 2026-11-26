// Shared JSON shapes for the edge back-office API (Phase 3).
// Money serializes as {amount: "12.3400", currency: "Usd"};
// decimals serialize as strings; never floats.

export type Money = { amount: string; currency: 'Usd' | 'Cad' }

export interface InventoryBalance {
  product_id: string
  sku: string
  name: string
  base_uom: string
  list_price: string
  standard_cost: string
  on_hand: string
  committed: string
  on_order: string
  in_transit: string
}

export interface ReorderPoint {
  id: string
  store_id: string
  product_id: string
  min_qty: string
  max_qty: string
  reorder_qty: string
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

export interface SuggestedPurchaseOrder {
  po_number: string
  vendor_id: string
  lines: SuggestedLine[]
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

export interface ShelfTagResponse {
  product_id: string
  sku: string
  name: string
  price: Money
  barcode: string
  zpl: string
}

export interface PricingRuleRow {
  id: string
  org_id: string
  priority: number
  customer_id: string | null
  job_id: string | null
  price_level_id: string | null
  product_id: string | null
  product_line_id: string | null
  class_id: string | null
  department_id: string | null
  vendor_id: string | null
  min_qty: string
  calc_method: string
  calc_value: string
  min_margin_floor: string | null
  effective_from: string | null
  effective_to: string | null
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

export interface AgingCustomerRow {
  customer_id: string
  account_number: string
  balance: Money
  buckets: [Money, Money, Money, Money, Money]
}

export interface AgingView {
  as_of: string
  totals: [Money, Money, Money, Money, Money]
  customers: AgingCustomerRow[]
}

export interface CollectionsRow {
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
  customers: CollectionsRow[]
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

// --- Phase 4: CRM, delivery, EDI -------------------------------------------
// Shapes mirror the edge JSON (df-crm / df-delivery / df-edi rows).
// Decimals serialize as strings; datetimes as RFC 3339 strings.
// Enum values serialize as their Rust variant names, except
// LapsedStatus which is snake_case ("lapsed" / "never_purchased").

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

export interface Appointment {
  id: string
  org_id: string
  store_id: string
  customer_id: string | null
  staff_id: string | null
  title: string
  starts_at: string
  ends_at: string
  status: string
  notes: string | null
  created_at: string
}

export interface CrmSavedView {
  id: string
  org_id: string
  store_id: string
  name: string
  filter_json: unknown
  created_at: string
}

export interface SavedViewCustomer {
  customer_id: string
  account_number: string
  company_name: string | null
  customer_type: string
  charge_enabled: boolean
  credit_hold: boolean
  balance: Money
  last_purchase_at: string | null
}

export interface LapsedContractor {
  customer_id: string
  account_number: string
  company_name: string | null
  status: 'lapsed' | 'never_purchased'
  last_purchase_at: string | null
  trailing_spend: string
  ar_balance: string
}

export interface LapsedReport {
  store_id: string
  as_of: string
  inactive_days: number
  include_never_purchased: boolean
  customers: LapsedContractor[]
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

export interface PackingSlipLine {
  product_id: string
  description: string
  qty: string
  uom: string
}

export interface PackingSlipView {
  delivery_id: string
  transaction_id: string | null
  doc_number: string | null
  customer_id: string | null
  address_json: unknown
  scheduled_date: string | null
  status: string
  lines: PackingSlipLine[]
  text: string
  zpl: string
}

export interface EdiDocument {
  id: string
  doc_type: string
  direction: string
  vendor_id: string
  related_po_id: string | null
  raw_text: string
  parsed_json: unknown
  received_at: string
}

export interface AckLineResult {
  line_no: number
  vendor_sku: string
  ack_code: string
  po_qty: string
  ack_qty: string
  qty_difference: string
  unit_cost_changed: boolean
}

export interface AckApplication {
  po_number: string
  accepted_lines: number
  rejected_lines: number
  changed_lines: number
  line_results: AckLineResult[]
  unmatched_skus: string[]
}

export interface IntakeResponse {
  doc_id: string
  doc_type: string
  parsed: boolean
  related_po_id: string | null
  parsed_json: unknown
  note: string
  ack_application?: AckApplication | null
  price_application_id?: string | null
}

export interface SendPoResponse {
  po_id: string
  po_number: string
  doc_id: string
  x12_850: string
  written_to: string | null
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
  qty_variance_amount: string
  price_variance_amount: string
  total_variance_amount: string
  status: string
  variance_kind: string
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
  stated_total_difference: string | null
  created_at: string
}

export interface InvoiceMatchReportRow {
  id: string
  po_id: string
  report: InvoiceMatchReport
}

export interface PoAcknowledgmentRow {
  id: string
  po_id: string
  ack: {
    po_number: string
    ack_date: string | null
    vendor_id: string | null
    lines: {
      line_no: number
      vendor_sku: string
      ack_code: string
      qty_acknowledged: string
      uom: string
      unit_cost: string | null
      promised_date: string | null
    }[]
  }
  received_at: string
}

export interface AppliedPriceLine {
  vendor_sku: string
  action: string
  old_cost: string | null
  old_list_price: string | null
  new_cost: string | null
  new_list_price: string | null
  result: string
  note: string
}

export interface PriceCatalogApplicationRow {
  id: string
  application: {
    id: string
    vendor_id: string
    catalog_id: string
    catalog_date: string
    status: string
    reviewed_by: string | null
    lines: AppliedPriceLine[]
  }
  applied_at: string | null
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

export interface CatalogItem {
  vendor_sku: string
  description: string
  cost: string
  list_price: string
  uom: string
  barcode: string | null
}

export interface CatalogImportResponse {
  imported: number
  items: CatalogItem[]
  errors: { row: number; message: string }[]
}

// --- Phase 6: BI -------------------------------------------------------------
// Shapes mirror the edge JSON (df-reports kpi / eco / outbox).
// Decimals serialize as strings; Money as {amount, currency}.

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

export interface EcoJurisdictionRow {
  country_code: string
  region_code: string
  eco_fee_category: string | null
  store_id: string
  store_code: string
  transaction_count: number
  line_count: number
  units: string
  eco_fee_total: Money
}

export interface EcoRemittanceReport {
  org_id: string
  start: string
  end: string
  currency: 'Usd' | 'Cad'
  rows: EcoJurisdictionRow[]
  total: Money
}

export interface InquiryRow {
  transaction_id: string
  doc_number: string
  doc_type: string
  status: string
  created_at: string
  customer_id: string | null
  subtotal: Money
  tax_total: Money
  eco_fee_total: Money
  grand_total: Money
  line_count: number
  product_ids: string[]
}

export interface EmailDelivery {
  id: string
  report_kind: string
  recipient: string
  subject: string
  file_name: string
  file_sha256: string
  scheduled_at: string
  created_at: string
  status: string
}

// --- Phase 6: Scheduling+ ----------------------------------------------------
// Shapes mirror the edge JSON (df-scheduling rows + forecast report).
// Decimals serialize as strings; datetimes as RFC 3339 strings.
// All scheduling times are UTC in v1.0 (see df-edge::scheduling).

export interface ShiftTemplate {
  id: string
  org_id: string
  store_id: string
  name: string
  weekday: number
  starts_at: string
  ends_at: string
  role: string | null
  staff_id: string | null
  required_count: number
  is_active: boolean
}

export interface StaffShift {
  id: string
  org_id: string
  store_id: string
  template_id: string | null
  staff_id: string
  shift_date: string
  starts_at: string
  ends_at: string
  status: string
  notes: string | null
}

export interface TimeclockEntry {
  id: string
  org_id: string
  store_id: string
  staff_id: string
  clock_in: string
  clock_out: string | null
  source: string
}

export interface HourBucket {
  hour: number
  avg_txns: string
  avg_sales: string
  required_hours: string
}

export interface DayForecast {
  date: string
  weekday: number
  forecast_txns: string
  forecast_sales: string
  required_hours: string
  hourly: HourBucket[]
}

export interface ForecastReport {
  store_id: string
  generated_at: string
  history_days: number
  history_txns: number
  history_sales: string
  history_labor_hours: string
  sales_per_labor_hour: string
  ratio_from_history: boolean
  days: DayForecast[]
}

export interface CoverageCheck {
  date: string
  forecast_hours: string | null
  scheduled_hours: string
  delta_hours: string | null
  status: 'adequate' | 'under' | 'over' | 'unknown'
}
