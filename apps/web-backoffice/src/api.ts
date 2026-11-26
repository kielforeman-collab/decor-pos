// Back-office client: plain HTTP to the edge (Vite proxies
// `/api/*` to a `df-server edge` on 127.0.0.1:8080). No mocks —
// every call hits the real edge API against the same SQLite edge
// database the counter POS uses.
//
// Phase 7: every call carries `Authorization: Bearer <token>` from
// localStorage key `df-pos-token` (same key in all four Phase 7 apps).
// A 401 clears the session and notifies the app (see src/auth.ts),
// which returns to the login screen.

import {
  UnauthorizedError,
  authHeaders,
  clearSession,
  notifyUnauthorized,
} from './auth'
import type {
  AgingView,
  Appointment,
  ArCustomerSummary,
  CatalogImportResponse,
  CollectionsView,
  CoverageCheck,
  ForecastReport,
  ShiftTemplate,
  StaffShift,
  TimeclockEntry,
  CrmActivity,
  CrmSavedView,
  DeliveryView,
  EcoRemittanceReport,
  EmailDelivery,
  EdiDocument,
  InquiryRow,
  IntakeResponse,
  InventoryBalance,
  InvoiceMatchReport,
  KpiDashboard,
  InvoiceMatchReportRow,
  LapsedReport,
  PackingSlipView,
  PhysicalCount,
  PoAcknowledgmentRow,
  PriceCatalogApplicationRow,
  PricingRuleRow,
  PurchaseOrderView,
  ReorderPoint,
  SavedViewCustomer,
  SendPoResponse,
  ShelfTagResponse,
  SuggestedPurchaseOrder,
  Transfer,
  TransportRecord,
} from './types'

