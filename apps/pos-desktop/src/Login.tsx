// Phase 7 (v1.1) — login screen. Plain, dense, monospace, white on
// black text, per the agreed POS direction: no teal, no gradients, no
// decorative graphics. Email + password, plus a fast PIN / badge
// counter-switch for the till (POST /pos/auth/counter-switch).
//
// First-run setup: on mount this screen calls GET /pos/auth/setup-status
// first. When the users table is empty (`needs_setup: true`) it shows
// the "Create your admin account" form INSTEAD of the login form; on
// success the new admin is signed straight in (same localStorage keys
// as login, no second step). When `needs_setup` is false — or the
// status call itself fails — this is exactly the existing login screen.

import { useEffect, useState } from 'react'
import { counterSwitch, getSetupStatus, login, setupAdmin, type AuthSession } from './auth'

export default function Login({
  title,
  subtitle,
  allowPin,
  onLoggedIn,
  onCancel,
}: {
  title: string
  subtitle: string
  allowPin: boolean
  onLoggedIn: (session: AuthSession) => void
  onCancel?: () => void
}) {
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [pin, setPin] = useState('')
  const [badge, setBadge] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  // First-run setup state: null = status check still in flight,
  // true = fresh install, no users yet → show the setup form,
  // false = normal login. A failed status call resolves to false,
  // so login is never blocked by the check.
  const [needsSetup, setNeedsSetup] = useState<boolean | null>(null)
  const [displayName, setDisplayName] = useState('')
  const [confirmPassword, setConfirmPassword] = useState('')

  useEffect(() => {
    let cancelled = false
    void getSetupStatus().then((needed) => {
      if (!cancelled) setNeedsSetup(needed)
    })
    return () => {
      cancelled = true
    }
  }, [])

  async function doLogin() {
    if (!email.trim() || !password) {
      setError('Email and password are required.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      onLoggedIn(await login(email.trim(), password))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  async function doSetup() {
    if (!email.trim() || !displayName.trim() || !password || !confirmPassword) {
      setError('Email, display name, password and confirm password are all required.')
      return
    }
    if (password !== confirmPassword) {
      setError('Passwords do not match.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      onLoggedIn(await setupAdmin(email.trim(), displayName.trim(), password))
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e)
      // 409 = an admin already exists (someone else set one up first):
      // fall through to the normal login form instead of stranding the
      // user on a setup form that can never succeed.
      if ((e as { status?: unknown }).status === 409) {
        setNeedsSetup(false)
        setError('An admin account already exists — sign in instead.')
        return
      }
      setError(message)
    } finally {
      setBusy(false)
    }
  }

  async function doSwitch() {
    if (!email.trim()) {
      setError("Enter the new operator's email, then their PIN or badge.")
      return
    }
    if (!pin.trim() && !badge.trim()) {
      setError('Enter a PIN or scan a badge to switch operator.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      onLoggedIn(
        await counterSwitch({
          email: email.trim(),
          pin: pin.trim() || undefined,
          badgeToken: badge.trim() || undefined,
        }),
      )
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  // Status check in flight: say so, briefly, inside the same box.
  // The login form is not rendered yet, and the check resolving to
  // false (including on failure) renders it — login is never blocked.
  if (needsSetup === null) {
    return (
      <div className="login">
        <h2>{title}</h2>
        <p className="hint">Checking first-run status…</p>
      </div>
    )
  }

  if (needsSetup) {
    return (
      <div className="login">
        <h2>Create your admin account</h2>
        <p className="hint">
          This is a fresh install with no users yet. Create the first
          admin account — you will be signed straight in.
        </p>
        <label>
          Email
          <input
            type="email"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void doSetup()
            }}
            placeholder="admin@store.example"
            autoComplete="username"
            autoFocus
          />
        </label>
        <label>
          Display name
          <input
            value={displayName}
            onChange={(e) => setDisplayName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void doSetup()
            }}
            placeholder="Admin"
            autoComplete="name"
          />
        </label>
        <label>
          Password
          <input
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void doSetup()
            }}
            autoComplete="new-password"
          />
        </label>
        <label>
          Confirm password
          <input
            type="password"
            value={confirmPassword}
            onChange={(e) => setConfirmPassword(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void doSetup()
            }}
            autoComplete="new-password"
          />
        </label>
        <div className="actions">
          <button type="button" onClick={() => void doSetup()} disabled={busy}>
            {busy ? 'Creating…' : 'Create admin account'}
          </button>
          {onCancel && (
            <button type="button" onClick={onCancel} disabled={busy}>
              Cancel
            </button>
          )}
        </div>
        {error && (
          <div className="error" role="alert">
            {error}
          </div>
        )}
        <p className="hint">
          Session token is stored in this browser&apos;s localStorage
          (key df-pos-token) — v1.1 choice, not an httpOnly cookie.
        </p>
      </div>
    )
  }

  return (
    <div className="login">
      <h2>{title}</h2>
      <p className="hint">{subtitle}</p>
      <label>
        Email
        <input
          type="email"
          value={email}
          onChange={(e) => setEmail(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void doLogin()
          }}
          placeholder="cashier@store.example"
          autoComplete="username"
          autoFocus
        />
      </label>
      <label>
        Password
        <input
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void doLogin()
          }}
          autoComplete="current-password"
        />
      </label>
      <div className="actions">
        <button type="button" onClick={() => void doLogin()} disabled={busy}>
          {busy ? 'Signing in…' : 'Sign in'}
        </button>
        {onCancel && (
          <button type="button" onClick={onCancel} disabled={busy}>
            Cancel
          </button>
        )}
      </div>
      {allowPin && (
        <>
          <h3>Switch operator — PIN / badge</h3>
          <p className="hint">
            Fast till switch: the new operator signs in with their counter
            PIN or badge. The previous session is replaced.
          </p>
          <label>
            Counter PIN
            <input
              value={pin}
              onChange={(e) => setPin(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void doSwitch()
              }}
              placeholder="4–6 digit PIN"
              inputMode="numeric"
              autoComplete="off"
            />
          </label>
          <label>
            Badge token
            <input
              value={badge}
              onChange={(e) => setBadge(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void doSwitch()
              }}
              placeholder="Scan badge (optional, instead of PIN)"
              autoComplete="off"
            />
          </label>
          <div className="actions">
            <button type="button" onClick={() => void doSwitch()} disabled={busy}>
              {busy ? 'Switching…' : 'Switch operator'}
            </button>
          </div>
        </>
      )}
      {error && (
        <div className="error" role="alert">
          {error}
        </div>
      )}
      <p className="hint">
        Session token is stored in this browser&apos;s localStorage
        (key df-pos-token) — v1.1 choice, not an httpOnly cookie.
      </p>
    </div>
  )
}
