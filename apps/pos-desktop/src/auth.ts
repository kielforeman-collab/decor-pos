// Phase 7 (v1.1) — session auth for the counter POS.
//
// Contract (backend endpoints are NOT in the tree yet — the sibling
// auth-core worker owns them; this client is built against the agreed
// contract, see below):
//   POST /pos/auth/login          { email, password }
//   POST /pos/auth/counter-switch { pin?, badge_token? }
//   POST /pos/auth/logout         {}
//   POST /pos/auth/override-verify { user_id, pin, reason_code }
// All four are reached through the Vite `/api` proxy, i.e. the client
// calls `/api/pos/auth/*` and the proxy strips `/api`.
// Expected success shape:
//   { token: "dfp_...", user: { id, email, display_name?, role, customer_id? } }
// The normalizer also accepts `access_token` / `session_token` for the
// token and `staff` / `account` for the user object, in case the backend
// names them differently — if the backend shape differs from this, the
// normalizer (not the call sites) is the one place to fix.
// The HQ equivalents under `/api/v1/auth/*` are NOT used here: this app
// has no HQ base URL (everything proxies to the edge on :8080).
//
// Storage: the session token lives in localStorage under `df-pos-token`
// (key shared verbatim by all four Phase 7 apps) and the session user
// under `df-pos-session`. That is the v1.1 choice, recorded honestly:
// localStorage tokens are readable by any script running on this origin,
// unlike httpOnly cookies — acceptable for a same-origin counter app in
// v1.1, but a cookie-based session is the follow-up before this POS is
// exposed beyond the store network.

export const TOKEN_KEY = 'df-pos-token'
export const SESSION_KEY = 'df-pos-session'

export interface SessionUser {
  id: string
  email: string
  display_name: string | null
  /** Role string as the backend sends it, e.g. "Cashier" / "Manager". */
  role: string
  /** Portal contractor only: the customer id the session is scoped to. */
  customer_id: string | null
}

export interface AuthSession {
  token: string
  user: SessionUser
}

/** Thrown (instead of a plain Error) when the backend answers 401. */
export class UnauthorizedError extends Error {
  readonly status = 401
  constructor(message = 'Session expired — sign in again.') {
    super(message)
    this.name = 'UnauthorizedError'
  }
}

export function getToken(): string | null {
  try {
    return window.localStorage.getItem(TOKEN_KEY)
  } catch {
    return null
  }
}

export function getSession(): AuthSession | null {
  try {
    const raw = window.localStorage.getItem(SESSION_KEY)
    if (!raw) return null
    const parsed = JSON.parse(raw) as AuthSession
    if (typeof parsed?.token !== 'string' || typeof parsed?.user?.id !== 'string') return null
    return parsed
  } catch {
    return null
  }
}

export function saveSession(session: AuthSession): void {
  window.localStorage.setItem(TOKEN_KEY, session.token)
  window.localStorage.setItem(SESSION_KEY, JSON.stringify(session))
}

export function clearSession(): void {
  window.localStorage.removeItem(TOKEN_KEY)
  window.localStorage.removeItem(SESSION_KEY)
}

/** Extra headers for an authenticated call (empty when logged out). */
export function authHeaders(): Record<string, string> {
  const token = getToken()
  return token ? { authorization: `Bearer ${token}` } : {}
}

// --- 401 fan-out -----------------------------------------------------------
// api.ts calls notifyUnauthorized() on a 401; App listens for the
// `decor-pos-unauthorized` window event (and/or registers a callback)
// and returns to the login screen. Login/logout/override calls use
// authPost() below, which deliberately does NOT call this — a 401 from
// /pos/auth/login is a wrong password, not an expired session.

type UnauthorizedListener = () => void
let unauthorizedListener: UnauthorizedListener | null = null

export function setUnauthorizedListener(fn: UnauthorizedListener | null): void {
  unauthorizedListener = fn
}

export function notifyUnauthorized(): void {
  unauthorizedListener?.()
  window.dispatchEvent(new Event('decor-pos-unauthorized'))
}

// --- auth endpoint calls ---------------------------------------------------

interface ErrorWithStatus extends Error {
  status?: number
}

