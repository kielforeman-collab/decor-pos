// Contractor portal client: plain HTTP to the edge (Vite proxies
// `/api/*` to a `df-server edge` on 127.0.0.1:8080). No mocks —
// every call hits the real edge API. There is no separate portal
// login: the customer id IS the scope, and every call re-checks
// `portal_enabled` server-side (403 otherwise).

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
    headers: { 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  const json = await res.json()
  if (!res.ok) {
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
