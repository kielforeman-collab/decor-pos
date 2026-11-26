// Contractor portal client: plain HTTP to the edge (Vite proxies
// `/api/*` to a `df-server edge` on 127.0.0.1:8080). No mocks —
// every call hits the real edge API.
//
// Phase 7: the portal is signed in with email + password (POST
// /pos/auth/login). The login response carries the contractor's
// customer id; that session-derived id — never a `?customer=` URL
// parameter or a stored customer id — is the scope for every call,
// and every call carries `Authorization: Bearer <token>` (localStorage
// key `df-pos-token`, same as the other Phase 7 apps). A 401 clears
// the session and returns to the login screen. The backend scopes all
// portal data to the logged-in contractor server-side.

import {
  UnauthorizedError,
  authHeaders,
  clearSession,
  notifyUnauthorized,
} from './auth'
import type {
  BillPayResult,
  Money,
  PortalCustomerView,
  PortalPaymentRow,
  PortalReorderResponse,
  StatementRow,
  TintFormula,
  Transaction,
} from './types'

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

export async function getPortalCustomer(customerId: string): Promise<PortalCustomerView> {
  return http(`/portal/customers/${customerId}`)
}

export async function getPortalInvoices(customerId: string): Promise<Transaction[]> {
  return http(`/portal/customers/${customerId}/invoices`)
}

export async function getPortalStatements(customerId: string): Promise<StatementRow[]> {
  return http(`/portal/customers/${customerId}/statements`)
}

export async function getPortalFormulas(customerId: string): Promise<TintFormula[]> {
  return http(`/portal/customers/${customerId}/formulas`)
}

export async function getPortalPayments(customerId: string): Promise<PortalPaymentRow[]> {
  return http(`/portal/customers/${customerId}/payments`)
}

export async function createReorder(body: {
  customer_id: string
  formula_id: string
  qty: string
  job_id: string | null
  fulfillment: 'Pickup' | 'Delivery'
}): Promise<PortalReorderResponse> {
  return http('/portal/reorders', body)
}

export async function payBill(body: {
  customer_id: string
  amount: Money
  method: 'card' | 'ach'
  idempotency_key: string
}): Promise<BillPayResult> {
  return http('/portal/payments', body)
}
