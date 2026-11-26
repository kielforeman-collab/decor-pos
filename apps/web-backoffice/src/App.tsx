import { useCallback, useEffect, useRef, useState } from 'react'
import {
  assignDelivery,
  bulkUpdatePricingRules,
  cancelAppointment,
  clockIn,
  clockOut,
  completeActivity,
  createActivity,
  createAppointment,
  createDelivery,
  createPurchaseOrder,
  createSavedView,
  createShiftTemplate,
  createStaffShift,
  createTransfer,
  deleteActivity,
  deleteAppointment,
  deleteSavedView,
  getArAging,
  getArCollections,
  getArSummary,
  getBalances,
  getEcoRemittance,
  getEcoRemittanceCsv,
  getDelivery,
  getHealth,
  getKpiDashboard,
  getLaborCoverage,
  getLaborForecast,
  getLapsedReport,
  getPackingSlip,
  getPriceApplication,
  getReorderPoints,
  getShelfTag,
  importCatalog,
  importProducts,
  intakeEdi,
  listAcknowledgments,
  listActivities,
  listAppointments,
  listCounts,
  listDispatch,
  listDueReminders,
  listEmailOutbox,
  listEdiDocuments,
  listMatchReports,
  listOverdueReminders,
  listPricingRules,
  listPurchaseOrders,
  listSavedViews,
  listShiftTemplates,
  listStaffShifts,
  listTimeclock,
  listTransfers,
  listTransportRecords,
  markPriceApplicationApplied,
  receivePurchaseOrder,
  receiveTransfer,
  reconcileCount,
  runInquiry,
  runSavedView,
  runSavedViewCustomers,
  scanCount,
  sendPurchaseOrder,
  setDeliveryStatus,
  setShiftStatus,
  setReorderPoint,
  shipTransfer,
  snoozeActivity,
  startCount,
  submitPod,
  suggestReplenishment,
  threeWayMatch,
} from './api'
import { DEMO_CATALOG } from './catalog'
import { parseImportFile, type ImportPreview } from './productImport'
import type {
  AgingView,
  Appointment,
  ArCustomerSummary,
  CatalogImportResponse,
  CollectionsView,
  CoverageCheck,
  CrmActivity,
  CrmSavedView,
  DeliveryView,
  EcoRemittanceReport,
  EmailDelivery,
  EdiDocument,
  IntakeResponse,
  InventoryBalance,
  InquiryRow,
  InvoiceMatchReport,
  ForecastReport,
  KpiDashboard,
  LapsedReport,
  PackingSlipView,
  PhysicalCount,
  PoAcknowledgmentRow,
  PriceCatalogApplicationRow,
  PricingRuleRow,
  ProductImportResponse,
  PurchaseOrderView,
  ReorderPoint,
  SavedViewCustomer,
  SendPoResponse,
  ShelfTagResponse,
  ShiftTemplate,
  StaffShift,
  SuggestedPurchaseOrder,
  TimeclockEntry,
  Transfer,
  TransportRecord,
} from './types'
import { fmtMoney, fmtQty } from './types'
import {
  authHeaders,
  clearSession,
  getSession,
  logout,
  notifyUnauthorized,
  UnauthorizedError,
  type AuthSession,
} from './auth'
import Login from './Login'

// Phase 7 — UI-only role gating. Which tabs each role sees (from
// df-auth RBAC permissions). The backend still enforces every call;
// hiding a tab here only keeps the UI honest, it grants nothing.
const TAB_ROLES: Record<Tab, string[]> = {
  inventory: ['SalesAssociate', 'Warehouse', 'Driver', 'Manager', 'Admin'],
  purchasing: ['Warehouse', 'Manager', 'Admin'],
  transfers: ['Warehouse', 'Manager', 'Admin'],
  counts: ['Warehouse', 'Manager', 'Admin'],
  tags: ['SalesAssociate', 'Warehouse', 'Driver', 'Manager', 'Admin'],
  pricing: ['Manager', 'Admin'],
  ar: ['SalesAssociate', 'Manager', 'Admin'],
  crm: ['Cashier', 'SalesAssociate', 'Manager', 'Admin'],
  delivery: ['Warehouse', 'Driver', 'Manager', 'Admin'],
  bi: ['Manager', 'Admin'],
  scheduling: ['Manager', 'Admin'],
}

function tabVisible(tab: Tab, role: string): boolean {
  const r = role.trim().toLowerCase()
  return TAB_ROLES[tab].some((allowed) => allowed.toLowerCase() === r)
}

type Tab = 'inventory' | 'purchasing' | 'transfers' | 'counts' | 'tags' | 'pricing' | 'ar' | 'crm' | 'delivery' | 'bi' | 'scheduling'

const TABS: { id: Tab; label: string }[] = [
  { id: 'inventory', label: 'Inventory' },
  { id: 'purchasing', label: 'Purchasing' },
  { id: 'transfers', label: 'Transfers' },
  { id: 'counts', label: 'Counts' },
  { id: 'tags', label: 'Shelf Tags' },
  { id: 'pricing', label: 'Pricing' },
  { id: 'ar', label: 'AR' },
  { id: 'crm', label: 'CRM' },
  { id: 'delivery', label: 'Delivery' },
  { id: 'bi', label: 'BI' },
  { id: 'scheduling', label: 'Scheduling' },
]

const CALC_METHODS = ['fixed', 'pct_off_list', 'markup_cost', 'target_gm']

