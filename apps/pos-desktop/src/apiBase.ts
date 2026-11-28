// Shared base URL resolver for the pos-desktop HTTP client.
//
// * Browser / Vite dev mode: requests go to `/api<path>` (proxied by
//   Vite to `df-server edge` on 127.0.0.1:8080).
// * Installed native Tauri desktop app: queries the embedded loopback
//   `df-edge` HTTP server URL (`http://127.0.0.1:<port>`) once via
//   `invoke('get_api_base_url')` and caches it, so every screen
//   (counter, back-office, Sync, and Admin) reaches the full
//   RBAC-gated edge API identically with zero proxy required.

export const isTauri =
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

type InvokeFn = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>

let invokeFn: Promise<InvokeFn> | null = null
let baseUrlPromise: Promise<string> | null = null

export async function tauriInvoke<T>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  if (!invokeFn) {
    invokeFn = import('@tauri-apps/api/core').then((m) => m.invoke as InvokeFn)
  }
  return (await invokeFn)(cmd, args)
}

/**
 * Resolve the base URL prefix (`'/api'` in browser mode, or
 * `'http://127.0.0.1:<port>'` in the native Tauri shell).
 */
export async function getApiBase(): Promise<string> {
  if (!isTauri) return '/api'
  if (!baseUrlPromise) {
    baseUrlPromise = tauriInvoke<string>('get_api_base_url').catch((err) => {
      // Clear cached promise so a transient startup race can retry.
      baseUrlPromise = null
      throw err
    })
  }
  return baseUrlPromise
}

/** Build the full URL for an edge API path (must start with `'/'`). */
export async function apiUrl(path: string): Promise<string> {
  const base = await getApiBase()
  return `${base}${path}`
}
