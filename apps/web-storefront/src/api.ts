// Storefront client: plain HTTP to the edge (Vite proxies
// `/api/*` to a `df-server edge` on 127.0.0.1:8080). No mocks —
// every call hits the real edge API. Guest checkout: no login,
// no card fields — checkout is authorized by the mock provider
// in dev (no live payment API, no card data stored).

import type { BopisOrderResponse, BopisOrderView, CatalogProduct } from './types'

async function http<T>(path: string, body?: unknown): Promise<T> {
  const res = await fetch(`/api${path}`, {
    method: body === undefined ? 'GET' : 'POST',
    headers: { 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  const json = await res.json().catch(() => null)
  if (!res.ok) {
    throw new Error(
      typeof json?.error === 'string'
        ? json.error
        : typeof json?.message === 'string'
          ? json.message
          : `HTTP ${res.status}`,
    )
  }
  return json as T
}

export async function getHealth(): Promise<{ store_id: string; status: string }> {
  return http('/health')
}

export async function getCatalog(): Promise<CatalogProduct[]> {
  return http('/catalog/products')
}

export async function placeBopisOrder(body: {
  lines: { product_id: string; qty: string }[]
  pickup_name: string
  contact: string
  scheduled_at: string
}): Promise<BopisOrderResponse> {
  return http('/bopis/orders', body)
}

export async function getBopisOrder(orderId: string): Promise<BopisOrderView> {
  return http(`/bopis/orders/${encodeURIComponent(orderId)}`)
}

export async function completePickup(orderId: string): Promise<BopisOrderView> {
  return http(`/bopis/orders/${encodeURIComponent(orderId)}/complete-pickup`, {})
}