export default function App() {
  // Phase 7: no session → login screen. Restored from localStorage on
  // reload; a 401 from any API call clears it (api.ts) and the
  // 'decor-pos-unauthorized' listener below returns here.
  const [session, setSession] = useState<AuthSession | null>(() => getSession())
  const [storeId, setStoreId] = useState('')
  const [tab, setTab] = useState<Tab>('inventory')
  const [notice, setNotice] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const [balances, setBalances] = useState<InventoryBalance[]>([])
  const [points, setPoints] = useState<ReorderPoint[]>([])
  const [suggested, setSuggested] = useState<SuggestedPurchaseOrder[]>([])
  const [purchaseOrders, setPurchaseOrders] = useState<PurchaseOrderView[]>([])
  const [transfers, setTransfers] = useState<Transfer[]>([])
  const [counts, setCounts] = useState<PhysicalCount[]>([])
  const [tag, setTag] = useState<ShelfTagResponse | null>(null)
  const [rules, setRules] = useState<PricingRuleRow[]>([])
  const [arSummary, setArSummary] = useState<ArCustomerSummary | null>(null)
  const [aging, setAging] = useState<AgingView | null>(null)
  const [collections, setCollections] = useState<CollectionsView | null>(null)

  // Phase 4: CRM.
  const [activities, setActivities] = useState<CrmActivity[]>([])
  const [dueReminders, setDueReminders] = useState<CrmActivity[]>([])
  const [overdueReminders, setOverdueReminders] = useState<CrmActivity[]>([])
  const [appointments, setAppointments] = useState<Appointment[]>([])
  const [savedViews, setSavedViews] = useState<CrmSavedView[]>([])
  const [viewActivities, setViewActivities] = useState<CrmActivity[] | null>(null)
  const [viewCustomers, setViewCustomers] = useState<SavedViewCustomer[] | null>(null)
  const [lapsed, setLapsed] = useState<LapsedReport | null>(null)
  const [lapsedDays, setLapsedDays] = useState('90')

  // Phase 4: delivery.
  const [deliveries, setDeliveries] = useState<DeliveryView[]>([])
  const [selectedDelivery, setSelectedDelivery] = useState<DeliveryView | null>(null)
  const [packingSlip, setPackingSlip] = useState<PackingSlipView | null>(null)

  // Phase 4: EDI (under the Purchasing tab).
  const [ediDocs, setEdiDocs] = useState<EdiDocument[]>([])
  const [ediAcks, setEdiAcks] = useState<PoAcknowledgmentRow[]>([])
  const [matchReport, setMatchReport] = useState<InvoiceMatchReport | null>(null)
  const [priceApp, setPriceApp] = useState<PriceCatalogApplicationRow | null>(null)
  const [transportRecords, setTransportRecords] = useState<TransportRecord[]>([])
  const [intakeResult, setIntakeResult] = useState<IntakeResponse | null>(null)
  const [sendResult, setSendResult] = useState<SendPoResponse | null>(null)
  const [catalogResult, setCatalogResult] = useState<CatalogImportResponse | null>(null)

  // GUI product import (Inventory tab).
  const [importFileName, setImportFileName] = useState('')
  const [importPreview, setImportPreview] = useState<ImportPreview | null>(null)
  const [importResult, setImportResult] = useState<ProductImportResponse | null>(null)
  const importInputRef = useRef<HTMLInputElement | null>(null)

  // Forms.
  const [pointForm, setPointForm] = useState({ product_id: '', min_qty: '10', max_qty: '20', reorder_qty: '12' })
  const [toStoreId, setToStoreId] = useState('')
  const [transferLines, setTransferLines] = useState<{ product_id: string; qty: string }[]>([])
  const [transferProduct, setTransferProduct] = useState('')
  const [transferQty, setTransferQty] = useState('1')
  const [countProduct, setCountProduct] = useState('')
  const [countQty, setCountQty] = useState('')
  const [selectedCountId, setSelectedCountId] = useState('')
  const [tagProduct, setTagProduct] = useState('')
  const [customerId, setCustomerId] = useState('')
  const [receiveQty, setReceiveQty] = useState<Record<string, string>>({})

  // Phase 4 forms.
  const [activityForm, setActivityForm] = useState({ customer_id: '', activity_type: 'follow_up', subject: '', notes: '', due_at: '' })
  const [snoozeUntil, setSnoozeUntil] = useState('')
  const [apptForm, setApptForm] = useState({ customer_id: '', staff_id: '', title: '', starts_at: '', ends_at: '' })
  const [apptDate, setApptDate] = useState('')
  const [viewForm, setViewForm] = useState({ name: '', filter_json: '{"activity_type": "follow_up", "completed": false}' })
  const [selectedViewId, setSelectedViewId] = useState('')
  const [deliveryForm, setDeliveryForm] = useState({ transaction_id: '', customer_id: '', address_json: '{"street": "", "city": "", "postal_code": ""}', scheduled_date: '', priority: '0', vehicle: '' })
  const [dispatchStatus, setDispatchStatus] = useState('')
  const [assignForm, setAssignForm] = useState({ driver_id: '', vehicle: '' })
  const [podForm, setPodForm] = useState({ signature_b64: '', photo_ref: '', photo_sha256: '', received_by: '' })
  const [ediPoId, setEdiPoId] = useState('')
  const [ediOutDir, setEdiOutDir] = useState('')
  const [ediVendorId, setEdiVendorId] = useState('')
  const [ediDocType, setEdiDocType] = useState('856')
  const [ediRawText, setEdiRawText] = useState('')
  const [ediCsvText, setEdiCsvText] = useState('')
  const [priceAppId, setPriceAppId] = useState('')
  const [reviewedBy, setReviewedBy] = useState('')

  const say = (msg: string) => {
    setNotice(msg)
    setError(null)
  }
  const fail = (e: unknown) => {
    setError(e instanceof Error ? e.message : String(e))
    setNotice(null)
  }

  async function doLogout() {
    await logout()
    setSession(null)
    setStoreId('')
    setNotice(null)
    setError(null)
  }

  function handleLoggedIn(next: AuthSession) {
    setSession(next)
    setNotice(null)
    setError(null)
  }

  const refreshInventory = useCallback(async () => {
    if (!storeId) return
    const [b, p] = await Promise.all([getBalances(storeId), getReorderPoints(storeId)])
    setBalances(b)
    setPoints(p)
    if (b.length > 0) {
      if (!pointForm.product_id) setPointForm((f) => ({ ...f, product_id: b[0].product_id }))
      if (!transferProduct) setTransferProduct(b[0].product_id)
      if (!countProduct) setCountProduct(b[0].product_id)
      if (!tagProduct) setTagProduct(b[0].product_id)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [storeId])

  const refreshPurchasing = useCallback(async () => {
    if (!storeId) return
    setPurchaseOrders(await listPurchaseOrders(storeId))
  }, [storeId])

  const refreshTransfers = useCallback(async () => {
    if (!storeId) return
    setTransfers(await listTransfers(storeId))
  }, [storeId])

  const refreshCounts = useCallback(async () => {
    if (!storeId) return
    setCounts(await listCounts(storeId))
  }, [storeId])

  const refreshPricing = useCallback(async () => {
    setRules(await listPricingRules())
  }, [])

  const refreshAr = useCallback(async () => {
    if (!storeId) return
    const [a, c] = await Promise.all([getArAging(storeId), getArCollections(storeId)])
    setAging(a)
    setCollections(c)
  }, [storeId])

  const refreshCrm = useCallback(async () => {
    const [a, d, o, ap, v] = await Promise.all([
      listActivities(),
      listDueReminders(),
      listOverdueReminders(),
      listAppointments(),
      listSavedViews(),
    ])
    setActivities(a)
    setDueReminders(d)
    setOverdueReminders(o)
    setAppointments(ap)
    setSavedViews(v)
  }, [])

  const refreshDelivery = useCallback(async () => {
    setDeliveries(await listDispatch())
  }, [])

  // Phase 6: BI.
  const [kpi, setKpi] = useState<KpiDashboard | null>(null)
  const [eco, setEco] = useState<EcoRemittanceReport | null>(null)
  const [inquiryRows, setInquiryRows] = useState<InquiryRow[] | null>(null)
  const [outbox, setOutbox] = useState<EmailDelivery[] | null>(null)
  const [biStart, setBiStart] = useState('')
  const [biEnd, setBiEnd] = useState('')
  const [inquiryDocType, setInquiryDocType] = useState('')
  const [inquiryProductId, setInquiryProductId] = useState('')

  // Phase 6: Scheduling+.
  const [templates, setTemplates] = useState<ShiftTemplate[]>([])
  const [shifts, setShifts] = useState<StaffShift[]>([])
  const [timeclock, setTimeclock] = useState<TimeclockEntry[]>([])
  const [forecast, setForecast] = useState<ForecastReport | null>(null)
  const [forecastNote, setForecastNote] = useState<string | null>(null)
  const [coverage, setCoverage] = useState<CoverageCheck | null>(null)
  const [schedDate, setSchedDate] = useState('')
  const [templateForm, setTemplateForm] = useState({ name: '', weekday: '1', starts_at: '08:00', ends_at: '16:00', role: '', staff_id: '', required_count: '1' })
  const [shiftForm, setShiftForm] = useState({ staff_id: '', starts_at: '', ends_at: '', template_id: '', notes: '' })
  const [clockStaffId, setClockStaffId] = useState('')
  const [forecastDays, setForecastDays] = useState('7')
  const [forecastRatio, setForecastRatio] = useState('')

  const refreshScheduling = useCallback(async () => {
    const today = new Date().toISOString().slice(0, 10)
    const day = schedDate.trim() || today
    const [t, s, c] = await Promise.all([
      listShiftTemplates(),
      listStaffShifts(day),
      listTimeclock(day),
    ])
    setTemplates(t)
    setShifts(s)
    setTimeclock(c)
    setCoverage(await getLaborCoverage(day))
    // The forecast needs 28 days of sales + timeclock history;
    // a fresh store has none, so a failure here is an inline
    // note, not a global error banner.
    try {
      const ratio = forecastRatio.trim() || undefined
      setForecast(await getLaborForecast({ from: day, days: Number(forecastDays) || 7, salesPerLaborHour: ratio }))
      setForecastNote(null)
    } catch (e) {
      setForecast(null)
      setForecastNote(e instanceof Error ? e.message : String(e))
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [schedDate, forecastDays, forecastRatio])

  const refreshBi = useCallback(async () => {
    if (!storeId) return
    const today = new Date().toISOString().slice(0, 10)
    const start = biStart.trim() || today
    const end = biEnd.trim() || today
    const [k, e] = await Promise.all([
      getKpiDashboard(storeId, start, end),
      getEcoRemittance(start, end),
    ])
    setKpi(k)
    setEco(e)
    // The outbox is unconfigured until the edge is started with
    // --email-outbox; a 404 there means "not configured", not
    // "no deliveries", so show it as an empty list with a note.
    try {
      setOutbox(await listEmailOutbox())
    } catch {
      setOutbox(null)
    }
  }, [storeId, biStart, biEnd])

  const downloadEcoCsv = useCallback(async () => {
    const today = new Date().toISOString().slice(0, 10)
    const start = biStart.trim() || today
    const end = biEnd.trim() || today
    const csv = await getEcoRemittanceCsv(start, end)
    const blob = new Blob([csv], { type: 'text/csv' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = `eco-remittance-${start}-to-${end}.csv`
    a.click()
    URL.revokeObjectURL(url)
  }, [biStart, biEnd])

  const refreshEdiForPo = useCallback(async (poId: string) => {
    if (!poId) return
    const [docs, acks] = await Promise.all([listEdiDocuments(poId), listAcknowledgments(poId)])
    setEdiDocs(docs)
    setEdiAcks(acks)
  }, [])

  useEffect(() => {
    if (!session) return
    getHealth()
      .then((health) => setStoreId(health.store_id))
      .catch(fail)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [session])

  useEffect(() => {
    const onUnauthorized = () => {
      setSession(null)
      setStoreId('')
      setError('Session expired — sign in again.')
    }
    window.addEventListener('decor-pos-unauthorized', onUnauthorized)
    return () => window.removeEventListener('decor-pos-unauthorized', onUnauthorized)
  }, [])

  useEffect(() => {
    if (!storeId) return
    refreshInventory().catch(fail)
    refreshPurchasing().catch(fail)
    refreshTransfers().catch(fail)
    refreshCounts().catch(fail)
    refreshPricing().catch(fail)
    refreshAr().catch(fail)
    refreshCrm().catch(fail)
    refreshDelivery().catch(fail)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [storeId])

  const productName = (productId: string) => {
    const row = balances.find((b) => b.product_id === productId)
    if (row) return `${row.name} (${row.sku})`
    const demo = DEMO_CATALOG.find((p) => p.product_id === productId)
    return demo ? `${demo.description} (${demo.sku})` : productId.slice(0, 8)
  }

  const productOptions = balances.length > 0
    ? balances.map((b) => ({ id: b.product_id, label: `${b.name} (${b.sku})` }))
    : DEMO_CATALOG.map((p) => ({ id: p.product_id, label: `${p.description} (${p.sku})` }))

  async function doSetPoint() {
    if (!pointForm.product_id) return
    setBusy(true)
    try {
      await setReorderPoint({ ...pointForm, store_id: storeId })
      say(`Reorder point saved for ${productName(pointForm.product_id)}.`)
      await refreshInventory()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  // GUI product import: parse the picked spreadsheet in the browser
  // and preview it. Nothing is sent until Confirm.
  async function handleImportFilePicked(file: File) {
    setImportResult(null)
    setImportPreview(null)
    try {
      const preview = await parseImportFile(file)
      setImportFileName(file.name)
      setImportPreview(preview)
      if (preview.error) {
        fail(new Error(preview.error))
        return
      }
      const skipped = preview.rows.filter((r) => r.problem).length
      say(`${file.name}: ${preview.rows.length} row(s) parsed, ${preview.rows.length - skipped} ready to import, ${skipped} will be skipped. Review below, then confirm.`)
    } catch (e) {
      fail(e)
    }
  }

  async function doConfirmImport() {
    if (!importPreview) return
    const rows = importPreview.rows.filter((r) => !r.problem).map((r) => r.data)
    if (rows.length === 0) {
      fail(new Error('No valid rows to import.'))
      return
    }
    setBusy(true)
    try {
      const res = await importProducts({ store_id: storeId || undefined, rows })
      setImportResult(res)
      say(`Import done: ${res.created} created, ${res.updated} updated, ${res.failed} failed.`)
      await refreshInventory()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doSuggest() {
    setBusy(true)
    try {
      const pos = await suggestReplenishment({ store_id: storeId, days: 30, lead_time_days: 7 })
      setSuggested(pos)
      say(pos.length === 0 ? 'Nothing below minimum — no replenishment suggested.' : `${pos.length} suggested purchase order(s) ready.`)
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doCreatePo(po: SuggestedPurchaseOrder) {
    setBusy(true)
    try {
      const created = await createPurchaseOrder({
        vendor_id: po.vendor_id,
        store_id: storeId,
        lines: po.lines.map((l) => ({ product_id: l.product_id, qty: l.qty })),
      })
      say(`PO ${created.po_number} created (${created.lines.length} line(s), ${fmtMoney({ amount: created.total, currency: 'Usd' })}).`)
      setSuggested((prev) => prev.filter((p) => p.po_number !== po.po_number))
      await refreshPurchasing()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doReceivePo(po: PurchaseOrderView) {
    const lines = po.lines
      .map((l) => ({ product_id: l.product_id, qty: receiveQty[l.id] ?? '' }))
      .filter((l) => l.qty.trim() !== '' && Number(l.qty) > 0)
    if (lines.length === 0) {
      setError('Enter a received quantity on at least one line.')
      return
    }
    setBusy(true)
    try {
      const updated = await receivePurchaseOrder(po.id, lines)
      say(`PO ${updated.po_number} is now ${updated.status}.`)
      setReceiveQty({})
      await refreshPurchasing()
      await refreshInventory()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  // --- Phase 4: CRM ---------------------------------------------------------

  async function doCreateActivity() {
    if (!activityForm.subject.trim()) {
      setError('An activity subject is required.')
      return
    }
    setBusy(true)
    try {
      await createActivity({
        customer_id: activityForm.customer_id.trim() || null,
        activity_type: activityForm.activity_type,
        subject: activityForm.subject.trim(),
        notes: activityForm.notes.trim() || null,
        due_at: activityForm.due_at.trim() || null,
      })
      say('Activity logged.')
      setActivityForm({ customer_id: '', activity_type: 'follow_up', subject: '', notes: '', due_at: '' })
      await refreshCrm()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doCompleteActivity(id: string) {
    setBusy(true)
    try {
      await completeActivity(id)
      say('Reminder marked complete.')
      await refreshCrm()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doSnoozeActivity(id: string) {
    if (!snoozeUntil.trim()) {
      setError('Enter a snooze target (RFC 3339) first.')
      return
    }
    setBusy(true)
    try {
      await snoozeActivity(id, snoozeUntil.trim())
      say('Reminder snoozed.')
      await refreshCrm()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doDeleteActivity(id: string) {
    setBusy(true)
    try {
      await deleteActivity(id)
      say('Activity deleted.')
      await refreshCrm()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doCreateAppointment() {
    if (!apptForm.title.trim() || !apptForm.starts_at.trim() || !apptForm.ends_at.trim()) {
      setError('Title, start and end are required.')
      return
    }
    setBusy(true)
    try {
      await createAppointment({
        customer_id: apptForm.customer_id.trim() || null,
        staff_id: apptForm.staff_id.trim() || null,
        title: apptForm.title.trim(),
        starts_at: apptForm.starts_at.trim(),
        ends_at: apptForm.ends_at.trim(),
      })
      say('Appointment booked.')
      setApptForm({ customer_id: '', staff_id: '', title: '', starts_at: '', ends_at: '' })
      await refreshCrm()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doCancelAppointment(id: string) {
    setBusy(true)
    try {
      await cancelAppointment(id)
      say('Appointment cancelled.')
      await refreshCrm()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doCreateSavedView() {
    let filter: unknown
    try {
      filter = JSON.parse(viewForm.filter_json)
    } catch {
      setError('filter_json is not valid JSON.')
      return
    }
    if (!viewForm.name.trim() || typeof filter !== 'object' || filter === null) {
      setError('A view name and a JSON object filter are required.')
      return
    }
    setBusy(true)
    try {
      await createSavedView({ name: viewForm.name.trim(), filter_json: filter })
      say(`Saved view “${viewForm.name.trim()}” created.`)
      setViewForm({ name: '', filter_json: '{"activity_type": "follow_up", "completed": false}' })
      await refreshCrm()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doRunSavedView() {
    if (!selectedViewId) return
    setBusy(true)
    try {
      const [a, c] = await Promise.all([
        runSavedView(selectedViewId).catch(() => null),
        runSavedViewCustomers(selectedViewId).catch(() => null),
      ])
      setViewActivities(a)
      setViewCustomers(c)
      say(
        a !== null
          ? `${a.length} activit(ies) match this view (activity dialect).`
          : 'This view is a customer-segment view — see the customer results below.',
      )
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doLapsedReport() {
    if (!storeId) return
    setBusy(true)
    try {
      const report = await getLapsedReport({
        store_id: storeId,
        inactive_days: Number(lapsedDays) || 90,
        include_never: true,
      })
      setLapsed(report)
      say(`${report.customers.length} contractor(s) on the lapsed report.`)
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  // --- Phase 4: delivery ----------------------------------------------------

  async function doCreateDelivery() {
    let address: unknown
    try {
      address = JSON.parse(deliveryForm.address_json)
    } catch {
      setError('address_json is not valid JSON.')
      return
    }
    if (typeof address !== 'object' || address === null) {
      setError('address_json must be a JSON object.')
      return
    }
    setBusy(true)
    try {
      const created = await createDelivery({
        transaction_id: deliveryForm.transaction_id.trim() || null,
        customer_id: deliveryForm.customer_id.trim() || null,
        address_json: address,
        scheduled_date: deliveryForm.scheduled_date.trim() || null,
        priority: Number(deliveryForm.priority) || 0,
        vehicle: deliveryForm.vehicle.trim() || null,
      })
      say(`Delivery ${created.id.slice(0, 8)} created (draft).`)
      setDeliveryForm({ transaction_id: '', customer_id: '', address_json: '{"street": "", "city": "", "postal_code": ""}', scheduled_date: '', priority: '0', vehicle: '' })
      await refreshDelivery()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doAssignDelivery(id: string) {
    if (!assignForm.driver_id.trim()) {
      setError('A driver id is required to assign.')
      return
    }
    setBusy(true)
    try {
      const updated = await assignDelivery(id, {
        driver_id: assignForm.driver_id.trim(),
        vehicle: assignForm.vehicle.trim() || null,
      })
      say(`Delivery assigned to ${updated.driver_id?.slice(0, 8)}.`)
      setSelectedDelivery(updated)
      await refreshDelivery()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doSetDeliveryStatus(id: string, status: string) {
    setBusy(true)
    try {
      const updated = await setDeliveryStatus(id, { status })
      say(`Delivery is now ${updated.status}.`)
      setSelectedDelivery(updated)
      await refreshDelivery()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doShowPackingSlip(id: string) {
    setBusy(true)
    try {
      setPackingSlip(await getPackingSlip(id))
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doSubmitPod(id: string) {
    if (!podForm.signature_b64.trim() || !podForm.received_by.trim()) {
      setError('A base64 signature and a receiver name are required — and a photo reference, or close-out is refused.')
      return
    }
    setBusy(true)
    try {
      const updated = await submitPod(id, {
        signature_b64: podForm.signature_b64.trim(),
        photo_ref: podForm.photo_ref.trim() || null,
        photo_sha256: podForm.photo_sha256.trim() || null,
        received_by: podForm.received_by.trim(),
      })
      say(`Delivery ${updated.id.slice(0, 8)} closed out as delivered.`)
      setPodForm({ signature_b64: '', photo_ref: '', photo_sha256: '', received_by: '' })
      setSelectedDelivery(updated)
      await refreshDelivery()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  // --- Phase 4: EDI --------------------------------------------------------

  async function doSendPo(poId: string) {
    setBusy(true)
    try {
      const res = await sendPurchaseOrder(poId, ediOutDir.trim() || null)
      setSendResult(res)
      say(`PO ${res.po_number} sent as X12 850${res.written_to ? ` — staged at ${res.written_to}` : ' (no file staged)'}.`)
      await refreshEdiForPo(poId)
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doIntakeEdi() {
    if (!ediVendorId.trim() || !ediRawText.trim()) {
      setError('A vendor id and document text are required.')
      return
    }
    setBusy(true)
    try {
      const res = await intakeEdi({
        doc_type: ediDocType,
        raw_text: ediRawText,
        vendor_id: ediVendorId.trim(),
        related_po_id: ediPoId.trim() || null,
      })
      setIntakeResult(res)
      say(`Intake stored (${res.doc_type}): ${res.note}`)
      if (ediPoId.trim()) await refreshEdiForPo(ediPoId.trim())
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doThreeWayMatch(poId: string) {
    setBusy(true)
    try {
      const report = await threeWayMatch(poId)
      setMatchReport(report)
      say(`3-way match for ${report.po_number}: ${report.status}.`)
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doLoadPriceApp() {
    if (!priceAppId.trim()) return
    setBusy(true)
    try {
      setPriceApp(await getPriceApplication(priceAppId.trim()))
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doMarkPriceAppApplied() {
    if (!priceApp || !reviewedBy.trim()) {
      setError('Load a price application and enter a reviewer name first.')
      return
    }
    setBusy(true)
    try {
      const updated = await markPriceApplicationApplied(priceApp.id, reviewedBy.trim())
      setPriceApp(updated)
      say(`Price application ${updated.id.slice(0, 8)} marked applied (review decision recorded — no prices were rewritten).`)
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doLoadTransport() {
    if (!ediVendorId.trim()) {
      setError('Enter a vendor id first.')
      return
    }
    setBusy(true)
    try {
      setTransportRecords(await listTransportRecords(ediVendorId.trim()))
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doImportCatalog() {
    if (!ediVendorId.trim() || !ediCsvText.trim()) {
      setError('A vendor id and CSV text are required.')
      return
    }
    setBusy(true)
    try {
      const res = await importCatalog(ediCsvText, ediVendorId.trim())
      setCatalogResult(res)
      say(`${res.imported} catalog item(s) imported, ${res.errors.length} row error(s).`)
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doCreateTransfer() {
    if (!toStoreId.trim() || transferLines.length === 0) {
      setError('A destination store id and at least one line are required.')
      return
    }
    setBusy(true)
    try {
      const transfer = await createTransfer({
        from_store_id: storeId,
        to_store_id: toStoreId.trim(),
        lines: transferLines.map((l) => ({
          ...l,
          unit_cost: balances.find((b) => b.product_id === l.product_id)?.standard_cost ?? '0.0000',
        })),
      })
      say(`Transfer ${transfer.id.slice(0, 8)} drafted.`)
      setTransferLines([])
      await refreshTransfers()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doShipTransfer(id: string) {
    setBusy(true)
    try {
      await shipTransfer(id)
      say('Transfer shipped — stock is in transit.')
      await refreshTransfers()
      await refreshInventory()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doReceiveTransfer(id: string) {
    setBusy(true)
    try {
      await receiveTransfer(id)
      say('Transfer received — destination stock updated.')
      await refreshTransfers()
      await refreshInventory()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doStartCount() {
    setBusy(true)
    try {
      const count = await startCount(storeId)
      setSelectedCountId(count.id)
      say(`Count ${count.id.slice(0, 8)} started.`)
      await refreshCounts()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doScanCount() {
    if (!selectedCountId || !countProduct || !countQty) return
    setBusy(true)
    try {
      await scanCount(selectedCountId, countProduct, countQty)
      setCountQty('')
      say(`Counted ${countQty} of ${productName(countProduct)}.`)
      await refreshCounts()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doReconcileCount() {
    if (!selectedCountId) return
    setBusy(true)
    try {
      const report = await reconcileCount(selectedCountId)
      const parts = report.adjustments.map(
        (a) => `${productName(a.product_id)}: ${fmtQty(a.adjustment)}`,
      )
      say(`Count reconciled — adjustments: ${parts.length > 0 ? parts.join(', ') : 'none'}.`)
      await refreshCounts()
      await refreshInventory()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doShelfTag() {
    if (!tagProduct) return
    setBusy(true)
    try {
      setTag(await getShelfTag(tagProduct))
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doSavePricing() {
    setBusy(true)
    try {
      await bulkUpdatePricingRules(
        rules.map((r) => ({
          id: r.id,
          calc_method: r.calc_method,
          calc_value: r.calc_value,
          min_margin_floor: r.min_margin_floor,
        })),
      )
      say('Pricing rules saved.')
      await refreshPricing()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doArSummary() {
    if (!customerId.trim()) return
    setBusy(true)
    try {
      setArSummary(await getArSummary(customerId.trim()))
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  // Phase 6: Scheduling+.
  async function doCreateTemplate() {
    if (!templateForm.name.trim()) return
    setBusy(true)
    try {
      await createShiftTemplate({
        name: templateForm.name.trim(),
        weekday: Number(templateForm.weekday),
        starts_at: templateForm.starts_at,
        ends_at: templateForm.ends_at,
        role: templateForm.role.trim() || null,
        staff_id: templateForm.staff_id.trim() || null,
        required_count: Number(templateForm.required_count) || 1,
      })
      say('Shift template created.')
      await refreshScheduling()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doCreateShift() {
    if (!shiftForm.staff_id.trim() || !shiftForm.starts_at || !shiftForm.ends_at) return
    setBusy(true)
    try {
      const day = schedDate.trim() || new Date().toISOString().slice(0, 10)
      await createStaffShift({
        staff_id: shiftForm.staff_id.trim(),
        shift_date: day,
        starts_at: new Date(shiftForm.starts_at).toISOString(),
        ends_at: new Date(shiftForm.ends_at).toISOString(),
        template_id: shiftForm.template_id.trim() || null,
        notes: shiftForm.notes.trim() || null,
      })
      say('Shift created.')
      await refreshScheduling()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doShiftStatus(id: string, status: string) {
    setBusy(true)
    try {
      await setShiftStatus(id, status)
      say(`Shift ${status}.`)
      await refreshScheduling()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doClockIn() {
    if (!clockStaffId.trim()) return
    setBusy(true)
    try {
      await clockIn(clockStaffId.trim())
      say('Clocked in.')
      await refreshScheduling()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doClockOut() {
    if (!clockStaffId.trim()) return
    setBusy(true)
    try {
      await clockOut(clockStaffId.trim())
      say('Clocked out.')
      await refreshScheduling()
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  const selectedCount = counts.find((c) => c.id === selectedCountId) ?? null

  // Phase 7: login screen replaces the whole back office until a
  // session exists. Role-gated tabs are computed below (UI-only).
  if (!session) {
    return (
      <div className="app">
        <header>
          <h1>Decor POS — Back Office</h1>
        </header>
        <main className="login-main">
          <Login
            title="Sign in — Back Office"
            subtitle="Sign in with your email + password. Tabs you cannot use are hidden for your role."
            allowPin={false}
            onLoggedIn={handleLoggedIn}
          />
        </main>
      </div>
    )
  }

  // Phase 7: UI-only role gating — tabs the role cannot use are hidden
  // (the backend still enforces RBAC on every call). If the current tab
  // is not visible for this role, fall back to the first visible tab.
  const visibleTabs = TABS.filter((t) => tabVisible(t.id, session.user.role))
  const effectiveTab: Tab | null = visibleTabs.some((t) => t.id === tab)
    ? tab
    : (visibleTabs[0]?.id ?? null)

  // GUI product import is Manager / Admin only (UI-only — the
  // backend enforces it with the PricingManage gate).
  const canImportProducts =
    session !== null && ['manager', 'admin'].includes(session.user.role.toLowerCase())

  return (
    <div className="app">
      <header>
        <h1>Decor POS — Back Office</h1>
        <div className="status">
          <span>
            {session.user.display_name ?? session.user.email} ({session.user.role})
          </span>
          <span>Store: {storeId ? storeId.slice(0, 8) : '…'}</span>
          <span>Edge: 127.0.0.1:8080 (dev proxy)</span>
          <button type="button" onClick={() => void doLogout()}>
            Log out
          </button>
        </div>
      </header>

      <nav className="tabs">
        {visibleTabs.map((t) => (
          <button
            key={t.id}
            type="button"
            className={effectiveTab === t.id ? 'active' : ''}
            onClick={() => setTab(t.id)}
          >
            {t.label}
          </button>
        ))}
      </nav>

      {visibleTabs.length === 0 && (
        <p className="hint" role="alert">
          Your role ({session.user.role}) has no back-office tabs. Ask a
          manager or admin for access — this is a UI gate only; the
          backend enforces roles on every call.
        </p>
      )}

      {error && <div className="error" role="alert">{error}</div>}
      {notice && <div className="notice">{notice}</div>}

      <main>
        {effectiveTab === 'inventory' && (
          <section>
            <h2>Inventory — {balances.length} product(s)</h2>
            <button type="button" onClick={() => void refreshInventory().catch(fail)}>Refresh</button>

            <h3>Import products</h3>
            {canImportProducts ? (
              <>
                <div className="entry">
                  <button type="button" onClick={() => importInputRef.current?.click()} disabled={busy}>
                    Import products…
                  </button>
                  <input
                    ref={importInputRef}
                    type="file"
                    accept=".xlsx,.csv"
                    style={{ display: 'none' }}
                    onChange={(e) => {
                      const f = e.target.files?.[0]
                      if (f) void handleImportFilePicked(f)
                      e.target.value = ''
                    }}
                  />
                  {importFileName && <span>{importFileName}</span>}
                </div>
                <p className="hint">
                  Reads a Benjamin Moore price list (.xlsx or .csv) in the browser — the file itself is never
                  uploaded. Products match by SKU: new SKUs are created, existing SKUs get new prices. Category,
                  UPC, MSRP and MAP are not stored (no columns for them).
                </p>
                {importPreview && !importPreview.error && (
                  <>
                    <p>
                      {importPreview.rows.length} row(s) parsed —{' '}
                      {importPreview.rows.filter((r) => !r.problem).length} ready,{' '}
                      {importPreview.rows.filter((r) => r.problem).length} will be skipped.
                    </p>
                    <table>
                      <thead><tr><th>SKU</th><th>Product</th><th>List price</th><th>Net price</th></tr></thead>
                      <tbody>
                        {importPreview.rows.filter((r) => !r.problem).slice(0, 5).map((r) => (
                          <tr key={r.data.row}>
                            <td>{r.data.sku}</td>
                            <td>{r.data.name}</td>
                            <td>{r.data.list_price}</td>
                            <td>{r.data.net_price}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                    {importPreview.rows.some((r) => r.problem) && (
                      <>
                        <h4>Rows that will be skipped</h4>
                        <table>
                          <thead><tr><th>Row</th><th>SKU</th><th>Reason</th></tr></thead>
                          <tbody>
                            {importPreview.rows.filter((r) => r.problem).slice(0, 20).map((r) => (
                              <tr key={r.data.row}>
                                <td>{r.data.row}</td>
                                <td>{r.data.sku || '—'}</td>
                                <td>{r.problem}</td>
                              </tr>
                            ))}
                          </tbody>
                        </table>
                        {importPreview.rows.filter((r) => r.problem).length > 20 && (
                          <p className="hint">
                            …and {importPreview.rows.filter((r) => r.problem).length - 20} more skipped row(s).
                          </p>
                        )}
                      </>
                    )}
                    <button
                      type="button"
                      onClick={() => void doConfirmImport()}
                      disabled={busy || importPreview.rows.every((r) => r.problem)}
                    >
                      Confirm import ({importPreview.rows.filter((r) => !r.problem).length} rows)
                    </button>
                  </>
                )}
                {importResult && (
                  <>
                    <p>
                      Import done: {importResult.created} created, {importResult.updated} updated,{' '}
                      {importResult.failed} failed.
                    </p>
                    {importResult.failures.length > 0 && (
                      <table>
                        <thead><tr><th>Row</th><th>SKU</th><th>Reason</th></tr></thead>
                        <tbody>
                          {importResult.failures.map((f) => (
                            <tr key={f.row}>
                              <td>{f.row}</td>
                              <td>{f.sku || '—'}</td>
                              <td>{f.reason}</td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    )}
                  </>
                )}
              </>
            ) : (
              <p className="hint">Product import needs a Manager or Admin login.</p>
            )}
            <table>
              <thead>
                <tr><th>SKU</th><th>Product</th><th>On hand</th><th>Committed</th><th>On order</th><th>In transit</th><th>Min / Max</th></tr>
              </thead>
              <tbody>
                {balances.map((b) => {
                  const point = points.find((p) => p.product_id === b.product_id)
                  return (
                    <tr key={b.product_id}>
                      <td>{b.sku}</td>
                      <td>{b.name} <small>{b.base_uom}</small></td>
                      <td>{fmtQty(b.on_hand)}</td>
                      <td>{fmtQty(b.committed)}</td>
                      <td>{fmtQty(b.on_order)}</td>
                      <td>{fmtQty(b.in_transit)}</td>
                      <td>{point ? `${fmtQty(point.min_qty)} / ${fmtQty(point.max_qty)}` : '—'}</td>
                    </tr>
                  )
                })}
                {balances.length === 0 && (
                  <tr><td colSpan={7}>No products in the edge database yet — seed the demo catalog (<code>sqlite3 edge.db &lt; apps/pos-desktop/seed-demo.sql</code>).</td></tr>
                )}
              </tbody>
            </table>

            <h3>Min / max reorder point</h3>
            <div className="entry">
              <select value={pointForm.product_id} onChange={(e) => setPointForm({ ...pointForm, product_id: e.target.value })}>
                {productOptions.map((p) => (
                  <option key={p.id} value={p.id}>{p.label}</option>
                ))}
              </select>
              <input value={pointForm.min_qty} onChange={(e) => setPointForm({ ...pointForm, min_qty: e.target.value })} placeholder="Min" inputMode="decimal" />
              <input value={pointForm.max_qty} onChange={(e) => setPointForm({ ...pointForm, max_qty: e.target.value })} placeholder="Max" inputMode="decimal" />
              <input value={pointForm.reorder_qty} onChange={(e) => setPointForm({ ...pointForm, reorder_qty: e.target.value })} placeholder="Reorder qty" inputMode="decimal" />
              <button type="button" onClick={() => void doSetPoint()} disabled={busy}>Save point</button>
            </div>
            <p className="hint">Reorder points persist in the edge database (<code>reorder_points</code>, migration 0005).</p>
          </section>
        )}

        {effectiveTab === 'purchasing' && (
          <section>
            <h2>Purchasing</h2>
            <div className="actions">
              <button type="button" onClick={() => void doSuggest()} disabled={busy}>Suggest replenishment</button>
              <button type="button" onClick={() => void refreshPurchasing().catch(fail)}>Refresh POs</button>
            </div>
            <p className="hint">Suggestions use min/max + trailing-30-day velocity (lead time 7 days); nothing is ordered until a PO is created.</p>
            {suggested.map((po) => (
              <div key={po.po_number} className="card">
                <h3>{po.po_number} — vendor {po.vendor_id}</h3>
                <table>
                  <thead><tr><th>SKU</th><th>Qty</th><th>On hand</th><th>Velocity/day</th><th>Unit cost</th></tr></thead>
                  <tbody>
                    {po.lines.map((l) => (
                      <tr key={l.product_id}>
                        <td>{l.sku}</td>
                        <td>{fmtQty(l.qty)}</td>
                        <td>{fmtQty(l.on_hand)}</td>
                        <td>{fmtQty(l.velocity_per_day)}</td>
                        <td>{fmtMoney({ amount: l.unit_cost, currency: 'Usd' })}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                <button type="button" onClick={() => void doCreatePo(po)} disabled={busy}>Create PO</button>
              </div>
            ))}
            <h3>Purchase orders</h3>
            {purchaseOrders.map((po) => (
              <div key={po.id} className="card">
                <h3>{po.po_number} — {po.vendor_id} — {po.status} — {fmtMoney({ amount: po.total, currency: 'Usd' })}</h3>
                <table>
                  <thead><tr><th>SKU</th><th>Ordered</th><th>Received</th><th>Receive qty</th></tr></thead>
                  <tbody>
                    {po.lines.map((l) => (
                      <tr key={l.id}>
                        <td>{l.sku} <small>{l.description}</small></td>
                        <td>{fmtQty(l.qty)}</td>
                        <td>{fmtQty(l.received_qty)}</td>
                        <td>
                          <input
                            value={receiveQty[l.id] ?? ''}
                            onChange={(e) => setReceiveQty((prev) => ({ ...prev, [l.id]: e.target.value }))}
                            placeholder="Qty"
                            inputMode="decimal"
                            disabled={po.status === 'received' || po.status === 'cancelled'}
                          />
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                <button type="button" onClick={() => void doReceivePo(po)} disabled={busy || po.status === 'received' || po.status === 'cancelled'}>
                  Receive
                </button>
              </div>
            ))}
            {purchaseOrders.length === 0 && <p className="hint">No purchase orders yet.</p>}

            <h3>Vendor EDI (Phase 4)</h3>
            <p className="hint">PO send stages the X12 850 through the local file-drop transport stand-in — a staged file has not reached any vendor. Intake parses 856 / 810 / 855 / 832; an 855 reports the ack application without changing the PO, and an 832 builds a price-apply report that stays pending review until a human marks it applied.</p>
            <div className="entry">
              <input value={ediPoId} onChange={(e) => setEdiPoId(e.target.value)} placeholder="PO id (UUID)" />
              <input value={ediOutDir} onChange={(e) => setEdiOutDir(e.target.value)} placeholder="Transport root (out-dir, optional)" />
              <input value={ediVendorId} onChange={(e) => setEdiVendorId(e.target.value)} placeholder="Vendor id" />
              <button type="button" onClick={() => void refreshEdiForPo(ediPoId.trim()).catch(fail)} disabled={busy || !ediPoId.trim()}>Load PO documents</button>
            </div>
            <div className="actions">
              <button type="button" onClick={() => void doSendPo(ediPoId.trim())} disabled={busy || !ediPoId.trim()}>Send PO as X12 850</button>
              <button type="button" onClick={() => void doThreeWayMatch(ediPoId.trim())} disabled={busy || !ediPoId.trim()}>Run 3-way match</button>
              <button type="button" onClick={() => void doLoadTransport()} disabled={busy}>Load transport records</button>
            </div>
            {sendResult && (
              <div className="card">
                <p>Sent {sendResult.po_number} — doc {sendResult.doc_id.slice(0, 8)}{sendResult.written_to ? ` — staged at ${sendResult.written_to}` : ' — no file staged'}.</p>
                <pre>{sendResult.x12_850}</pre>
              </div>
            )}
            {matchReport && (
              <div className="card">
                <h3>3-way match — {matchReport.po_number} — {matchReport.status}</h3>
                <p>PO total {matchReport.po_total} · invoiced {matchReport.invoiced_total} · variance {matchReport.total_variance_amount}{matchReport.stated_total_difference !== null ? ` · stated-total difference ${matchReport.stated_total_difference}` : ''}</p>
                <table>
                  <thead><tr><th>SKU</th><th>Ordered</th><th>Received</th><th>Invoiced</th><th>Status</th><th>Variance kind</th></tr></thead>
                  <tbody>
                    {matchReport.lines.map((l) => (
                      <tr key={l.vendor_sku}>
                        <td>{l.vendor_sku}</td>
                        <td>{fmtQty(l.po_qty)}</td>
                        <td>{fmtQty(l.received_qty)}</td>
                        <td>{fmtQty(l.invoiced_qty)}</td>
                        <td>{l.status}</td>
                        <td>{l.variance_kind}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
            {ediDocs.length > 0 && (
              <>
                <h3>EDI documents for this PO</h3>
                <table>
                  <thead><tr><th>Type</th><th>Direction</th><th>Vendor</th><th>Received</th></tr></thead>
                  <tbody>
                    {ediDocs.map((d) => (
                      <tr key={d.id}>
                        <td>{d.doc_type}</td>
                        <td>{d.direction}</td>
                        <td>{d.vendor_id}</td>
                        <td>{d.received_at}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </>
            )}
            {ediAcks.length > 0 && (
              <>
                <h3>855 acknowledgments for this PO</h3>
                {ediAcks.map((row) => (
                  <div key={row.id} className="card">
                    <p>PO {row.ack.po_number} — acked {row.ack.ack_date ?? '—'} — received {row.received_at}</p>
                    <table>
                      <thead><tr><th>Line</th><th>SKU</th><th>Code</th><th>Qty acked</th><th>UoM</th><th>Unit cost</th><th>Promised</th></tr></thead>
                      <tbody>
                        {row.ack.lines.map((l) => (
                          <tr key={l.line_no}>
                            <td>{l.line_no}</td>
                            <td>{l.vendor_sku}</td>
                            <td>{l.ack_code}</td>
                            <td>{fmtQty(l.qty_acknowledged)}</td>
                            <td>{l.uom}</td>
                            <td>{l.unit_cost ?? '—'}</td>
                            <td>{l.promised_date ?? '—'}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                ))}
              </>
            )}
            <h3>Vendor document intake</h3>
            <div className="entry">
              <select value={ediDocType} onChange={(e) => setEdiDocType(e.target.value)}>
                <option value="856">856 ASN</option>
                <option value="810">810 invoice</option>
                <option value="855">855 acknowledgment</option>
                <option value="832">832 price catalog</option>
                <option value="csv">CSV ASN / invoice</option>
              </select>
              <button type="button" onClick={() => void doIntakeEdi()} disabled={busy}>Intake document</button>
            </div>
            <textarea
              value={ediRawText}
              onChange={(e) => setEdiRawText(e.target.value)}
              placeholder="Paste the raw X12 (or CSV) document text here"
              rows={6}
              style={{ width: '100%' }}
            />
            {intakeResult && (
              <div className="card">
                <p>Intake {intakeResult.doc_type} — {intakeResult.note}</p>
                {intakeResult.ack_application && (
                  <p> Ack: {intakeResult.ack_application.accepted_lines} accepted · {intakeResult.ack_application.rejected_lines} rejected · {intakeResult.ack_application.changed_lines} changed{intakeResult.ack_application.unmatched_skus.length > 0 ? ` · unmatched SKUs: ${intakeResult.ack_application.unmatched_skus.join(', ')}` : ''}.</p>
                )}
                {intakeResult.price_application_id && (
                  <p>Price-apply report {intakeResult.price_application_id.slice(0, 8)} stored pending review — load it below to review.</p>
                )}
              </div>
            )}
            <h3>832 price application review</h3>
            <div className="entry">
              <input value={priceAppId} onChange={(e) => setPriceAppId(e.target.value)} placeholder="Price application id (UUID)" />
              <button type="button" onClick={() => void doLoadPriceApp()} disabled={busy}>Load application</button>
              <input value={reviewedBy} onChange={(e) => setReviewedBy(e.target.value)} placeholder="Reviewed by" />
              <button type="button" onClick={() => void doMarkPriceAppApplied()} disabled={busy || !priceApp}>Mark applied</button>
            </div>
            {priceApp && (
              <div className="card">
                <p>Catalog {priceApp.application.catalog_id} — {priceApp.application.vendor_id} — {priceApp.application.catalog_date} — status <strong>{priceApp.application.status}</strong>{priceApp.application.reviewed_by ? ` — reviewed by ${priceApp.application.reviewed_by}` : ''}{priceApp.applied_at ? ` — applied ${priceApp.applied_at}` : ''}.</p>
                <p className="hint">Marking applied records the review decision only — it rewrites no product prices.</p>
                <table>
                  <thead><tr><th>SKU</th><th>Action</th><th>Old cost / list</th><th>New cost / list</th><th>Result</th><th>Note</th></tr></thead>
                  <tbody>
                    {priceApp.application.lines.map((l) => (
                      <tr key={l.vendor_sku}>
                        <td>{l.vendor_sku}</td>
                        <td>{l.action}</td>
                        <td>{l.old_cost ?? '—'} / {l.old_list_price ?? '—'}</td>
                        <td>{l.new_cost ?? '—'} / {l.new_list_price ?? '—'}</td>
                        <td>{l.result}</td>
                        <td>{l.note}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
            {transportRecords.length > 0 && (
              <>
                <h3>Transport records — {ediVendorId}</h3>
                <table>
                  <thead><tr><th>File</th><th>Direction</th><th>Transport</th><th>Doc</th><th>Status</th><th>When</th></tr></thead>
                  <tbody>
                    {transportRecords.map((r) => (
                      <tr key={r.id}>
                        <td>{r.file_name}</td>
                        <td>{r.direction}</td>
                        <td>{r.transport}</td>
                        <td>{r.doc_type ?? '—'}</td>
                        <td>{r.status}{r.error ? ` (${r.error})` : ''}</td>
                        <td>{r.created_at}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </>
            )}
            <h3>Vendor catalog CSV import</h3>
            <textarea
              value={ediCsvText}
              onChange={(e) => setEdiCsvText(e.target.value)}
              placeholder="Paste vendor catalog CSV here"
              rows={4}
              style={{ width: '100%' }}
            />
            <div className="actions">
              <button type="button" onClick={() => void doImportCatalog()} disabled={busy}>Import catalog CSV</button>
            </div>
            {catalogResult && (
              <div className="card">
                <p>{catalogResult.imported} item(s) imported, {catalogResult.errors.length} row error(s).</p>
                {catalogResult.errors.length > 0 && (
                  <ul>
                    {catalogResult.errors.map((e) => (
                      <li key={e.row}>Row {e.row}: {e.message}</li>
                    ))}
                  </ul>
                )}
              </div>
            )}
          </section>
        )}

        {effectiveTab === 'transfers' && (
          <section>
            <h2>Inter-store transfers</h2>
            <label>
              Destination store id
              <input value={toStoreId} onChange={(e) => setToStoreId(e.target.value)} placeholder="UUID of the receiving store" />
            </label>
            <div className="entry">
              <select value={transferProduct} onChange={(e) => setTransferProduct(e.target.value)}>
                {productOptions.map((p) => (
                  <option key={p.id} value={p.id}>{p.label}</option>
                ))}
              </select>
              <input value={transferQty} onChange={(e) => setTransferQty(e.target.value)} placeholder="Qty" inputMode="decimal" />
              <button
                type="button"
                onClick={() => {
                  if (!transferProduct || !transferQty) return
                  setTransferLines((prev) => [...prev, { product_id: transferProduct, qty: transferQty }])
                }}
              >
                Add line
              </button>
            </div>
            {transferLines.length > 0 && (
              <ul>
                {transferLines.map((l, i) => (
                  <li key={i}>{productName(l.product_id)} — {fmtQty(l.qty)} <button type="button" onClick={() => setTransferLines((prev) => prev.filter((_, j) => j !== i))}>Remove</button></li>
                ))}
              </ul>
            )}
            <button type="button" onClick={() => void doCreateTransfer()} disabled={busy}>Create draft transfer</button>
            <h3>Transfers</h3>
            {transfers.map((t) => (
              <div key={t.id} className="card">
                <p>
                  {t.id.slice(0, 8)} — {t.from_store_id.slice(0, 8)} → {t.to_store_id.slice(0, 8)} — <strong>{t.status}</strong>
                </p>
                <ul>
                  {t.lines.map((l, i) => (
                    <li key={i}>{productName(l.product_id)} — {fmtQty(l.qty)}</li>
                  ))}
                </ul>
                <div className="actions">
                  <button type="button" onClick={() => void doShipTransfer(t.id)} disabled={busy || t.status !== 'draft'}>Ship</button>
                  <button type="button" onClick={() => void doReceiveTransfer(t.id)} disabled={busy || t.status !== 'shipped'}>Receive</button>
                </div>
              </div>
            ))}
            {transfers.length === 0 && <p className="hint">No transfers yet.</p>}
          </section>
        )}

        {effectiveTab === 'counts' && (
          <section>
            <h2>Physical counts</h2>
            <div className="actions">
              <button type="button" onClick={() => void doStartCount()} disabled={busy}>Start count</button>
              <button type="button" onClick={() => void refreshCounts().catch(fail)}>Refresh</button>
            </div>
            <label>
              Count
              <select value={selectedCountId} onChange={(e) => setSelectedCountId(e.target.value)}>
                <option value="">— select —</option>
                {counts.map((c) => (
                  <option key={c.id} value={c.id}>{c.id.slice(0, 8)} — {c.status}</option>
                ))}
              </select>
            </label>
            {selectedCount && (
              <>
                <div className="entry">
                  <select value={countProduct} onChange={(e) => setCountProduct(e.target.value)}>
                    {productOptions.map((p) => (
                      <option key={p.id} value={p.id}>{p.label}</option>
                    ))}
                  </select>
                  <input value={countQty} onChange={(e) => setCountQty(e.target.value)} placeholder="Counted qty" inputMode="decimal" />
                  <button type="button" onClick={() => void doScanCount()} disabled={busy || selectedCount.status !== 'open'}>Scan / enter</button>
                </div>
                <table>
                  <thead><tr><th>Product</th><th>Counted</th><th>System</th><th>Variance</th></tr></thead>
                  <tbody>
                    {selectedCount.lines.map((l) => (
                      <tr key={l.product_id}>
                        <td>{productName(l.product_id)}</td>
                        <td>{fmtQty(l.counted_qty)}</td>
                        <td>{fmtQty(l.system_qty)}</td>
                        <td>{fmtQty(l.variance_qty)}</td>
                      </tr>
                    ))}
                    {selectedCount.lines.length === 0 && (
                      <tr><td colSpan={4}>No lines scanned yet.</td></tr>
                    )}
                  </tbody>
                </table>
                {selectedCount.adjustments.length > 0 && (
                  <>
                    <h3>Reconciled adjustments</h3>
                    <ul>
                      {selectedCount.adjustments.map((a) => (
                        <li key={a.product_id}>{productName(a.product_id)}: {fmtQty(a.adjustment)} (was {fmtQty(a.on_hand_before)}, counted {fmtQty(a.counted_qty)})</li>
                      ))}
                    </ul>
                  </>
                )}
                <button type="button" onClick={() => void doReconcileCount()} disabled={busy || selectedCount.status !== 'open'}>
                  Reconcile count
                </button>
                <p className="hint">Reconciling posts <code>count_adj</code> ledger entries so on-hand equals the counted quantity.</p>
              </>
            )}
          </section>
        )}

        {effectiveTab === 'tags' && (
          <section>
            <h2>Shelf tags (ZPL)</h2>
            <div className="entry">
              <select value={tagProduct} onChange={(e) => setTagProduct(e.target.value)}>
                {productOptions.map((p) => (
                  <option key={p.id} value={p.id}>{p.label}</option>
                ))}
              </select>
              <button type="button" onClick={() => void doShelfTag()} disabled={busy}>Generate tag</button>
            </div>
            {tag && (
              <>
                <p>{tag.name} — {tag.sku} — {fmtMoney(tag.price)} — barcode {tag.barcode}</p>
                <pre>{tag.zpl}</pre>
                <p className="hint">Send this ZPL to a ZPL printer to print the shelf tag.</p>
              </>
            )}
          </section>
        )}

        {effectiveTab === 'pricing' && (
          <section>
            <h2>Bulk matrix pricing</h2>
            <p className="hint">Edit the calculation of any rule, then save all. A rule whose computed price falls below its min-margin floor is rejected — the whole batch is refused and nothing changes.</p>
            <table>
              <thead><tr><th>Rule</th><th>Method</th><th>Value</th><th>Min margin %</th></tr></thead>
              <tbody>
                {rules.map((r) => (
                  <tr key={r.id}>
                    <td>
                      {r.id.slice(0, 8)} <small>pri {r.priority}{r.product_id ? ` · ${productName(r.product_id)}` : ' · all products'}</small>
                    </td>
                    <td>
                      <select
                        value={r.calc_method}
                        onChange={(e) => setRules((prev) => prev.map((x) => (x.id === r.id ? { ...x, calc_method: e.target.value } : x)))}
                      >
                        {CALC_METHODS.map((m) => (
                          <option key={m} value={m}>{m}</option>
                        ))}
                      </select>
                    </td>
                    <td>
                      <input
                        value={r.calc_value}
                        onChange={(e) => setRules((prev) => prev.map((x) => (x.id === r.id ? { ...x, calc_value: e.target.value } : x)))}
                        inputMode="decimal"
                      />
                    </td>
                    <td>
                      <input
                        value={r.min_margin_floor ?? ''}
                        onChange={(e) => setRules((prev) => prev.map((x) => (x.id === r.id ? { ...x, min_margin_floor: e.target.value || null } : x)))}
                        placeholder="none"
                        inputMode="decimal"
                      />
                    </td>
                  </tr>
                ))}
                {rules.length === 0 && (
                  <tr><td colSpan={4}>No pricing rules in the edge database yet.</td></tr>
                )}
              </tbody>
            </table>
            <button type="button" onClick={() => void doSavePricing()} disabled={busy || rules.length === 0}>Save all rules</button>
          </section>
        )}

        {effectiveTab === 'crm' && (
          <section>
            <h2>CRM — activities, tickler, appointments, saved views</h2>
            <div className="actions">
              <button type="button" onClick={() => void refreshCrm().catch(fail)}>Refresh CRM</button>
            </div>

            <h3>Log an activity</h3>
            <div className="entry">
              <select value={activityForm.activity_type} onChange={(e) => setActivityForm({ ...activityForm, activity_type: e.target.value })}>
                <option value="call">call</option>
                <option value="email">email</option>
                <option value="visit">visit</option>
                <option value="quote">quote</option>
                <option value="note">note</option>
                <option value="follow_up">follow_up</option>
              </select>
              <input value={activityForm.customer_id} onChange={(e) => setActivityForm({ ...activityForm, customer_id: e.target.value })} placeholder="Customer id (UUID, optional)" />
              <input value={activityForm.subject} onChange={(e) => setActivityForm({ ...activityForm, subject: e.target.value })} placeholder="Subject" />
              <input value={activityForm.notes} onChange={(e) => setActivityForm({ ...activityForm, notes: e.target.value })} placeholder="Notes (optional)" />
              <input value={activityForm.due_at} onChange={(e) => setActivityForm({ ...activityForm, due_at: e.target.value })} placeholder="Due at, RFC 3339 (optional — makes it a reminder)" />
              <button type="button" onClick={() => void doCreateActivity()} disabled={busy}>Log activity</button>
            </div>

            <h3>Tickler — due now ({dueReminders.length}) · overdue ({overdueReminders.length})</h3>
            <div className="entry">
              <input value={snoozeUntil} onChange={(e) => setSnoozeUntil(e.target.value)} placeholder="Snooze target, RFC 3339" />
            </div>
            <table>
              <thead><tr><th>Subject</th><th>Type</th><th>Due</th><th>Actions</th></tr></thead>
              <tbody>
                {[...dueReminders, ...overdueReminders.filter((o) => !dueReminders.some((d) => d.id === o.id))].map((a) => (
                  <tr key={a.id}>
                    <td>{a.subject}</td>
                    <td>{a.activity_type}</td>
                    <td>{a.due_at ?? '—'}{a.snoozed_until ? ` (snoozed to ${a.snoozed_until})` : ''}</td>
                    <td>
                      <button type="button" onClick={() => void doCompleteActivity(a.id)} disabled={busy}>Complete</button>{' '}
                      <button type="button" onClick={() => void doSnoozeActivity(a.id)} disabled={busy}>Snooze</button>
                    </td>
                  </tr>
                ))}
                {dueReminders.length === 0 && overdueReminders.length === 0 && (
                  <tr><td colSpan={4}>Nothing due.</td></tr>
                )}
              </tbody>
            </table>

            <h3>Activity log ({activities.length})</h3>
            <table>
              <thead><tr><th>Subject</th><th>Type</th><th>Customer</th><th>Due</th><th>Done</th><th>Actions</th></tr></thead>
              <tbody>
                {activities.map((a) => (
                  <tr key={a.id}>
                    <td>{a.subject}</td>
                    <td>{a.activity_type}</td>
                    <td>{a.customer_id ? a.customer_id.slice(0, 8) : '—'}</td>
                    <td>{a.due_at ?? '—'}</td>
                    <td>{a.completed_at ? 'yes' : 'no'}</td>
                    <td>
                      <button type="button" onClick={() => void doDeleteActivity(a.id)} disabled={busy}>Delete</button>
                    </td>
                  </tr>
                ))}
                {activities.length === 0 && (
                  <tr><td colSpan={6}>No activities logged yet.</td></tr>
                )}
              </tbody>
            </table>

            <h3>Book an appointment</h3>
            <div className="entry">
              <input value={apptForm.title} onChange={(e) => setApptForm({ ...apptForm, title: e.target.value })} placeholder="Title" />
              <input value={apptForm.customer_id} onChange={(e) => setApptForm({ ...apptForm, customer_id: e.target.value })} placeholder="Customer id (UUID, optional)" />
              <input value={apptForm.staff_id} onChange={(e) => setApptForm({ ...apptForm, staff_id: e.target.value })} placeholder="Staff id (UUID, optional)" />
              <input value={apptForm.starts_at} onChange={(e) => setApptForm({ ...apptForm, starts_at: e.target.value })} placeholder="Starts at, RFC 3339" />
              <input value={apptForm.ends_at} onChange={(e) => setApptForm({ ...apptForm, ends_at: e.target.value })} placeholder="Ends at, RFC 3339" />
              <button type="button" onClick={() => void doCreateAppointment()} disabled={busy}>Book</button>
            </div>
            <p className="hint">Overlaps are refused for the same contractor or the same staff member; adjacent slots are free.</p>
            <div className="entry">
              <input value={apptDate} onChange={(e) => setApptDate(e.target.value)} placeholder="Filter by day YYYY-MM-DD" />
              <button
                type="button"
                onClick={() => {
                  setBusy(true)
                  listAppointments(apptDate.trim() ? { date: apptDate.trim() } : undefined)
                    .then(setAppointments)
                    .catch(fail)
                    .finally(() => setBusy(false))
                }}
                disabled={busy}
              >
                Filter appointments
              </button>
            </div>
            <table>
              <thead><tr><th>Title</th><th>Starts</th><th>Ends</th><th>Status</th><th>Actions</th></tr></thead>
              <tbody>
                {appointments.map((a) => (
                  <tr key={a.id}>
                    <td>{a.title}</td>
                    <td>{a.starts_at}</td>
                    <td>{a.ends_at}</td>
                    <td>{a.status}</td>
                    <td>
                      {a.status === 'scheduled' && (
                        <button type="button" onClick={() => void doCancelAppointment(a.id)} disabled={busy}>Cancel</button>
                      )}
                    </td>
                  </tr>
                ))}
                {appointments.length === 0 && (
                  <tr><td colSpan={5}>No appointments.</td></tr>
                )}
              </tbody>
            </table>

            <h3>Saved views</h3>
            <p className="hint">Two dialects: the activity dialect (<code>activity_type</code>, <code>customer_id</code>, <code>completed</code>, <code>due_before</code>) filters the activity log; the customer dialect (<code>customer_type</code>, <code>charge_enabled</code>, <code>min_balance</code>, …) segments customers. Running a view uses the dialect its JSON is written in.</p>
            <div className="entry">
              <input value={viewForm.name} onChange={(e) => setViewForm({ ...viewForm, name: e.target.value })} placeholder="View name" />
              <input value={viewForm.filter_json} onChange={(e) => setViewForm({ ...viewForm, filter_json: e.target.value })} placeholder='Filter JSON, e.g. {"activity_type": "follow_up", "completed": false}' />
              <button type="button" onClick={() => void doCreateSavedView()} disabled={busy}>Save view</button>
            </div>
            <div className="entry">
              <select value={selectedViewId} onChange={(e) => setSelectedViewId(e.target.value)}>
                <option value="">— select a view —</option>
                {savedViews.map((v) => (
                  <option key={v.id} value={v.id}>{v.name}</option>
                ))}
              </select>
              <button type="button" onClick={() => void doRunSavedView()} disabled={busy || !selectedViewId}>Run view</button>
            </div>
            {viewActivities !== null && (
              <>
                <h3>View results — activities ({viewActivities.length})</h3>
                <table>
                  <thead><tr><th>Subject</th><th>Type</th><th>Due</th><th>Done</th></tr></thead>
                  <tbody>
                    {viewActivities.map((a) => (
                      <tr key={a.id}>
                        <td>{a.subject}</td>
                        <td>{a.activity_type}</td>
                        <td>{a.due_at ?? '—'}</td>
                        <td>{a.completed_at ? 'yes' : 'no'}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </>
            )}
            {viewCustomers !== null && (
              <>
                <h3>View results — customers ({viewCustomers.length})</h3>
                <table>
                  <thead><tr><th>Account</th><th>Company</th><th>Type</th><th>Balance</th><th>Last purchase</th></tr></thead>
                  <tbody>
                    {viewCustomers.map((c) => (
                      <tr key={c.customer_id}>
                        <td>{c.account_number}</td>
                        <td>{c.company_name ?? '—'}</td>
                        <td>{c.customer_type}</td>
                        <td>{fmtMoney(c.balance)}</td>
                        <td>{c.last_purchase_at ?? '—'}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </>
            )}

            <h3>Lapsed-contractor report</h3>
            <div className="entry">
              <input value={lapsedDays} onChange={(e) => setLapsedDays(e.target.value)} placeholder="Inactive days" inputMode="numeric" />
              <button type="button" onClick={() => void doLapsedReport()} disabled={busy}>Run lapsed report</button>
            </div>
            {lapsed && (
              <>
                <p>As of {lapsed.as_of} · inactive &gt; {lapsed.inactive_days} days · {lapsed.customers.length} contractor(s).</p>
                <table>
                  <thead><tr><th>Account</th><th>Company</th><th>Status</th><th>Last purchase</th><th>Trailing 365d spend</th><th>AR balance</th></tr></thead>
                  <tbody>
                    {lapsed.customers.map((c) => (
                      <tr key={c.customer_id}>
                        <td>{c.account_number}</td>
                        <td>{c.company_name ?? '—'}</td>
                        <td>{c.status}</td>
                        <td>{c.last_purchase_at ?? 'never'}</td>
                        <td>{fmtMoney({ amount: c.trailing_spend, currency: 'Usd' })}</td>
                        <td>{fmtMoney({ amount: c.ar_balance, currency: 'Usd' })}</td>
                      </tr>
                    ))}
                    {lapsed.customers.length === 0 && (
                      <tr><td colSpan={6}>No lapsed contractors in this window.</td></tr>
                    )}
                  </tbody>
                </table>
              </>
            )}
          </section>
        )}

        {effectiveTab === 'delivery' && (
          <section>
            <h2>Delivery — dispatch board, packing slips, proof of delivery</h2>
            <div className="actions">
              <button type="button" onClick={() => void refreshDelivery().catch(fail)}>Refresh board</button>
            </div>

            <h3>Create a delivery job</h3>
            <div className="entry">
              <input value={deliveryForm.transaction_id} onChange={(e) => setDeliveryForm({ ...deliveryForm, transaction_id: e.target.value })} placeholder="Transaction id (UUID, optional)" />
              <input value={deliveryForm.customer_id} onChange={(e) => setDeliveryForm({ ...deliveryForm, customer_id: e.target.value })} placeholder="Customer id (UUID, optional)" />
              <input value={deliveryForm.scheduled_date} onChange={(e) => setDeliveryForm({ ...deliveryForm, scheduled_date: e.target.value })} placeholder="Scheduled date YYYY-MM-DD (optional)" />
              <input value={deliveryForm.priority} onChange={(e) => setDeliveryForm({ ...deliveryForm, priority: e.target.value })} placeholder="Priority (higher = more urgent)" inputMode="numeric" />
              <input value={deliveryForm.vehicle} onChange={(e) => setDeliveryForm({ ...deliveryForm, vehicle: e.target.value })} placeholder="Vehicle (optional)" />
            </div>
            <input
              value={deliveryForm.address_json}
              onChange={(e) => setDeliveryForm({ ...deliveryForm, address_json: e.target.value })}
              placeholder='Address JSON, e.g. {"street": "1 Main St", "city": "Vancouver", "postal_code": "V6B 1A1"}'
              style={{ width: '100%' }}
            />
            <div className="actions">
              <button type="button" onClick={() => void doCreateDelivery()} disabled={busy}>Create delivery</button>
            </div>

            <h3>Dispatch board ({deliveries.length} open)</h3>
            <div className="entry">
              <select value={dispatchStatus} onChange={(e) => setDispatchStatus(e.target.value)}>
                <option value="">all open statuses</option>
                <option value="draft">draft</option>
                <option value="assigned">assigned</option>
                <option value="out_for_delivery">out_for_delivery</option>
              </select>
              <button
                type="button"
                onClick={() => {
                  setBusy(true)
                  listDispatch(dispatchStatus ? { status: dispatchStatus } : undefined)
                    .then(setDeliveries)
                    .catch(fail)
                    .finally(() => setBusy(false))
                }}
                disabled={busy}
              >
                Filter board
              </button>
            </div>
            <table>
              <thead><tr><th>ID</th><th>Status</th><th>Priority</th><th>Driver</th><th>Vehicle</th><th>Scheduled</th><th>POD</th><th>Actions</th></tr></thead>
              <tbody>
                {deliveries.map((d) => (
                  <tr key={d.id}>
                    <td>{d.id.slice(0, 8)}</td>
                    <td>{d.status}</td>
                    <td>{d.priority}</td>
                    <td>{d.driver_id ? d.driver_id.slice(0, 8) : '—'}</td>
                    <td>{d.vehicle ?? '—'}</td>
                    <td>{d.scheduled_date ?? '—'}</td>
                    <td>{d.pod ? 'submitted' : '—'}</td>
                    <td>
                      <button type="button" onClick={() => { setSelectedDelivery(d); setPackingSlip(null) }}>Open</button>
                    </td>
                  </tr>
                ))}
                {deliveries.length === 0 && (
                  <tr><td colSpan={8}>No open deliveries.</td></tr>
                )}
              </tbody>
            </table>

            {selectedDelivery && (
              <div className="card">
                <h3>Delivery {selectedDelivery.id.slice(0, 8)} — {selectedDelivery.status}</h3>
                <p>
                  Transaction {selectedDelivery.transaction_id ? selectedDelivery.transaction_id.slice(0, 8) : '—'} · customer {selectedDelivery.customer_id ? selectedDelivery.customer_id.slice(0, 8) : '—'} · driver {selectedDelivery.driver_id ? selectedDelivery.driver_id.slice(0, 8) : '—'} · vehicle {selectedDelivery.vehicle ?? '—'}
                </p>
                <p>Address: <code>{JSON.stringify(selectedDelivery.address_json)}</code></p>
                <div className="entry">
                  <input value={assignForm.driver_id} onChange={(e) => setAssignForm({ ...assignForm, driver_id: e.target.value })} placeholder="Driver id (UUID)" />
                  <input value={assignForm.vehicle} onChange={(e) => setAssignForm({ ...assignForm, vehicle: e.target.value })} placeholder="Vehicle (optional)" />
                  <button type="button" onClick={() => void doAssignDelivery(selectedDelivery.id)} disabled={busy || selectedDelivery.status !== 'draft'}>Assign driver</button>
                </div>
                <div className="actions">
                  <button type="button" onClick={() => void doSetDeliveryStatus(selectedDelivery.id, 'out_for_delivery')} disabled={busy || selectedDelivery.status !== 'assigned'}>Mark out for delivery</button>
                  <button type="button" onClick={() => void doSetDeliveryStatus(selectedDelivery.id, 'cancelled')} disabled={busy || selectedDelivery.status === 'delivered' || selectedDelivery.status === 'cancelled'}>Cancel delivery</button>
                  <button type="button" onClick={() => void doShowPackingSlip(selectedDelivery.id)} disabled={busy}>Show packing slip</button>
                </div>
                {selectedDelivery.events.length > 0 && (
                  <>
                    <h3>Audit trail</h3>
                    <ul>
                      {selectedDelivery.events.map((e) => (
                        <li key={e.id}>{e.created_at} — {e.from_status} → {e.to_status}{e.note ? ` — ${e.note}` : ''}</li>
                      ))}
                    </ul>
                  </>
                )}
                <h3>Proof of delivery</h3>
                <p className="hint">Close-out needs a signature AND a photo reference — a submission without both is refused up front and nothing is written. The photo itself is never stored; only its reference and SHA-256.</p>
                {selectedDelivery.pod ? (
                  <p>Submitted by {selectedDelivery.pod.received_by} at {selectedDelivery.pod.signed_at} — photo {selectedDelivery.pod.photo_ref ?? '—'} (SHA-256 {selectedDelivery.pod.photo_sha256?.slice(0, 12) ?? '—'}…).</p>
                ) : (
                  <p>No POD submitted yet.</p>
                )}
                <div className="entry">
                  <input value={podForm.received_by} onChange={(e) => setPodForm({ ...podForm, received_by: e.target.value })} placeholder="Received by (signer name)" />
                  <input value={podForm.photo_ref} onChange={(e) => setPodForm({ ...podForm, photo_ref: e.target.value })} placeholder="Photo reference (path or key)" />
                  <input value={podForm.photo_sha256} onChange={(e) => setPodForm({ ...podForm, photo_sha256: e.target.value })} placeholder="Photo SHA-256 (64 hex, optional)" />
                </div>
                <textarea
                  value={podForm.signature_b64}
                  onChange={(e) => setPodForm({ ...podForm, signature_b64: e.target.value })}
                  placeholder="Signature bytes as base64 (e.g. PNG from a signature pad)"
                  rows={3}
                  style={{ width: '100%' }}
                />
                <div className="actions">
                  <button type="button" onClick={() => void doSubmitPod(selectedDelivery.id)} disabled={busy || selectedDelivery.status !== 'out_for_delivery'}>Submit POD + close out</button>
                </div>
                {packingSlip && (
                  <>
                    <h3>Packing slip {packingSlip.doc_number ? `— ${packingSlip.doc_number}` : ''}</h3>
                    <table>
                      <thead><tr><th>Description</th><th>Qty</th><th>UoM</th></tr></thead>
                      <tbody>
                        {packingSlip.lines.map((l) => (
                          <tr key={l.product_id}>
                            <td>{l.description}</td>
                            <td>{fmtQty(l.qty)}</td>
                            <td>{l.uom}</td>
                          </tr>
                        ))}
                        {packingSlip.lines.length === 0 && (
                          <tr><td colSpan={3}>No lines (delivery names no transaction).</td></tr>
                        )}
                      </tbody>
                    </table>
                    <pre>{packingSlip.text}</pre>
                    <p className="hint">ZPL label is stored with the slip for re-printing; the text above is the 42-column render.</p>
                  </>
                )}
              </div>
            )}
          </section>
        )}

        {effectiveTab === 'ar' && (
          <section>
            <h2>Contractor AR (read-only)</h2>
            <div className="entry">
              <input value={customerId} onChange={(e) => setCustomerId(e.target.value)} placeholder="Customer id (UUID)" />
              <button type="button" onClick={() => void doArSummary()} disabled={busy}>Customer summary</button>
              <button type="button" onClick={() => void refreshAr().catch(fail)}>Refresh aging</button>
            </div>
            {arSummary && (
              <div className="totals">
                <div><span>Account</span><strong>{arSummary.account_number}</strong></div>
                <div><span>Company</span><strong>{arSummary.company_name ?? '—'}</strong></div>
                <div><span>Balance</span><strong>{fmtMoney(arSummary.balance)}</strong></div>
                <div><span>Credit limit</span><strong>{fmtMoney(arSummary.credit_limit)}</strong></div>
                <div><span>Charge enabled</span><strong>{arSummary.charge_enabled ? 'yes' : 'no'}</strong></div>
                <div><span>Credit hold</span><strong>{arSummary.credit_hold ? 'yes' : 'no'}</strong></div>
                <div><span>Entries</span><strong>{arSummary.entry_count}</strong></div>
              </div>
            )}
            {aging && (
              <>
                <h3>Aging as of {aging.as_of}</h3>
                <table>
                  <thead><tr><th>Account</th><th>Balance</th><th>Current</th><th>31–60</th><th>61–90</th><th>91–120</th><th>120+</th></tr></thead>
                  <tbody>
                    {aging.customers.map((c) => (
                      <tr key={c.customer_id}>
                        <td>{c.account_number}</td>
                        <td>{fmtMoney(c.balance)}</td>
                        {c.buckets.map((b, i) => (
                          <td key={i}>{fmtMoney(b)}</td>
                        ))}
                      </tr>
                    ))}
                    <tr>
                      <td><strong>Totals</strong></td>
                      <td><strong>{fmtMoney({ amount: String(aging.totals.reduce((a, b) => a + Number(b.amount), 0)), currency: 'Usd' })}</strong></td>
                      {aging.totals.map((b, i) => (
                        <td key={i}><strong>{fmtMoney(b)}</strong></td>
                      ))}
                    </tr>
                  </tbody>
                </table>
              </>
            )}
            {collections && collections.customers.length > 0 && (
              <>
                <h3>Collections</h3>
                <table>
                  <thead><tr><th>Account</th><th>Balance</th><th>Overdue</th><th>Hold</th></tr></thead>
                  <tbody>
                    {collections.customers.map((c) => (
                      <tr key={c.customer_id}>
                        <td>{c.account_number}</td>
                        <td>{fmtMoney(c.balance)}</td>
                        <td>{fmtMoney(c.overdue)}</td>
                        <td>{c.credit_hold ? 'yes' : 'no'}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </>
            )}
          </section>
        )}
        {effectiveTab === 'bi' && (
          <section>
            <h2>BI — KPI dashboard, eco remittance, ad-hoc inquiry</h2>
            <div className="entry">
              <input value={biStart} onChange={(e) => setBiStart(e.target.value)} placeholder="Start YYYY-MM-DD (default today)" />
              <input value={biEnd} onChange={(e) => setBiEnd(e.target.value)} placeholder="End YYYY-MM-DD (default today)" />
              <button type="button" onClick={() => { setBusy(true); refreshBi().catch(fail).finally(() => setBusy(false)) }} disabled={busy}>Load BI</button>
              <button type="button" onClick={() => { setBusy(true); downloadEcoCsv().catch(fail).finally(() => setBusy(false)) }} disabled={busy}>Download eco CSV</button>
            </div>
            <p className="hint">KPIs come from the real edge ledgers (completed sales/returns, inventory ledger, AR ledger, stored formulas). Inventory turns value stock from the append-only ledger, not a snapshot table. Eco fees are grouped by each store&apos;s country/region jurisdiction.</p>
            {kpi && (
              <>
                <h3>KPI dashboard — {kpi.start} to {kpi.end} (AR as of {kpi.as_of})</h3>
                <div className="totals">
                  <div><span>Sales</span><strong>{kpi.sales.sale_count} sale(s)</strong></div>
                  <div><span>Returns</span><strong>{kpi.sales.return_count} return(s)</strong></div>
                  <div><span>Gross sales</span><strong>{fmtMoney(kpi.sales.gross_sales)}</strong></div>
                  <div><span>Net sales</span><strong>{fmtMoney(kpi.sales.net_sales)}</strong></div>
                  <div><span>Grand total</span><strong>{fmtMoney(kpi.sales.grand_total)}</strong></div>
                  <div><span>Avg ticket</span><strong>{kpi.sales.avg_ticket ? fmtMoney(kpi.sales.avg_ticket) : '—'}</strong></div>
                  <div><span>Revenue</span><strong>{fmtMoney(kpi.margin.revenue)}</strong></div>
                  <div><span>COGS</span><strong>{fmtMoney(kpi.margin.cogs)}</strong></div>
                  <div><span>Gross profit</span><strong>{fmtMoney(kpi.margin.gross_profit)}</strong></div>
                  <div><span>Margin</span><strong>{kpi.margin.margin_pct !== null ? `${kpi.margin.margin_pct}%` : '—'}</strong></div>
                  <div><span>On-hand value</span><strong>{fmtMoney(kpi.inventory.on_hand_value)}</strong></div>
                  <div><span>Avg inventory</span><strong>{fmtMoney(kpi.inventory.avg_inventory_value)}</strong></div>
                  <div><span>Inventory turns</span><strong>{kpi.inventory.turns !== null ? `${kpi.inventory.turns}x` : '—'}</strong></div>
                  <div><span>Total open AR</span><strong>{fmtMoney(kpi.total_open_ar)}</strong></div>
                  <div><span>Formulas created</span><strong>{kpi.formulas.formulas_created}</strong></div>
                  <div><span>Tint cost</span><strong>{fmtMoney(kpi.formulas.total_tint_cost)}</strong></div>
                </div>
                <table>
                  <thead><tr><th>AR aging</th><th>Current</th><th>31–60</th><th>61–90</th><th>91–120</th><th>120+</th></tr></thead>
                  <tbody>
                    <tr>
                      <td>Open charges</td>
                      <td>{fmtMoney(kpi.ar_aging.current)}</td>
                      <td>{fmtMoney(kpi.ar_aging.days_31_60)}</td>
                      <td>{fmtMoney(kpi.ar_aging.days_61_90)}</td>
                      <td>{fmtMoney(kpi.ar_aging.days_91_120)}</td>
                      <td>{fmtMoney(kpi.ar_aging.over_120)}</td>
                    </tr>
                  </tbody>
                </table>
                {kpi.formulas.per_color.length > 0 && (
                  <table>
                    <thead><tr><th>Color</th><th>Name</th><th>Formulas</th></tr></thead>
                    <tbody>
                      {kpi.formulas.per_color.map((c) => (
                        <tr key={c.color_code}>
                          <td>{c.color_code}</td>
                          <td>{c.color_name}</td>
                          <td>{c.formulas}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </>
            )}
            {eco && (
              <>
                <h3>Eco-fee remittance — {eco.start} to {eco.end} (total {fmtMoney(eco.total)})</h3>
                <table>
                  <thead><tr><th>Jurisdiction</th><th>Store</th><th>Category</th><th>Transactions</th><th>Lines</th><th>Units</th><th>Eco fees</th></tr></thead>
                  <tbody>
                    {eco.rows.map((r) => (
                      <tr key={`${r.country_code}-${r.region_code}-${r.eco_fee_category ?? 'none'}-${r.store_id}`}>
                        <td>{r.country_code}-{r.region_code}</td>
                        <td>{r.store_code}</td>
                        <td>{r.eco_fee_category ?? 'uncategorized'}</td>
                        <td>{r.transaction_count}</td>
                        <td>{r.line_count}</td>
                        <td>{fmtQty(r.units)}</td>
                        <td>{fmtMoney(r.eco_fee_total)}</td>
                      </tr>
                    ))}
                    {eco.rows.length === 0 && (
                      <tr><td colSpan={7}>No eco fees in this period.</td></tr>
                    )}
                  </tbody>
                </table>
              </>
            )}
            <h3>Ad-hoc inquiry</h3>
            <div className="entry">
              <input value={inquiryDocType} onChange={(e) => setInquiryDocType(e.target.value)} placeholder="Doc type: sale / return (optional)" />
              <input value={inquiryProductId} onChange={(e) => setInquiryProductId(e.target.value)} placeholder="Product id (UUID, optional)" />
              <button
                type="button"
                onClick={() => {
                  if (!storeId) return
                  setBusy(true)
                  const today = new Date().toISOString().slice(0, 10)
                  runInquiry({
                    storeId,
                    start: biStart.trim() || today,
                    end: biEnd.trim() || today,
                    docType: inquiryDocType.trim() || undefined,
                    productId: inquiryProductId.trim() || undefined,
                  })
                    .then(setInquiryRows)
                    .catch(fail)
                    .finally(() => setBusy(false))
                }}
                disabled={busy}
              >
                Run inquiry
              </button>
            </div>
            {inquiryRows !== null && (
              <table>
                <thead><tr><th>Doc</th><th>Type</th><th>Date</th><th>Lines</th><th>Subtotal</th><th>Grand total</th></tr></thead>
                <tbody>
                  {inquiryRows.map((r) => (
                    <tr key={r.transaction_id}>
                      <td>{r.doc_number}</td>
                      <td>{r.doc_type}</td>
                      <td>{r.created_at.slice(0, 10)}</td>
                      <td>{r.line_count}</td>
                      <td>{fmtMoney(r.subtotal)}</td>
                      <td>{fmtMoney(r.grand_total)}</td>
                    </tr>
                  ))}
                  {inquiryRows.length === 0 && (
                    <tr><td colSpan={6}>No transactions match.</td></tr>
                  )}
                </tbody>
              </table>
            )}
            <h3>Scheduled report emails (file outbox)</h3>
            <p className="hint">The 07:00 UTC job renders the KPI dashboard (PDF) + eco remittance (CSV) and queues them here as <code>manifest.json</code> entries. Status stays <code>queued</code> — there is no SMTP send; a mail relay picks the outbox up. The outbox appears once the edge is started with <code>--email-outbox</code>.</p>
            {outbox !== null && (
              <table>
                <thead><tr><th>Report</th><th>Recipient</th><th>File</th><th>Scheduled</th><th>Status</th></tr></thead>
                <tbody>
                  {outbox.map((d) => (
                    <tr key={d.id}>
                      <td>{d.report_kind}</td>
                      <td>{d.recipient}</td>
                      <td>{d.file_name}</td>
                      <td>{d.scheduled_at.slice(0, 10)}</td>
                      <td>{d.status}</td>
                    </tr>
                  ))}
                  {outbox.length === 0 && (
                    <tr><td colSpan={5}>Outbox is empty — no scheduled emails queued yet.</td></tr>
                  )}
                </tbody>
              </table>
            )}
            {outbox === null && (
              <p className="hint">No outbox directory configured for this edge.</p>
            )}
          </section>
        )}
        {effectiveTab === 'scheduling' && (
          <section>
            <h2>Scheduling+ — shifts, timeclock, labor forecast</h2>
            <div className="entry">
              <input value={schedDate} onChange={(e) => setSchedDate(e.target.value)} placeholder="Date YYYY-MM-DD (default today)" />
              <input value={forecastDays} onChange={(e) => setForecastDays(e.target.value)} placeholder="Forecast days (default 7)" />
              <input value={forecastRatio} onChange={(e) => setForecastRatio(e.target.value)} placeholder="Sales per labor hour override (optional)" />
              <button type="button" onClick={() => { setBusy(true); refreshScheduling().catch(fail).finally(() => setBusy(false)) }} disabled={busy}>Refresh</button>
            </div>
            <p className="hint">All scheduling times are UTC in v1.0. Shift start/end are entered in this browser&apos;s local time and stored as UTC. The forecast reads the last 28 days of completed sales and closed timeclock entries; a fresh store has no history yet, so the forecast reports that instead of inventing hours.</p>

            {coverage && (
              <div className="totals">
                <div><span>Coverage — {coverage.date}</span><strong>{coverage.status}</strong></div>
                <div><span>Forecast hours</span><strong>{coverage.forecast_hours ?? '—'}</strong></div>
                <div><span>Scheduled hours</span><strong>{coverage.scheduled_hours}</strong></div>
                <div><span>Delta hours</span><strong>{coverage.delta_hours ?? '—'}</strong></div>
              </div>
            )}

            {forecast && (
              <>
                <h3>Labor forecast — {forecast.days.length} day(s) at {forecast.sales_per_labor_hour} sales per labor hour{forecast.ratio_from_history ? ' (from history)' : ' (override)'}</h3>
                <p className="hint">History: {forecast.history_days} day(s), {forecast.history_txns} sale(s), {fmtMoney({ amount: forecast.history_sales, currency: 'Usd' })} sales, {forecast.history_labor_hours} worked hours.</p>
                <table>
                  <thead><tr><th>Date</th><th>Forecast txns</th><th>Forecast sales</th><th>Required hours</th><th>Peak hour (UTC)</th></tr></thead>
                  <tbody>
                    {forecast.days.map((d) => {
                      const peak = d.hourly.length > 0 ? d.hourly.reduce((a, b) => (Number(b.required_hours) > Number(a.required_hours) ? b : a)) : null
                      return (
                        <tr key={d.date}>
                          <td>{d.date}</td>
                          <td>{d.forecast_txns}</td>
                          <td>{d.forecast_sales}</td>
                          <td>{d.required_hours}</td>
                          <td>{peak ? `${String(peak.hour).padStart(2, '0')}:00 — ${peak.required_hours}h` : '—'}</td>
                        </tr>
                      )
                    })}
                  </tbody>
                </table>
              </>
            )}
            {forecastNote && (
              <p className="hint">Forecast unavailable: {forecastNote}</p>
            )}

            <h3>Shifts — {shifts.length} on this date</h3>
            <table>
              <thead><tr><th>Staff</th><th>Start (UTC)</th><th>End (UTC)</th><th>Status</th><th>Actions</th></tr></thead>
              <tbody>
                {shifts.map((s) => (
                  <tr key={s.id}>
                    <td>{s.staff_id.slice(0, 8)}</td>
                    <td>{s.starts_at.replace('T', ' ').slice(0, 16)}</td>
                    <td>{s.ends_at.replace('T', ' ').slice(0, 16)}</td>
                    <td>{s.status}</td>
                    <td>
                      {s.status !== 'confirmed' && (<button type="button" onClick={() => void doShiftStatus(s.id, 'confirmed')} disabled={busy}>Confirm</button>)}
                      {' '}
                      {s.status !== 'cancelled' && (<button type="button" onClick={() => void doShiftStatus(s.id, 'cancelled')} disabled={busy}>Cancel</button>)}
                    </td>
                  </tr>
                ))}
                {shifts.length === 0 && (
                  <tr><td colSpan={5}>No shifts on this date yet.</td></tr>
                )}
              </tbody>
            </table>

            <h3>New shift</h3>
            <div className="entry">
              <input value={shiftForm.staff_id} onChange={(e) => setShiftForm({ ...shiftForm, staff_id: e.target.value })} placeholder="Staff UUID" />
              <input type="datetime-local" value={shiftForm.starts_at} onChange={(e) => setShiftForm({ ...shiftForm, starts_at: e.target.value })} />
              <input type="datetime-local" value={shiftForm.ends_at} onChange={(e) => setShiftForm({ ...shiftForm, ends_at: e.target.value })} />
              <input value={shiftForm.template_id} onChange={(e) => setShiftForm({ ...shiftForm, template_id: e.target.value })} placeholder="Template UUID (optional)" />
              <input value={shiftForm.notes} onChange={(e) => setShiftForm({ ...shiftForm, notes: e.target.value })} placeholder="Notes (optional)" />
              <button type="button" onClick={() => void doCreateShift()} disabled={busy}>Create shift</button>
            </div>

            <h3>Shift templates — {templates.length}</h3>
            <table>
              <thead><tr><th>Name</th><th>Weekday</th><th>Hours</th><th>Role</th><th>Required</th></tr></thead>
              <tbody>
                {templates.map((t) => (
                  <tr key={t.id}>
                    <td>{t.name}</td>
                    <td>{['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'][t.weekday] ?? t.weekday}</td>
                    <td>{t.starts_at}–{t.ends_at}</td>
                    <td>{t.role ?? '—'}</td>
                    <td>{t.required_count}</td>
                  </tr>
                ))}
                {templates.length === 0 && (
                  <tr><td colSpan={5}>No templates yet.</td></tr>
                )}
              </tbody>
            </table>

            <h3>New template</h3>
            <div className="entry">
              <input value={templateForm.name} onChange={(e) => setTemplateForm({ ...templateForm, name: e.target.value })} placeholder="Name" />
              <select value={templateForm.weekday} onChange={(e) => setTemplateForm({ ...templateForm, weekday: e.target.value })}>
                {['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'].map((label, i) => (
                  <option key={label} value={String(i)}>{label}</option>
                ))}
              </select>
              <input value={templateForm.starts_at} onChange={(e) => setTemplateForm({ ...templateForm, starts_at: e.target.value })} placeholder="HH:MM" />
              <input value={templateForm.ends_at} onChange={(e) => setTemplateForm({ ...templateForm, ends_at: e.target.value })} placeholder="HH:MM" />
              <input value={templateForm.role} onChange={(e) => setTemplateForm({ ...templateForm, role: e.target.value })} placeholder="Role (optional)" />
              <input value={templateForm.staff_id} onChange={(e) => setTemplateForm({ ...templateForm, staff_id: e.target.value })} placeholder="Staff UUID (optional)" />
              <input value={templateForm.required_count} onChange={(e) => setTemplateForm({ ...templateForm, required_count: e.target.value })} placeholder="Required count" />
              <button type="button" onClick={() => void doCreateTemplate()} disabled={busy}>Create template</button>
            </div>

            <h3>Timeclock</h3>
            <div className="entry">
              <input value={clockStaffId} onChange={(e) => setClockStaffId(e.target.value)} placeholder="Staff UUID" />
              <button type="button" onClick={() => void doClockIn()} disabled={busy}>Clock in</button>
              <button type="button" onClick={() => void doClockOut()} disabled={busy}>Clock out</button>
            </div>
            <table>
              <thead><tr><th>Staff</th><th>Clock in (UTC)</th><th>Clock out (UTC)</th><th>Source</th></tr></thead>
              <tbody>
                {timeclock.map((entry) => (
                  <tr key={entry.id}>
                    <td>{entry.staff_id.slice(0, 8)}</td>
                    <td>{entry.clock_in.replace('T', ' ').slice(0, 16)}</td>
                    <td>{entry.clock_out ? entry.clock_out.replace('T', ' ').slice(0, 16) : 'open'}</td>
                    <td>{entry.source}</td>
                  </tr>
                ))}
                {timeclock.length === 0 && (
                  <tr><td colSpan={4}>No timeclock entries on this date yet.</td></tr>
                )}
              </tbody>
            </table>
          </section>
        )}
      </main>

      <footer>
        <p>Back office talks to the same edge database as the counter POS — stock, POs, transfers and counts are live, not demo data. Reorder points, transfers and counts persist in the edge database (migration 0005). Phase 4 adds CRM, delivery dispatch + POD, and vendor EDI on the same edge API — no mock data anywhere. Phase 6 adds a BI tab: KPI dashboards, eco-fee remittance (CSV download) and ad-hoc inquiries from the same ledgers, plus the scheduled-email outbox listing. Phase 6 also adds a Scheduling tab: shift templates, staff shifts, timeclock and the sales-driven labor forecast with its coverage check.</p>
      </footer>
    </div>
  )
}
