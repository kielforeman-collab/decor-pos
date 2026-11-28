// Back-office HTTP API helpers absorbed into pos-desktop (counts, shelf tags,
// matrix pricing rules, CRM, deliveries/POD, EDI intake/832, reports/BI,
// and Scheduling+). Uses the same Bearer session & 401 handling as api.ts.

import {
  UnauthorizedError,
  authHeaders,
  clearSession,
  notifyUnauthorized,
} from './auth'
import type {
  AdminUserSummary,
  Appointment,
  CatalogImportResponse,
  CountReconcileResponse,
  CoverageCheck,
  CreateAdminUserRequest,
  CreateServiceTokenResponse,
  CrmActivity,
  CrmSavedView,
  DailyUsageReport,
  DeadLetterActionResponse,
  DeadLetterEntry,
  DeliveryView,
  EcoRemittanceReport,
  EmailDelivery,
  ForecastReport,
  InquiryRow,
  IntakeResponse,
  LapsedReport,
  LowStockReport,
  PackingSlipView,
  PhysicalCount,
  PriceCatalogApplicationRow,
  PricingRuleRow,
  RevokeServiceTokenResponse,
  RevokeUserSessionsResponse,
  SavedViewCustomer,
  ServiceTokenSummary,
  ShelfTagResponse,
  ShiftTemplate,
  StaffShift,
  StoreSettingsResponse,
  TimeclockEntry,
  UpdateAdminUserRequest,
  UpdateStoreSettingsRequest,
} from './backofficeTypes'

async function http<T>(path: string, body?: unknown): Promise<T> {
  const res = await fetch(`/api${path}`, {
    method: body === undefined ? 'GET' : 'POST',
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

// --- Physical counts -------------------------------------------------------

export async function startCount(storeId: string): Promise<PhysicalCount> {
  return http('/inventory/counts', { store_id: storeId })
}

export async function listCounts(storeId: string): Promise<PhysicalCount[]> {
  return http(`/inventory/counts?store_id=${encodeURIComponent(storeId)}`)
}

export async function scanCount(
  id: string,
  productId: string,
  countedQty: string,
): Promise<PhysicalCount> {
  return http(`/inventory/counts/${encodeURIComponent(id)}/scan`, {
    product_id: productId,
    counted_qty: countedQty,
  })
}

export async function reconcileCount(id: string): Promise<CountReconcileResponse> {
  return http(`/inventory/counts/${encodeURIComponent(id)}/reconcile`, {})
}

// --- Shelf tags & bulk matrix pricing --------------------------------------

export async function getShelfTag(productId: string): Promise<ShelfTagResponse> {
  return http(`/inventory/shelf-tags?product_id=${encodeURIComponent(productId)}`)
}

export async function listPricingRules(): Promise<PricingRuleRow[]> {
  return http('/pricing/rules')
}

export async function bulkUpdatePricingRules(
  rules: {
    id: string
    calc_method: string
    calc_value: string
    min_margin_floor: string | null
  }[],
): Promise<PricingRuleRow[]> {
  return http('/pricing/rules', { rules })
}

// --- CRM -------------------------------------------------------------------

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
  due_at?: string | null
}): Promise<CrmActivity> {
  return http('/crm/activities', body)
}

export async function completeActivity(id: string): Promise<CrmActivity> {
  return http(`/crm/activities/${encodeURIComponent(id)}/complete`, {})
}

export async function snoozeActivity(id: string, snoozedUntil: string): Promise<CrmActivity> {
  return http(`/crm/activities/${encodeURIComponent(id)}/snooze`, {
    snoozed_until: snoozedUntil,
  })
}

export async function deleteActivity(id: string): Promise<{ id: string; deleted: boolean }> {
  return http(`/crm/activities/${encodeURIComponent(id)}/delete`, {})
}

export async function listDueReminders(asOf?: string): Promise<CrmActivity[]> {
  const suffix = asOf ? `?as_of=${encodeURIComponent(asOf)}` : ''
  return http(`/crm/reminders/due${suffix}`)
}

export async function listOverdueReminders(asOf?: string): Promise<CrmActivity[]> {
  const suffix = asOf ? `?as_of=${encodeURIComponent(asOf)}` : ''
  return http(`/crm/reminders/overdue${suffix}`)
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
  return http(`/crm/appointments/${encodeURIComponent(id)}/cancel`, {})
}

export async function deleteAppointment(id: string): Promise<{ id: string; deleted: boolean }> {
  return http(`/crm/appointments/${encodeURIComponent(id)}/delete`, {})
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
  return http(`/crm/saved-views/${encodeURIComponent(id)}/run`)
}

export async function runSavedViewCustomers(id: string): Promise<SavedViewCustomer[]> {
  return http(`/crm/saved-views/${encodeURIComponent(id)}/run-customers`)
}

export async function deleteSavedView(id: string): Promise<{ id: string; deleted: boolean }> {
  return http(`/crm/saved-views/${encodeURIComponent(id)}/delete`, {})
}

export async function getLapsedReport(params: {
  store_id?: string
  as_of?: string
  inactive_days?: number
  include_never?: boolean
}): Promise<LapsedReport> {
  const q = new URLSearchParams()
  if (params.store_id) q.set('store_id', params.store_id)
  if (params.as_of) q.set('as_of', params.as_of)
  if (params.inactive_days !== undefined) q.set('inactive_days', String(params.inactive_days))
  if (params.include_never !== undefined) q.set('include_never', String(params.include_never))
  const suffix = q.toString() ? `?${q}` : ''
  return http(`/crm/reports/lapsed${suffix}`)
}

// --- Deliveries & POD ------------------------------------------------------

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
  return http(`/deliveries/${encodeURIComponent(id)}`)
}

