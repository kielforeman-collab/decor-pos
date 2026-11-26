// Mobile client: plain HTTP to the edge (Vite proxies `/api/*` to
// a `df-server edge` on 127.0.0.1:8080). No mocks — every call hits
// the real edge API. Request shapes match
// apps/web-backoffice/src/api.ts exactly where the endpoint is shared.
//
// Writes go through `mutate()`: while offline (or when the network
// fails) the mutation is queued in the localStorage outbox instead
// of being lost; see src/offline.ts for the idempotency caveat.

import { addToOutbox } from './offline'
import type {
  CrmActivity,
  DeliveryView,
  MobileKpis,
  PhysicalCount,
  PurchaseOrderView,
  SaleInput,
  Transfer,
} from './types'

async function http<T>(path: string, body?: unknown): Promise<T> {
  const res = await fetch(`/api${path}`, {
    method: body === undefined ? 'GET' : 'POST',
    headers: { 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  const json = await res.json()
  if (!res.ok) {
    throw new Error(typeof json?.error === 'string' ? json.error : `HTTP ${res.status}`)
  }
  return json as T
}

export type MutationResult<T> = { queued: true } | { queued: false; data: T }

/** POST, or queue in the outbox when offline / unreachable.
 * Server rejections (4xx/5xx with a body) throw and are NOT queued:
 * replaying a rejected write cannot make it valid. */
function queue(label: string, path: string, body: unknown): void {
  addToOutbox(label, path, body)
  window.dispatchEvent(new Event('decor-pos-outbox'))
}

async function mutate<T>(label: string, path: string, body: unknown): Promise<MutationResult<T>> {
  if (!navigator.onLine) {
    queue(label, path, body)
    return { queued: true }
  }
  try {
    const data = await http<T>(path, body)
    return { queued: false, data }
  } catch (err) {
    // A fetch TypeError means the request never reached the server
    // (edge down, Wi-Fi dropped): queue it. Anything else is a
    // server-side rejection — surface it, don't queue it.
    if (err instanceof TypeError) {
      queue(label, path, body)
      return { queued: true }
    }
    throw err
  }
}

export const DEMO_STORE_ID = '00000000-0000-0000-0000-00000000a000'
export const DEMO_ORG_ID = '00000000-0000-0000-0000-00000000b000'

export async function getHealth(): Promise<{ store_id: string; status: string }> {
  return http('/health')
}

// --- Outside Sales: KPIs ----------------------------------------------------

export async function getKpis(storeId: string): Promise<MobileKpis> {
  return http(`/mobile/kpis?store_id=${storeId}`)
}

// --- Warehouse: purchasing --------------------------------------------------

export async function listPurchaseOrders(storeId: string): Promise<PurchaseOrderView[]> {
  return http(`/purchasing/orders?store_id=${storeId}`)
}

export async function receivePurchaseOrder(
  poId: string,
  lines: { product_id: string; qty: string }[],
): Promise<MutationResult<PurchaseOrderView>> {
  return mutate(`PO receive ${poId.slice(0, 8)}`, `/purchasing/orders/${poId}/receive`, { lines })
}

// --- Warehouse: counts ------------------------------------------------------

export async function startCount(storeId: string): Promise<MutationResult<PhysicalCount>> {
  return mutate('Start count', '/inventory/counts', { store_id: storeId })
}

export async function listCounts(storeId: string): Promise<PhysicalCount[]> {
  return http(`/inventory/counts?store_id=${storeId}`)
}

export async function scanCount(
  id: string,
  productId: string,
  countedQty: string,
): Promise<MutationResult<PhysicalCount>> {
  return mutate(`Count scan ${id.slice(0, 8)}`, `/inventory/counts/${id}/scan`, {
    product_id: productId,
    counted_qty: countedQty,
  })
}

export async function reconcileCount(
  id: string,
): Promise<
  MutationResult<{
    count_id: string
    adjustments: { product_id: string; on_hand_before: string; counted_qty: string; adjustment: string }[]
  }>
> {
  return mutate(`Count reconcile ${id.slice(0, 8)}`, `/inventory/counts/${id}/reconcile`, {})
}

// --- Warehouse: transfers ---------------------------------------------------

export async function createTransfer(body: {
  from_store_id: string
  to_store_id: string
  lines: { product_id: string; qty: string; unit_cost: string }[]
}): Promise<MutationResult<Transfer>> {
  return mutate('Create transfer', '/inventory/transfers', body)
}

export async function listTransfers(storeId: string): Promise<Transfer[]> {
  return http(`/inventory/transfers?store_id=${storeId}`)
}

export async function shipTransfer(id: string): Promise<MutationResult<Transfer>> {
  return mutate(`Ship transfer ${id.slice(0, 8)}`, `/inventory/transfers/${id}/ship`, {})
}

export async function receiveTransfer(id: string): Promise<MutationResult<Transfer>> {
  return mutate(`Receive transfer ${id.slice(0, 8)}`, `/inventory/transfers/${id}/receive`, {})
}

// --- Outside Sales: CRM -----------------------------------------------------

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
}): Promise<MutationResult<CrmActivity>> {
  return mutate(`Activity: ${body.subject.slice(0, 40)}`, '/crm/activities', body)
}

// --- Outside Sales: quotes --------------------------------------------------

export async function recordQuote(input: SaleInput): Promise<MutationResult<{ transaction_id: string }>> {
  return mutate('Record quote', '/pos/quotes', input)
}

// --- Delivery Driver --------------------------------------------------------

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

export async function setDeliveryStatus(
  id: string,
  body: { status: string; actor_user_id?: string | null; note?: string | null },
): Promise<MutationResult<DeliveryView>> {
  return mutate(`Delivery ${id.slice(0, 8)} → ${body.status}`, `/deliveries/${id}/status`, body)
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
): Promise<MutationResult<DeliveryView>> {
  return mutate(`POD ${id.slice(0, 8)}`, `/deliveries/${id}/pod`, body)
}

/** SHA-256 hex of a file's bytes, computed in-browser (WebCrypto). */
export async function sha256Hex(bytes: ArrayBuffer): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-256', bytes)
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, '0')).join('')
}
