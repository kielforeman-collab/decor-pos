// Back-office domain types absorbed into pos-desktop (counts, shelf tags,
// matrix pricing rules, CRM, deliveries/POD, EDI intake/832, reports/BI,
// and Scheduling+). Mirrors df-edge JSON shapes verbatim.

import type { Money } from './types'

// --- Physical counts -------------------------------------------------------

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

export interface CountReconcileResponse {
  count_id: string
  store_id: string
  status: string
  adjustments: CountAdjustment[]
}

// --- Shelf tags & bulk matrix pricing --------------------------------------

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

// --- CRM -------------------------------------------------------------------

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

// --- Delivery & Proof of Delivery ------------------------------------------

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

// --- EDI intake, 832 price review & catalog CSV ----------------------------

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

// --- Reports & BI (Phase 2 + Phase 6) --------------------------------------

export interface DailyUsageReport {
  date: string
  store_id: string
  tint_count?: number
  sales_total?: Money
  [key: string]: unknown
}

export interface LowStockReport {
  date: string
  store_id: string
  low_products?: unknown[]
  [key: string]: unknown
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

// --- Scheduling+ (Phase 6) -------------------------------------------------

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

// --- Sync dead-letters & Edge Admin (Store, Users, Service Tokens) ---------

export interface DeadLetterEntry {
  id: string
  store_id: string
  aggregate_type: string
  aggregate_id: string
  payload: unknown
  created_at: string
  synced_at: string | null
  retry_count: number
  last_error: string | null
  quarantined_at: string | null
  quarantine_reason: string | null
}

export interface DeadLetterActionResponse {
  id: string
  updated: boolean
}

export interface TaxPresetComponentView {
  kind: string
  /** Exact decimal fraction, e.g. "0.0725" for 7.25%. */
  rate: string
}

export interface TaxJurisdictionPresetView {
  country_code: string
  region_code: string
  total_rate: string
  components: TaxPresetComponentView[]
  eco_fee_rule_count: number
}

export interface StoreProfile {
  id: string
  org_id: string
  code: string
  name: string
  country_code: string
  region_code: string
  postal_code: string
  currency: string
  tax_zone_id: string
  timezone: string
  is_warehouse: boolean
}

export interface StoreSettingsResponse {
  store: StoreProfile
  active_tax_preset: TaxJurisdictionPresetView | null
  active_eco_fee_rule_count: number
  available_presets: TaxJurisdictionPresetView[]
}

export interface UpdateStoreSettingsRequest {
  code?: string
  name?: string
  country_code?: string
  region_code?: string
  postal_code?: string
  currency?: string
  timezone?: string
  is_warehouse?: boolean
}

export interface AdminUserSummary {
  id: string
  org_id: string
  email: string
  display_name: string
  role: string
  active: boolean
  has_password: boolean
  has_pin: boolean
  has_badge: boolean
  portal_customer_id: string | null
}

export interface CreateAdminUserRequest {
  email: string
  display_name?: string
  role: string
  password?: string
  pin?: string
  badge_token?: string
  portal_customer_id?: string | null
  active?: boolean
}

export interface UpdateAdminUserRequest {
  display_name?: string
  role?: string
  active?: boolean
  password?: string
  pin?: string
  clear_pin?: boolean
  badge_token?: string
  clear_badge?: boolean
  portal_customer_id?: string | null
}

export interface RevokeUserSessionsResponse {
  user_id: string
  revoked_count: number
}

export interface ServiceTokenSummary {
  id: string
  org_id: string
  store_id: string
  label: string
  scopes: string[]
  created_at: string
  expires_at: string | null
  revoked_at: string | null
  last_used_at: string | null
}

export interface CreateServiceTokenResponse extends ServiceTokenSummary {
  token: string
}

export interface RevokeServiceTokenResponse {
  id: string
  revoked: boolean
}