async function http<T>(path: string, body?: unknown): Promise<T> {
  const res = await fetch(`/api${path}`, {
    method: body === undefined ? 'GET' : 'POST',
    headers: { 'content-type': 'application/json', ...authHeaders() },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  const json = await res.json()
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

export async function getHealth(): Promise<{ store_id: string; status: string }> {
  return http('/health')
}

export async function getBalances(storeId: string): Promise<InventoryBalance[]> {
  return http(`/inventory/balances?store_id=${storeId}`)
}

export async function getReorderPoints(storeId: string): Promise<ReorderPoint[]> {
  return http(`/inventory/reorder-points?store_id=${storeId}`)
}

export async function setReorderPoint(body: {
  product_id: string
  store_id: string
  min_qty: string
  max_qty: string
  reorder_qty: string
}): Promise<ReorderPoint> {
  return http('/inventory/reorder-points', body)
}

export async function suggestReplenishment(body: {
  store_id: string
  days: number
  lead_time_days: number
}): Promise<SuggestedPurchaseOrder[]> {
  return http('/purchasing/replenishment/suggest', body)
}

export async function createPurchaseOrder(body: {
  vendor_id: string
  store_id: string
  lines: { product_id: string; qty: string }[]
}): Promise<PurchaseOrderView> {
  return http('/purchasing/orders', body)
}

export async function listPurchaseOrders(storeId: string): Promise<PurchaseOrderView[]> {
  return http(`/purchasing/orders?store_id=${storeId}`)
}

export async function receivePurchaseOrder(
  poId: string,
  lines: { product_id: string; qty: string }[],
): Promise<PurchaseOrderView> {
  return http(`/purchasing/orders/${poId}/receive`, { lines })
}

export async function createTransfer(body: {
  from_store_id: string
  to_store_id: string
  lines: { product_id: string; qty: string; unit_cost: string }[]
}): Promise<Transfer> {
  return http('/inventory/transfers', body)
}

export async function listTransfers(storeId: string): Promise<Transfer[]> {
  return http(`/inventory/transfers?store_id=${storeId}`)
}

export async function shipTransfer(id: string): Promise<Transfer> {
  return http(`/inventory/transfers/${id}/ship`, {})
}

export async function receiveTransfer(id: string): Promise<Transfer> {
  return http(`/inventory/transfers/${id}/receive`, {})
}

export async function startCount(storeId: string): Promise<PhysicalCount> {
  return http('/inventory/counts', { store_id: storeId })
}

export async function listCounts(storeId: string): Promise<PhysicalCount[]> {
  return http(`/inventory/counts?store_id=${storeId}`)
}

export async function scanCount(
  id: string,
  productId: string,
  countedQty: string,
): Promise<PhysicalCount> {
  return http(`/inventory/counts/${id}/scan`, {
    product_id: productId,
    counted_qty: countedQty,
  })
}

export async function reconcileCount(
  id: string,
): Promise<{ count_id: string; adjustments: { product_id: string; on_hand_before: string; counted_qty: string; adjustment: string }[] }> {
  return http(`/inventory/counts/${id}/reconcile`, {})
}

export async function getShelfTag(productId: string): Promise<ShelfTagResponse> {
  return http(`/inventory/shelf-tags?product_id=${productId}`)
}

export async function listPricingRules(): Promise<PricingRuleRow[]> {
  return http('/pricing/rules')
}

export async function bulkUpdatePricingRules(
  rules: { id: string; calc_method: string; calc_value: string; min_margin_floor: string | null }[],
): Promise<PricingRuleRow[]> {
  return http('/pricing/rules', { rules })
}

export async function getArSummary(customerId: string): Promise<ArCustomerSummary> {
  return http(`/ar/customers/${customerId}/summary`)
}

export async function getArAging(storeId: string, asOf?: string): Promise<AgingView> {
  const query = asOf ? `?store_id=${storeId}&as_of=${asOf}` : `?store_id=${storeId}`
  return http(`/ar/aging${query}`)
}

export async function getArCollections(storeId: string): Promise<CollectionsView> {
  return http(`/ar/collections?store_id=${storeId}`)
}

// --- Phase 4: CRM -----------------------------------------------------------

export async function listActivities(params?: {
  customer_id?: string
  activity_type?: string
  completed?: boolean
}): Promise<CrmActivity[]> {
  const q = new URLSearchParams()
  if (params?.customer_id) q.set('customer_id', params.customer_id)
  if (params?.activity_type) q.set('activity_type', params.activity_type)
  if (params?.completed !== undefined) q.set('completed', String(params.completed))
  const suffix = q.toString() ? `?${q}` : ''
  return http(`/crm/activities${suffix}`)
}

export async function createActivity(body: {
  customer_id?: string | null
  activity_type: string
  subject: string
  notes?: string | null
  created_by?: string | null
  due_at?: string | null
}): Promise<CrmActivity> {
  return http('/crm/activities', body)
}

export async function completeActivity(id: string): Promise<CrmActivity> {
  return http(`/crm/activities/${id}/complete`, {})
}

export async function snoozeActivity(id: string, snoozedUntil: string): Promise<CrmActivity> {
  return http(`/crm/activities/${id}/snooze`, { snoozed_until: snoozedUntil })
}

export async function deleteActivity(id: string): Promise<{ id: string; deleted: boolean }> {
  return http(`/crm/activities/${id}/delete`, {})
}

export async function listDueReminders(asOf?: string): Promise<CrmActivity[]> {
  return http(asOf ? `/crm/reminders/due?as_of=${encodeURIComponent(asOf)}` : '/crm/reminders/due')
}

export async function listOverdueReminders(asOf?: string): Promise<CrmActivity[]> {
  return http(
    asOf ? `/crm/reminders/overdue?as_of=${encodeURIComponent(asOf)}` : '/crm/reminders/overdue',
  )
}

export async function listAppointments(params?: {
  date?: string
  customer_id?: string
  staff_id?: string
}): Promise<Appointment[]> {
  const q = new URLSearchParams()
  if (params?.date) q.set('date', params.date)
  if (params?.customer_id) q.set('customer_id', params.customer_id)
  if (params?.staff_id) q.set('staff_id', params.staff_id)
  const suffix = q.toString() ? `?${q}` : ''
  return http(`/crm/appointments${suffix}`)
}

export async function createAppointment(body: {
  customer_id?: string | null
  staff_id?: string | null
  title: string
  starts_at: string
  ends_at: string
  notes?: string | null
}): Promise<Appointment> {
  return http('/crm/appointments', body)
}

export async function cancelAppointment(id: string): Promise<Appointment> {
  return http(`/crm/appointments/${id}/cancel`, {})
}

export async function deleteAppointment(id: string): Promise<{ id: string; deleted: boolean }> {
  return http(`/crm/appointments/${id}/delete`, {})
}

export async function listSavedViews(): Promise<CrmSavedView[]> {
  return http('/crm/saved-views')
}

export async function createSavedView(body: {
  name: string
  filter_json: unknown
}): Promise<CrmSavedView> {
  return http('/crm/saved-views', body)
}

export async function runSavedView(id: string): Promise<CrmActivity[]> {
  return http(`/crm/saved-views/${id}/run`)
}

export async function runSavedViewCustomers(id: string): Promise<SavedViewCustomer[]> {
  return http(`/crm/saved-views/${id}/run-customers`)
}

export async function deleteSavedView(id: string): Promise<{ id: string; deleted: boolean }> {
  return http(`/crm/saved-views/${id}/delete`, {})
}

export async function getLapsedReport(params: {
  store_id: string
  as_of?: string
  inactive_days?: number
  include_never?: boolean
}): Promise<LapsedReport> {
  const q = new URLSearchParams({ store_id: params.store_id })
  if (params.as_of) q.set('as_of', params.as_of)
  if (params.inactive_days !== undefined) q.set('inactive_days', String(params.inactive_days))
  if (params.include_never !== undefined) q.set('include_never', String(params.include_never))
  return http(`/crm/reports/lapsed?${q}`)
}

// --- Phase 4: delivery ------------------------------------------------------

export async function createDelivery(body: {
  transaction_id?: string | null
  customer_id?: string | null
  address_json: unknown
  scheduled_date?: string | null
  priority?: number
  vehicle?: string | null
}): Promise<DeliveryView> {
  return http('/deliveries', body)
}

export async function listDispatch(params?: {
  status?: string
  driver_id?: string
  scheduled_date?: string
}): Promise<DeliveryView[]> {
  const q = new URLSearchParams()
  if (params?.status) q.set('status', params.status)
  if (params?.driver_id) q.set('driver_id', params.driver_id)
  if (params?.scheduled_date) q.set('scheduled_date', params.scheduled_date)
  const suffix = q.toString() ? `?${q}` : ''
  return http(`/deliveries/dispatch${suffix}`)
}

export async function getDelivery(id: string): Promise<DeliveryView> {
  return http(`/deliveries/${id}`)
}

export async function assignDelivery(
  id: string,
  body: { driver_id: string; vehicle?: string | null; actor_user_id?: string | null; note?: string | null },
): Promise<DeliveryView> {
  return http(`/deliveries/${id}/assign`, body)
}

export async function setDeliveryStatus(
  id: string,
  body: { status: string; actor_user_id?: string | null; note?: string | null },
): Promise<DeliveryView> {
  return http(`/deliveries/${id}/status`, body)
}

export async function getPackingSlip(id: string): Promise<PackingSlipView> {
  return http(`/deliveries/${id}/packing-slip`)
}

export async function submitPod(
  id: string,
  body: {
    signature_b64: string
    photo_ref?: string | null
    photo_sha256?: string | null
    received_by: string
    signed_at?: string | null
    actor_user_id?: string | null
  },
): Promise<DeliveryView> {
  return http(`/deliveries/${id}/pod`, body)
}

// --- Phase 4: EDI -----------------------------------------------------------

export async function sendPurchaseOrder(
  poId: string,
  outDir?: string | null,
): Promise<SendPoResponse> {
  return http(`/edi/purchase-orders/${poId}/send`, { out_dir: outDir ?? null })
}

export async function intakeEdi(body: {
  doc_type: string
  raw_text: string
  vendor_id: string
  related_po_id?: string | null
}): Promise<IntakeResponse> {
  return http('/edi/intake', body)
}

export async function listEdiDocuments(poId: string): Promise<EdiDocument[]> {
  return http(`/edi/documents?po_id=${poId}`)
}

export async function listAcknowledgments(poId: string): Promise<PoAcknowledgmentRow[]> {
  return http(`/edi/acknowledgments?po_id=${poId}`)
}

export async function threeWayMatch(poId: string): Promise<InvoiceMatchReport> {
  return http(`/edi/three-way-match?po_id=${poId}`)
}

export async function listMatchReports(poId: string): Promise<InvoiceMatchReportRow[]> {
  return http(`/edi/match-reports?po_id=${poId}`)
}

export async function getPriceApplication(id: string): Promise<PriceCatalogApplicationRow> {
  return http(`/edi/price-applications/${id}`)
}

export async function markPriceApplicationApplied(
  id: string,
  reviewedBy: string,
): Promise<PriceCatalogApplicationRow> {
  return http(`/edi/price-applications/${id}/mark-applied`, { reviewed_by: reviewedBy })
}

export async function listTransportRecords(vendorId: string): Promise<TransportRecord[]> {
  return http(`/edi/transport?vendor_id=${encodeURIComponent(vendorId)}`)
}

export async function importCatalog(
  csvText: string,
  vendorId: string,
): Promise<CatalogImportResponse> {
  return http('/edi/catalog/import', { csv_text: csvText, vendor_id: vendorId })
}

// --- Phase 6: BI -------------------------------------------------------------

export async function getKpiDashboard(
  storeId: string,
  start: string,
  end: string,
  asOf?: string,
): Promise<KpiDashboard> {
  const q = new URLSearchParams({ store_id: storeId, start, end })
  if (asOf) q.set('as_of', asOf)
  return http(`/bi/kpi?${q}`)
}

export async function getEcoRemittance(start: string, end: string): Promise<EcoRemittanceReport> {
  const q = new URLSearchParams({ start, end })
  return http(`/bi/eco?${q}`)
}

export async function getEcoRemittanceCsv(start: string, end: string): Promise<string> {
  const q = new URLSearchParams({ start, end, format: 'csv' })
  const res = await fetch(`/api/bi/eco?${q}`, { headers: { ...authHeaders() } })
  if (!res.ok) {
    if (res.status === 401) {
      clearSession()
      notifyUnauthorized()
      throw new UnauthorizedError()
    }
    const json = await res.json().catch(() => null)
    throw new Error(typeof json?.error === 'string' ? json.error : `HTTP ${res.status}`)
  }
  return res.text()
}

export async function runInquiry(params: {
  storeId: string
  start: string
  end: string
  customerId?: string
  productId?: string
  docType?: string
}): Promise<InquiryRow[]> {
  const q = new URLSearchParams({ store_id: params.storeId, start: params.start, end: params.end })
  if (params.customerId) q.set('customer_id', params.customerId)
  if (params.productId) q.set('product_id', params.productId)
  if (params.docType) q.set('doc_type', params.docType)
  return http(`/bi/inquiry?${q}`)
}

export async function listEmailOutbox(): Promise<EmailDelivery[]> {
  return http('/bi/email-outbox')
}

// --- Phase 6: Scheduling+ ----------------------------------------------------

export async function listShiftTemplates(): Promise<ShiftTemplate[]> {
  return http('/scheduling/templates')
}

export async function createShiftTemplate(body: {
  name: string
  weekday: number
  starts_at: string
  ends_at: string
  role?: string | null
  staff_id?: string | null
  required_count: number
}): Promise<ShiftTemplate> {
  return http('/scheduling/templates', body)
}

export async function listStaffShifts(date: string): Promise<StaffShift[]> {
  return http(`/scheduling/shifts?date=${date}`)
}

export async function createStaffShift(body: {
  staff_id: string
  shift_date: string
  starts_at: string
  ends_at: string
  template_id?: string | null
  notes?: string | null
}): Promise<StaffShift> {
  return http('/scheduling/shifts', body)
}

export async function setShiftStatus(id: string, status: string): Promise<StaffShift> {
  return http(`/scheduling/shifts/${id}/status`, { status })
}

export async function clockIn(staffId: string): Promise<TimeclockEntry> {
  return http('/scheduling/timeclock/clock-in', { staff_id: staffId })
}

export async function clockOut(staffId: string): Promise<TimeclockEntry> {
  return http('/scheduling/timeclock/clock-out', { staff_id: staffId })
}

export async function listTimeclock(date: string): Promise<TimeclockEntry[]> {
  return http(`/scheduling/timeclock?date=${date}`)
}

export async function getLaborForecast(params: {
  from: string
  days?: number
  salesPerLaborHour?: string
}): Promise<ForecastReport> {
  const q = new URLSearchParams({ from: params.from })
  if (params.days !== undefined) q.set('days', String(params.days))
  if (params.salesPerLaborHour) q.set('sales_per_labor_hour', params.salesPerLaborHour)
  return http(`/scheduling/forecast?${q}`)
}

export async function getLaborCoverage(date: string): Promise<CoverageCheck> {
  return http(`/scheduling/coverage?date=${date}`)
}
