// Phase 7 (v1.1) — login screen. Plain, dense, monospace, white on
// black text, per the agreed POS direction: no teal, no gradients, no
// decorative graphics. Email + password, plus a fast PIN / badge
// counter-switch for the till (POST /pos/auth/counter-switch).

import { useState } from 'react'
import { counterSwitch, login, type AuthSession } from './auth'

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

  async function doSwitch() {
    if (!pin.trim() && !badge.trim()) {
      setError('Enter a PIN or scan a badge to switch operator.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      onLoggedIn(
        await counterSwitch({
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
