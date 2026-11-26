// Phase 7 (v1.1) — manager override prompt.
//
// There was no override UI before Phase 7: voiding called the void
// endpoint directly and a line price could be edited below cost with no
// check. The prompt collects the MANAGER's user id + counter PIN and
// verifies them against POST /pos/auth/override-verify with the
// reason code for the action ('void' or 'price_override' — assumed
// values; the backend defines the accepted set). The prompt is UI-side
// only: the backend still enforces who may void / override a price,
// so a forged client-side approval cannot void a sale server-side.

import { useState } from 'react'
import { verifyOverride } from './auth'

export default function OverridePrompt({
  label,
  reasonCode,
  onVerified,
  onCancel,
}: {
  label: string
  reasonCode: 'void' | 'price_override'
  onVerified: () => void
  onCancel: () => void
}) {
  const [managerId, setManagerId] = useState('')
  const [pin, setPin] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  async function doVerify() {
    if (!managerId.trim() || !pin) {
      setError('Manager user id and PIN are required.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const ok = await verifyOverride(managerId.trim(), pin, reasonCode)
      if (!ok) {
        setError('Override not verified — check the manager id and PIN.')
        return
      }
      onVerified()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="login" role="dialog" aria-label="Manager override">
      <h2>Manager override required</h2>
      <p className="hint">{label}</p>
      <label>
        Manager user id
        <input
          value={managerId}
          onChange={(e) => setManagerId(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void doVerify()
          }}
          placeholder="Manager's user id"
          autoComplete="off"
          autoFocus
        />
      </label>
      <label>
        Manager PIN
        <input
          type="password"
          value={pin}
          onChange={(e) => setPin(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void doVerify()
          }}
          inputMode="numeric"
          autoComplete="off"
        />
      </label>
      <div className="actions">
        <button type="button" onClick={() => void doVerify()} disabled={busy}>
          {busy ? 'Verifying…' : 'Verify override'}
        </button>
        <button type="button" onClick={onCancel} disabled={busy}>
          Cancel
        </button>
      </div>
      {error && (
        <div className="error" role="alert">
          {error}
        </div>
      )}
    </div>
  )
}