async function authPost(path: string, body: unknown): Promise<unknown> {
  const res = await fetch(`/api${path}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', ...authHeaders() },
    body: JSON.stringify(body),
  })
  const json: unknown = await res.json().catch(() => null)
  if (!res.ok) {
    const message =
      typeof (json as { error?: unknown } | null)?.error === 'string'
        ? (json as { error: string }).error
        : `HTTP ${res.status}`
    const err = new Error(message) as ErrorWithStatus
    err.status = res.status
    throw err
  }
  return json
}

/**
 * Accept the contracted shape plus the two most likely renames.
 * Unknown/missing role defaults to Cashier (least privilege) rather
 * than failing the login — the backend still enforces RBAC.
 */
export function normalizeSession(json: unknown): AuthSession {
  const obj = (json ?? {}) as Record<string, unknown>
  const token = [obj.token, obj.access_token, obj.session_token].find(
    (t): t is string => typeof t === 'string' && t.length > 0,
  )
  if (!token) throw new Error('Login response did not include a session token.')
  // The real backend (df-auth-http LoginResponse) is FLAT — {token, user_id,
  // email, display_name, role, customer_id, expires_at} — with no nested
  // `user` object. Fall back to the top-level object itself so the flat
  // shape parses; the nested shapes stay accepted as before.
  const rawUser = (obj.user ?? obj.staff ?? obj.account ?? obj) as Record<string, unknown>
  const id = [rawUser.id, rawUser.user_id].find(
    (v): v is string => typeof v === 'string' && v.length > 0,
  )
  if (!id) throw new Error('Login response did not include a user id.')
  const role =
    typeof rawUser.role === 'string' && rawUser.role
      ? rawUser.role
      : typeof obj.role === 'string' && obj.role
        ? obj.role
        : 'Cashier'
  return {
    token,
    user: {
      id,
      email: typeof rawUser.email === 'string' ? rawUser.email : '',
      display_name:
        typeof rawUser.display_name === 'string'
          ? rawUser.display_name
          : typeof rawUser.name === 'string'
            ? rawUser.name
            : null,
      role,
      customer_id:
        typeof rawUser.customer_id === 'string'
          ? rawUser.customer_id
          : typeof obj.customer_id === 'string'
            ? obj.customer_id
            : null,
    },
  }
}

export async function login(email: string, password: string): Promise<AuthSession> {
  const session = normalizeSession(await authPost('/pos/auth/login', { email, password }))
  saveSession(session)
  return session
}

// --- first-run setup -------------------------------------------------------
// Contract (backend worker owns the endpoints; this client is built
// against the agreed contract):
//   GET  /pos/auth/setup-status  → { needs_setup: true/false }
//     (true only when the users table is empty)
//   POST /pos/auth/setup  { email, display_name, password }
//     → the same LoginResponse shape as login; the created user is an
//     Admin. Second call when users exist → 409. Empty
//     email/password → 400. No role parameter is accepted.
// Both are reached through the Vite `/api` proxy like the other auth
// calls above.

/**
 * True when this is a fresh install with no users yet. Any failure
 * (server down, unexpected shape) returns false — the caller falls
 * back to the normal login form and login is never blocked by this
 * check.
 */
export async function getSetupStatus(): Promise<boolean> {
  try {
    const res = await fetch('/api/pos/auth/setup-status')
    if (!res.ok) return false
    const json: unknown = await res.json().catch(() => null)
    return (json as { needs_setup?: unknown } | null)?.needs_setup === true
  } catch {
    return false
  }
}

/**
 * Create the first (admin) account and sign straight in: the returned
 * session is saved exactly like `login()` does (same localStorage
 * keys), so there is no second sign-in step.
 */
export async function setupAdmin(
  email: string,
  displayName: string,
  password: string,
): Promise<AuthSession> {
  const session = normalizeSession(
    await authPost('/pos/auth/setup', {
      email,
      display_name: displayName,
      password,
    }),
  )
  saveSession(session)
  return session
}

export async function counterSwitch(opts: {
  pin?: string
  badgeToken?: string
}): Promise<AuthSession> {
  const body: Record<string, string> = {}
  if (opts.pin) body.pin = opts.pin
  if (opts.badgeToken) body.badge_token = opts.badgeToken
  const session = normalizeSession(await authPost('/pos/auth/counter-switch', body))
  saveSession(session)
  return session
}

/** Best-effort server logout; the local session is cleared regardless. */
export async function logout(): Promise<void> {
  try {
    await authPost('/pos/auth/logout', {})
  } catch {
    // A failed logout must not strand the operator on a signed-in UI.
  } finally {
    clearSession()
  }
}

/**
 * Manager override check. `reasonCode` is 'void' or 'price_override'
 * (assumed values — the backend defines the accepted set). A 401/403
 * answer means "not verified"; other failures are thrown.
 */
export async function verifyOverride(
  userId: string,
  pin: string,
  reasonCode: string,
): Promise<boolean> {
  try {
    const json = (await authPost('/pos/auth/override-verify', {
      user_id: userId,
      pin,
      reason_code: reasonCode,
    })) as Record<string, unknown> | null
    if (json && (json.ok === false || json.verified === false || json.approved === false)) {
      return false
    }
    return true
  } catch (err) {
    if ((err as ErrorWithStatus).status === 401 || (err as ErrorWithStatus).status === 403) {
      return false
    }
    throw err
  }
}

// --- role helpers (df-auth RBAC: only Manager/Admin hold PosVoid and
// PosPriceOverride). UI-side only — the backend still enforces. --------

function normalizedRole(role: string): string {
  return role.trim().toLowerCase()
}

export function canVoid(role: string): boolean {
  const r = normalizedRole(role)
  return r === 'manager' || r === 'admin'
}

export function canPriceOverride(role: string): boolean {
  const r = normalizedRole(role)
  return r === 'manager' || r === 'admin'
}
