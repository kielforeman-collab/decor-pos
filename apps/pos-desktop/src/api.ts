// Edge client with two backends, one interface.
//
// * Native Tauri app: `window.__TAURI_INTERNALS__` is present and
//   calls go through `invoke()` to the in-process Edge (same JSON
//   shapes as the HTTP API). Provider switching and the captured
//   receipt text only exist here.
// * Dev path (plain browser): calls go to `/api/*`, proxied by Vite
//   to a `df-server edge` on 127.0.0.1:8080. The server's provider is
//   whatever it was started with (mock by default), so
//   `providerSwitchable` is false and `lastReceiptText()` is null —
//   the receipt preview is rendered from the checkout response.

import { DEMO_CATALOG, findByBarcode } from './catalog'
import {
  UnauthorizedError,
  authHeaders,
  clearSession,
  notifyUnauthorized,
} from './auth'
import { parseScannerInput } from './scan'
import type {
  ApplyReport,
  CompletedSale,
  Money,
  PricePreview,
  SaleInput,
  ScanResult,
  SyncState,
  TenderInput,
  TillSession,
  ZReport,
} from './types'

export const isTauri =
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

type InvokeFn = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>

let invokeFn: Promise<InvokeFn> | null = null

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!invokeFn) {
    invokeFn = import('@tauri-apps/api/core').then((m) => m.invoke as InvokeFn)
  }
  return (await invokeFn)(cmd, args)
}

async function http<T>(path: string, body?: unknown): Promise<T> {
  const res = await fetch(`/api${path}`, {
    method: body === undefined ? 'GET' : 'POST',
    // Phase 7: every call carries the session token. In Tauri mode the
    // invoke() calls below cannot carry a Bearer token (local IPC, not
    // HTTP) — enforcement for those is the Tauri command layer's job,
    // not this wrapper's; the login screen still gates the UI.
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

export const providerSwitchable = isTauri

export async function getStatus(): Promise<{
  store_name: string
  provider: string
  pending_outbox: number
}> {
  if (isTauri) return invoke('get_status')
  const health = await http<{ store_id: string; status: string }>('/health')
  const sync = await http<SyncState>('/pos/sync/state')
  return {
    store_name: `Store ${health.store_id.slice(0, 8)}`,
    provider: 'mock (df-server edge)',
    pending_outbox: sync.pending_count,
  }
}

export async function setProvider(name: string): Promise<string> {
  if (!isTauri) throw new Error('Provider switching needs the native Tauri app')
  return invoke('set_provider', { name })
}

export async function pricePreview(input: SaleInput): Promise<PricePreview> {
  if (isTauri) return invoke('price_preview', { input })
  return http('/pos/carts/price-preview', input)
}

export async function checkout(input: SaleInput): Promise<CompletedSale> {
  if (isTauri) return invoke('checkout', { input })
  return http('/pos/transactions', input)
}

export async function openTill(
  cashier_id: string,
  opening_float: Money,
): Promise<TillSession> {
  if (isTauri) return invoke('open_till', { cashier_id, opening_float })
  return http('/pos/tills/open', { cashier_id, opening_float })
}

export async function closeTill(till_id: string, counted_cash: Money): Promise<ZReport> {
  if (isTauri) return invoke('close_till', { till_id, counted_cash })
  return http(`/pos/tills/${till_id}/close`, { counted_cash })
}

export async function parkHold(input: SaleInput): Promise<{ hold_id: string }> {
  if (isTauri) {
    const hold_id = await invoke<string>('park_hold', { input })
    return { hold_id }
  }
  return http('/pos/holds', input)
}

export async function listHolds(): Promise<{ hold_ids: string[] }> {
  if (isTauri) {
    const hold_ids = await invoke<string[]>('list_holds')
    return { hold_ids }
  }
  return http('/pos/holds')
}

export async function resumeHold(hold_id: string): Promise<SaleInput> {
  if (isTauri) return invoke('resume_hold', { hold_id })
  return http(`/pos/holds/${hold_id}/resume`, {})
}

export async function recordQuote(input: SaleInput): Promise<{ transaction_id: string }> {
  if (isTauri) {
    const transaction_id = await invoke<string>('record_quote', { input })
    return { transaction_id }
  }
  return http('/pos/quotes', input)
}

export async function recordLayaway(
  sale: SaleInput,
  deposit: TenderInput,
): Promise<{ transaction_id: string }> {
  if (isTauri) {
    const transaction_id = await invoke<string>('record_layaway', {
      input: sale,
      deposit,
    })
    return { transaction_id }
  }
  return http('/pos/layaways', { sale, deposit })
}

export async function recordReturn(
  transaction_id: string,
  lines: { product_id: string; qty: string }[],
): Promise<CompletedSale> {
  if (isTauri) return invoke('record_return', { original_tx_id: transaction_id, lines })
  return http(`/pos/transactions/${transaction_id}/return`, lines)
}

export async function voidTransaction(
  transaction_id: string,
): Promise<{ transaction_id: string; status: string }> {
  if (isTauri) return invoke('void_transaction', { id: transaction_id })
  return http(`/pos/transactions/${transaction_id}/void`, {})
}

export async function syncState(): Promise<SyncState> {
  if (isTauri) return invoke('sync_state')
  return http('/pos/sync/state')
}

export async function syncRun(): Promise<ApplyReport> {
  if (isTauri) return invoke('sync_now')
  return http('/pos/sync/run', {})
}

/** Last receipt text captured by the mock printer (Tauri only). */
export async function lastReceiptText(): Promise<string | null> {
  if (!isTauri) return null
  return invoke('last_receipt_text')
}

/**
 * Scan one barcode input. Native: the real df-hw parser in Rust.
 * Dev path: the TS mirror in scan.ts against the demo catalog.
 */
export async function scanBarcode(input: string): Promise<ScanResult> {
  if (isTauri) {
    const catalog = DEMO_CATALOG.map((p) => ({
      barcode: p.barcode,
      uom_code: p.uom,
      qty_multiplier: '1',
    }))
    return invoke('scan_barcode', { input, catalog })
  }
  const parsed = parseScannerInput(input)
  if (!parsed) throw new Error('Empty barcode')
  const product = findByBarcode(parsed.barcode)
  if (!product) throw new Error(`Unknown barcode: ${parsed.barcode}`)
  return { barcode: parsed.barcode, qty: String(parsed.qty), uom_code: product.uom }
}