export async function assignDelivery(
  id: string,
  body: {
    driver_id: string
    vehicle?: string | null
    actor_user_id?: string | null
    note?: string | null
  },
): Promise<DeliveryView> {
  return http(`/deliveries/${encodeURIComponent(id)}/assign`, body)
}

export async function setDeliveryStatus(
  id: string,
  body: { status: string; actor_user_id?: string | null; note?: string | null },
): Promise<DeliveryView> {
  return http(`/deliveries/${encodeURIComponent(id)}/status`, body)
}

export async function getPackingSlip(id: string): Promise<PackingSlipView> {
  return http(`/deliveries/${encodeURIComponent(id)}/packing-slip`)
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
  return http(`/deliveries/${encodeURIComponent(id)}/pod`, body)
}

// --- EDI intake, 832 price review & catalog CSV ----------------------------

export async function intakeEdi(body: {
  doc_type: string
  raw_text: string
  vendor_id: string
  related_po_id?: string | null
}): Promise<IntakeResponse> {
  return http('/edi/intake', body)
}

export async function getPriceApplication(id: string): Promise<PriceCatalogApplicationRow> {
  return http(`/edi/price-applications/${encodeURIComponent(id)}`)
}

export async function markPriceApplicationApplied(
  id: string,
  reviewedBy: string,
): Promise<PriceCatalogApplicationRow> {
  return http(`/edi/price-applications/${encodeURIComponent(id)}/mark-applied`, {
    reviewed_by: reviewedBy,
  })
}

export async function importEdiCatalog(
  csvText: string,
  vendorId: string,
): Promise<CatalogImportResponse> {
  return http('/edi/catalog/import', { csv_text: csvText, vendor_id: vendorId })
}

// --- Reports & BI ----------------------------------------------------------

export async function getDailyUsageReport(date?: string): Promise<DailyUsageReport> {
  const suffix = date ? `?date=${encodeURIComponent(date)}` : ''
  return http(`/pos/reports/daily-usage${suffix}`)
}

export async function getLowStockReport(date?: string): Promise<LowStockReport> {
  const suffix = date ? `?date=${encodeURIComponent(date)}` : ''
  return http(`/pos/reports/low-stock${suffix}`)
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
  const q = new URLSearchParams({
    store_id: params.storeId,
    start: params.start,
    end: params.end,
  })
  if (params.customerId) q.set('customer_id', params.customerId)
  if (params.productId) q.set('product_id', params.productId)
  if (params.docType) q.set('doc_type', params.docType)
  return http(`/bi/inquiry?${q}`)
}

export async function listEmailOutbox(): Promise<EmailDelivery[]> {
  return http('/bi/email-outbox')
}

// --- Scheduling+ -----------------------------------------------------------

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
  return http(`/scheduling/shifts?date=${encodeURIComponent(date)}`)
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
  return http(`/scheduling/shifts/${encodeURIComponent(id)}/status`, { status })
}

export async function clockIn(staffId: string): Promise<TimeclockEntry> {
  return http('/scheduling/timeclock/clock-in', { staff_id: staffId })
}

export async function clockOut(staffId: string): Promise<TimeclockEntry> {
  return http('/scheduling/timeclock/clock-out', { staff_id: staffId })
}

export async function listTimeclock(date: string): Promise<TimeclockEntry[]> {
  return http(`/scheduling/timeclock?date=${encodeURIComponent(date)}`)
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
  return http(`/scheduling/coverage?date=${encodeURIComponent(date)}`)
}

// --- Sync dead-letters & Edge Admin ----------------------------------------

export async function listSyncDeadLetters(limit = 100): Promise<DeadLetterEntry[]> {
  return http(`/pos/sync/dead-letters?limit=${encodeURIComponent(String(limit))}`)
}

export async function requeueSyncDeadLetter(id: string): Promise<DeadLetterActionResponse> {
  return http(`/pos/sync/dead-letters/${encodeURIComponent(id)}/requeue`, {})
}

export async function dismissSyncDeadLetter(
  id: string,
  reason: string,
): Promise<DeadLetterActionResponse> {
  return http(`/pos/sync/dead-letters/${encodeURIComponent(id)}/dismiss`, { reason })
}

export async function getStoreSettings(): Promise<StoreSettingsResponse> {
  return http('/admin/store')
}

export async function updateStoreSettings(
  body: UpdateStoreSettingsRequest,
): Promise<StoreSettingsResponse> {
  return http('/admin/store', body)
}

export async function listAdminUsers(): Promise<AdminUserSummary[]> {
  return http('/admin/users')
}

export async function createAdminUser(
  body: CreateAdminUserRequest,
): Promise<AdminUserSummary> {
  return http('/admin/users', body)
}

export async function updateAdminUser(
  id: string,
  body: UpdateAdminUserRequest,
): Promise<AdminUserSummary> {
  return http(`/admin/users/${encodeURIComponent(id)}`, body)
}

export async function revokeAdminUserSessions(
  id: string,
): Promise<RevokeUserSessionsResponse> {
  return http(`/admin/users/${encodeURIComponent(id)}/revoke-sessions`, {})
}

export async function listAdminServiceTokens(
  storeId?: string,
): Promise<ServiceTokenSummary[]> {
  const q = storeId ? `?store_id=${encodeURIComponent(storeId)}` : ''
  return http(`/admin/service-tokens${q}`)
}

export async function createAdminServiceToken(body: {
  store_id?: string
  label: string
  ttl_days?: number | null
}): Promise<CreateServiceTokenResponse> {
  return http('/admin/service-tokens', body)
}

export async function revokeAdminServiceToken(
  id: string,
): Promise<RevokeServiceTokenResponse> {
  return http(`/admin/service-tokens/${encodeURIComponent(id)}/revoke`, {})
}
