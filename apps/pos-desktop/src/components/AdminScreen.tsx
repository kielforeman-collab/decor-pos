// Admin — Store & tax jurisdiction settings, Staff & users CRUD (with
// password/PIN/badge rotation and session revocation), and dfs_ store
// service token management. Uses existing pos-desktop CSS tokens so all
// themes (mono, dark, expanse, lcars, tos, custom) stay consistent.

import { useCallback, useEffect, useState } from 'react'
import { listStores, type Store } from '../api'
import {
  createAdminServiceToken,
  createAdminUser,
  getStoreSettings,
  listAdminServiceTokens,
  listAdminUsers,
  revokeAdminServiceToken,
  revokeAdminUserSessions,
  updateAdminUser,
  updateStoreSettings,
} from '../backofficeApi'
import type {
  AdminUserSummary,
  CreateServiceTokenResponse,
  ServiceTokenSummary,
  StoreProfile,
  StoreSettingsResponse,
} from '../backofficeTypes'

type AdminTab = 'store' | 'users' | 'tokens'

const ROLES = [
  'Cashier',
  'SalesAssociate',
  'Warehouse',
  'Driver',
  'Manager',
  'Admin',
  'PortalContractor',
] as const

const COMMON_TIMEZONES = [
  'America/Vancouver',
  'America/Edmonton',
  'America/Winnipeg',
  'America/Toronto',
  'America/Halifax',
  'America/Los_Angeles',
  'America/Denver',
  'America/Chicago',
  'America/New_York',
  'UTC',
]

function fmtRatePct(decimalStr: string): string {
  const n = Number(decimalStr)
  if (!Number.isFinite(n)) return decimalStr
  return `${(n * 100).toFixed(3).replace(/\.?0+$/, '')}%`
}

function fmtTime(iso: string | null): string {
  if (!iso) return '—'
  const d = new Date(iso)
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString()
}

export default function AdminScreen({
  currentUserId,
  userRole,
  storeId,
  onStoreUpdated,
  onToast,
}: {
  currentUserId: string
  userRole: string
  storeId: string | null
  onStoreUpdated: (store: StoreProfile) => void
  onToast: (msg: string) => void
}) {
  const isAdmin = userRole === 'Admin'
  const canReadStore = isAdmin || userRole === 'Manager'

  const [tab, setTab] = useState<AdminTab>('store')
  const [loading, setLoading] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // --- Store & tax tab ---
  const [storeResp, setStoreResp] = useState<StoreSettingsResponse | null>(null)
  const [storeCode, setStoreCode] = useState('')
  const [storeName, setStoreName] = useState('')
  const [countryCode, setCountryCode] = useState('')
  const [regionCode, setRegionCode] = useState('')
  const [postalCode, setPostalCode] = useState('')
  const [currency, setCurrency] = useState('USD')
  const [timezone, setTimezone] = useState('UTC')
  const [isWarehouse, setIsWarehouse] = useState(false)

  // --- Staff & users tab ---
  const [users, setUsers] = useState<AdminUserSummary[]>([])
  const [selectedUserId, setSelectedUserId] = useState<string | null>(null)
  const [creatingUser, setCreatingUser] = useState(false)
  // New user form
  const [newEmail, setNewEmail] = useState('')
  const [newName, setNewName] = useState('')
  const [newRole, setNewRole] = useState<string>('Cashier')
  const [newPassword, setNewPassword] = useState('')
  const [newPin, setNewPin] = useState('')
  const [newBadge, setNewBadge] = useState('')
  const [newPortalCustomerId, setNewPortalCustomerId] = useState('')
  // Edit selected user form
  const [editName, setEditName] = useState('')
  const [editRole, setEditRole] = useState<string>('Cashier')
  const [editActive, setEditActive] = useState(true)
  const [editPassword, setEditPassword] = useState('')
  const [editPin, setEditPin] = useState('')
  const [editClearPin, setEditClearPin] = useState(false)
  const [editBadge, setEditBadge] = useState('')
  const [editClearBadge, setEditClearBadge] = useState(false)
  const [editPortalCustomerId, setEditPortalCustomerId] = useState('')

  // --- Service tokens tab ---
  const [stores, setStores] = useState<Store[]>([])
  const [tokens, setTokens] = useState<ServiceTokenSummary[]>([])
  const [tokenLabel, setTokenLabel] = useState('')
  const [tokenStoreId, setTokenStoreId] = useState(storeId ?? '')
  const [tokenTtlDays, setTokenTtlDays] = useState('')
  const [mintedToken, setMintedToken] = useState<CreateServiceTokenResponse | null>(null)

  const populateStoreForm = useCallback((resp: StoreSettingsResponse) => {
    setStoreResp(resp)
    setStoreCode(resp.store.code)
    setStoreName(resp.store.name)
    setCountryCode(resp.store.country_code)
    setRegionCode(resp.store.region_code)
    setPostalCode(resp.store.postal_code)
    setCurrency(resp.store.currency)
    setTimezone(resp.store.timezone)
    setIsWarehouse(resp.store.is_warehouse)
  }, [])

  const populateEditUser = useCallback((u: AdminUserSummary | null) => {
    if (!u) return
    setEditName(u.display_name)
    setEditRole(u.role)
    setEditActive(u.active)
    setEditPassword('')
    setEditPin('')
    setEditClearPin(false)
    setEditBadge('')
    setEditClearBadge(false)
    setEditPortalCustomerId(u.portal_customer_id ?? '')
  }, [])

  const loadStore = useCallback(async () => {
    if (!canReadStore) return
    setLoading(true)
    setError(null)
    try {
      const resp = await getStoreSettings()
      populateStoreForm(resp)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [canReadStore, populateStoreForm])

  const loadUsers = useCallback(async () => {
    if (!isAdmin) return
    setLoading(true)
    setError(null)
    try {
      const list = await listAdminUsers()
      setUsers(list)
      setSelectedUserId((prev) => {
        const nextId = prev && list.some((u) => u.id === prev) ? prev : (list[0]?.id ?? null)
        const target = list.find((u) => u.id === nextId) ?? null
        populateEditUser(target)
        return nextId
      })
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [isAdmin, populateEditUser])

  const loadTokens = useCallback(async () => {
    if (!isAdmin) return
    setLoading(true)
    setError(null)
    try {
      const [toks, orgStores] = await Promise.all([
        listAdminServiceTokens(),
        listStores().catch(() => [] as Store[]),
      ])
      setTokens(toks)
      setStores(orgStores)
      setTokenStoreId((prev) => prev || storeId || orgStores[0]?.id || '')
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [isAdmin, storeId])

  useEffect(() => {
    if (tab === 'store') void loadStore()
    else if (tab === 'users') void loadUsers()
    else void loadTokens()
  }, [tab, loadStore, loadUsers, loadTokens])

  async function saveStore(e: React.FormEvent) {
    e.preventDefault()
    if (!isAdmin) return
    setSaving(true)
    setError(null)
    try {
      const resp = await updateStoreSettings({
        code: storeCode,
        name: storeName,
        country_code: countryCode,
        region_code: regionCode,
        postal_code: postalCode,
        currency,
        timezone,
        is_warehouse: isWarehouse,
      })
      populateStoreForm(resp)
      onStoreUpdated(resp.store)
      onToast(`Store settings saved (${resp.store.name}).`)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  async function doCreateUser(e: React.FormEvent) {
    e.preventDefault()
    setSaving(true)
    setError(null)
    try {
      const created = await createAdminUser({
        email: newEmail.trim(),
        display_name: newName.trim() || undefined,
        role: newRole,
        password: newPassword || undefined,
        pin: newPin.trim() || undefined,
        badge_token: newBadge.trim() || undefined,
        portal_customer_id:
          newRole === 'PortalContractor' ? newPortalCustomerId.trim() || null : null,
      })
      setNewEmail('')
      setNewName('')
      setNewRole('Cashier')
      setNewPassword('')
      setNewPin('')
      setNewBadge('')
      setNewPortalCustomerId('')
      setCreatingUser(false)
      onToast(`Created ${created.role} account for ${created.email}.`)
      const list = await listAdminUsers()
      setUsers(list)
      setSelectedUserId(created.id)
      populateEditUser(created)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  async function doUpdateUser(e: React.FormEvent) {
    e.preventDefault()
    if (!selectedUserId) return
    setSaving(true)
    setError(null)
    try {
      const updated = await updateAdminUser(selectedUserId, {
        display_name: editName.trim(),
        role: editRole,
        active: editActive,
        password: editPassword || undefined,
        pin: editClearPin ? undefined : editPin.trim() || undefined,
        clear_pin: editClearPin,
        badge_token: editClearBadge ? undefined : editBadge.trim() || undefined,
        clear_badge: editClearBadge,
        portal_customer_id:
          editRole === 'PortalContractor' ? editPortalCustomerId.trim() || null : null,
      })
      onToast(`Updated ${updated.email}.`)
      const list = await listAdminUsers()
      setUsers(list)
      populateEditUser(updated)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  async function doRevokeSessions(u: AdminUserSummary) {
    setSaving(true)
    setError(null)
    try {
      const res = await revokeAdminUserSessions(u.id)
      onToast(`Revoked ${res.revoked_count} active session(s) for ${u.email}.`)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  async function doCreateToken(e: React.FormEvent) {
    e.preventDefault()
    if (!tokenLabel.trim()) {
      setError('Enter a descriptive label for the service token.')
      return
    }
    const parsedTtl = tokenTtlDays.trim() ? Number(tokenTtlDays.trim()) : null
    if (parsedTtl !== null && (!Number.isInteger(parsedTtl) || parsedTtl <= 0)) {
      setError('TTL days must be a positive whole number (or leave blank for non-expiring).')
      return
    }
    setSaving(true)
    setError(null)
    try {
      const created = await createAdminServiceToken({
        store_id: tokenStoreId || undefined,
        label: tokenLabel.trim(),
        ttl_days: parsedTtl,
      })
      setMintedToken(created)
      setTokenLabel('')
      setTokenTtlDays('')
      onToast(`Issued service token "${created.label}". Copy it now — it will not be shown again.`)
      await loadTokens()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  async function doRevokeToken(tok: ServiceTokenSummary) {
    setSaving(true)
    setError(null)
    try {
      await revokeAdminServiceToken(tok.id)
      onToast(`Revoked service token "${tok.label}".`)
      await loadTokens()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  async function copyText(text: string, label: string) {
    try {
      await navigator.clipboard.writeText(text)
      onToast(`${label} copied to clipboard.`)
    } catch {
      onToast('Clipboard unavailable — select and copy the token manually.')
    }
  }

  const selectedUser = users.find((u) => u.id === selectedUserId) ?? null
  const isEditingSelf = selectedUser?.id === currentUserId

  return (
    <section className="screen" aria-label="Administration">
      <div className="screen-head">
        <h2>Administration</h2>
        <div className="screen-tabs" role="tablist">
          {(
            [
              ['store', 'Store & tax'],
              ['users', 'Staff & users'],
              ['tokens', 'Service tokens'],
            ] as [AdminTab, string][]
          ).map(([id, label]) => (
            <button
              key={id}
              type="button"
              role="tab"
              aria-selected={tab === id}
              className={tab === id ? 'active' : ''}
              onClick={() => {
                setError(null)
                setTab(id)
              }}
            >
              {label}
            </button>
          ))}
        </div>
      </div>

      {error && (
        <div className="error" role="alert">
          <span>{error}</span>
          <button type="button" className="banner-x" aria-label="Dismiss error" onClick={() => setError(null)}>
            ✕
          </button>
        </div>
      )}

      {tab === 'store' && (
        !canReadStore ? (
          <div className="screen-panel">
            <h3>Store &amp; tax settings</h3>
            <p className="screen-note">
              Signed in as <strong>{userRole}</strong>. Viewing store configuration requires a{' '}
              <strong>Manager</strong> or <strong>Admin</strong> role.
            </p>
          </div>
        ) : (
          <div className="screen-split">
            <form className="screen-panel" onSubmit={(e) => void saveStore(e)}>
              <h3>Store profile</h3>
              {!isAdmin && (
                <p className="screen-meta">
                  Read-only view (signed in as <strong>{userRole}</strong>; editing requires <strong>Admin</strong>).
                </p>
              )}
              <div className="screen-form-row">
                <label>
                  Store name
                  <input
                    className="screen-input"
                    value={storeName}
                    disabled={!isAdmin || saving}
                    onChange={(e) => setStoreName(e.target.value)}
                    required
                  />
                </label>
                <label>
                  Store code
                  <input
                    className="screen-input mono"
                    value={storeCode}
                    disabled={!isAdmin || saving}
                    onChange={(e) => setStoreCode(e.target.value)}
                    required
                  />
                </label>
              </div>
              <div className="screen-form-row">
                <label>
                  Country code
                  <select
                    className="screen-input"
                    value={countryCode}
                    disabled={!isAdmin || saving}
                    onChange={(e) => setCountryCode(e.target.value)}
                  >
                    <option value="">Not configured (0% tax)</option>
                    <option value="US">US — United States</option>
                    <option value="CA">CA — Canada</option>
                  </select>
                </label>
                <label>
                  Region / State / Province
                  <input
                    className="screen-input mono"
                    value={regionCode}
                    disabled={!isAdmin || saving}
                    placeholder="e.g. BC, ON, CA, TX, WA"
                    onChange={(e) => setRegionCode(e.target.value.toUpperCase())}
                  />
                </label>
                <label>
                  Postal / ZIP
                  <input
                    className="screen-input mono"
                    value={postalCode}
                    disabled={!isAdmin || saving}
                    placeholder="e.g. V6B 1A1 or 94107"
                    onChange={(e) => setPostalCode(e.target.value)}
                  />
                </label>
              </div>
              <div className="screen-form-row">
                <label>
                  Currency
                  <select
                    className="screen-input"
                    value={currency}
                    disabled={!isAdmin || saving}
                    onChange={(e) => setCurrency(e.target.value)}
                  >
                    <option value="USD">USD</option>
                    <option value="CAD">CAD</option>
                  </select>
                </label>
                <label>
                  Timezone
                  <input
                    className="screen-input"
                    list="df-timezones"
                    value={timezone}
                    disabled={!isAdmin || saving}
                    onChange={(e) => setTimezone(e.target.value)}
                    required
                  />
                  <datalist id="df-timezones">
                    {COMMON_TIMEZONES.map((tz) => (
                      <option key={tz} value={tz} />
                    ))}
                  </datalist>
                </label>
              </div>
              <div className="screen-form-row">
                <label style={{ flexDirection: 'row', alignItems: 'center', gap: 8 }}>
                  <input
                    type="checkbox"
                    checked={isWarehouse}
                    disabled={!isAdmin || saving}
                    onChange={(e) => setIsWarehouse(e.target.checked)}
                  />
                  <span>Acts as distribution warehouse</span>
                </label>
              </div>
              {isAdmin && (
                <div className="screen-actions">
                  <button type="submit" className="screen-btn screen-btn-primary" disabled={saving || loading}>
                    {saving ? 'Saving…' : 'Save store settings'}
                  </button>
                  <button type="button" className="screen-btn" disabled={loading} onClick={() => void loadStore()}>
                    Reload
                  </button>
                </div>
              )}
            </form>

            <div className="screen-panel">
              <h3>Active tax &amp; PaintCare / eco-fee rules</h3>
              {storeResp?.active_tax_preset ? (
                <>
                  <dl className="screen-facts">
                    <div>
                      <dt>Jurisdiction</dt>
                      <dd className="mono">
                        {storeResp.active_tax_preset.country_code}-{storeResp.active_tax_preset.region_code}
                      </dd>
                    </div>
                    <div>
                      <dt>Combined rate</dt>
                      <dd className="mono">{fmtRatePct(storeResp.active_tax_preset.total_rate)}</dd>
                    </div>
                    <div>
                      <dt>Eco-fee rules</dt>
                      <dd className="mono">{storeResp.active_eco_fee_rule_count}</dd>
                    </div>
                  </dl>
                  {storeResp.active_tax_preset.components.length > 0 ? (
                    <table className="screen-table">
                      <thead>
                        <tr>
                          <th>Component</th>
                          <th className="num">Rate</th>
                        </tr>
                      </thead>
                      <tbody>
                        {storeResp.active_tax_preset.components.map((c) => (
                          <tr key={c.kind}>
                            <td className="mono">{c.kind}</td>
                            <td className="num">{fmtRatePct(c.rate)}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  ) : (
                    <p className="screen-note">0% statewide sales tax jurisdiction.</p>
                  )}
                </>
              ) : (
                <p className="screen-empty">
                  No built-in tax preset matches{' '}
                  <span className="mono">
                    {countryCode || '—'}-{regionCode || '—'}
                  </span>
                  . Register sales calculate 0% tax until a supported jurisdiction is selected.
                </p>
              )}

              <div className="screen-sub">
                <h4>Shipped US / CA presets</h4>
                <p className="screen-note" style={{ marginBottom: 6 }}>
                  Click a preset to fill country, region, and currency (remember to click Save). Preset rates
                  are development defaults — verify against statutory schedules before live production.
                </p>
                <table className="screen-table screen-table-clickable">
                  <thead>
                    <tr>
                      <th>Jurisdiction</th>
                      <th className="num">Tax rate</th>
                      <th className="num">Eco rules</th>
                    </tr>
                  </thead>
                  <tbody>
                    {(storeResp?.available_presets ?? []).map((p) => {
                      const active =
                        p.country_code === countryCode && p.region_code === regionCode
                      return (
                        <tr
                          key={`${p.country_code}-${p.region_code}`}
                          className={active ? 'selected' : ''}
                          onClick={() => {
                            if (!isAdmin) return
                            setCountryCode(p.country_code)
                            setRegionCode(p.region_code)
                            setCurrency(p.country_code === 'CA' ? 'CAD' : 'USD')
                          }}
                        >
                          <td className="mono">
                            {p.country_code}-{p.region_code}
                          </td>
                          <td className="num">{fmtRatePct(p.total_rate)}</td>
                          <td className="num">{p.eco_fee_rule_count}</td>
                        </tr>
                      )
                    })}
                  </tbody>
                </table>
              </div>
            </div>
          </div>
        )
      )}

      {tab === 'users' && (
        !isAdmin ? (
          <div className="screen-panel">
            <h3>Staff &amp; users</h3>
            <p className="screen-note">
              Signed in as <strong>{userRole}</strong>. Managing staff accounts, PINs, and sessions requires
              the <strong>Admin</strong> role.
            </p>
          </div>
        ) : (
          <div className="screen-split">
            <div className="screen-panel">
              <div className="screen-group-head">
                <span>Org accounts ({users.length})</span>
                <span />
                <button
                  type="button"
                  className="screen-btn screen-btn-primary"
                  onClick={() => {
                    setCreatingUser((v) => !v)
                    setError(null)
                  }}
                >
                  {creatingUser ? 'Cancel new user' : 'New user'}
                </button>
              </div>
              {users.length === 0 ? (
                <p className="screen-empty">No users loaded.</p>
              ) : (
                <table className="screen-table screen-table-clickable">
                  <thead>
                    <tr>
                      <th>Name</th>
                      <th>Role</th>
                      <th>Auth</th>
                      <th>Status</th>
                    </tr>
                  </thead>
                  <tbody>
                    {users.map((u) => (
                      <tr
                        key={u.id}
                        className={!creatingUser && u.id === selectedUserId ? 'selected' : ''}
                        onClick={() => {
                          setCreatingUser(false)
                          setSelectedUserId(u.id)
                          populateEditUser(u)
                          setError(null)
                        }}
                      >
                        <td>
                          <div>{u.display_name}</div>
                          <div className="screen-note-inline mono">{u.email}</div>
                        </td>
                        <td className="mono">{u.role}</td>
                        <td className="mono">
                          {[
                            u.has_password ? 'PW' : null,
                            u.has_pin ? 'PIN' : null,
                            u.has_badge ? 'BDG' : null,
                          ]
                            .filter(Boolean)
                            .join(' · ') || '—'}
                        </td>
                        <td>{u.active ? 'Active' : 'Disabled'}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>

            {creatingUser ? (
              <form className="screen-panel" onSubmit={(e) => void doCreateUser(e)}>
                <h3>Create staff / portal account</h3>
                <div className="screen-form-row">
                  <label>
                    Email
                    <input
                      type="email"
                      className="screen-input"
                      value={newEmail}
                      disabled={saving}
                      onChange={(e) => setNewEmail(e.target.value)}
                      required
                    />
                  </label>
                  <label>
                    Display name
                    <input
                      className="screen-input"
                      value={newName}
                      disabled={saving}
                      placeholder="Defaults to email prefix"
                      onChange={(e) => setNewName(e.target.value)}
                    />
                  </label>
                </div>
                <div className="screen-form-row">
                  <label>
                    Role
                    <select
                      className="screen-input"
                      value={newRole}
                      disabled={saving}
                      onChange={(e) => setNewRole(e.target.value)}
                    >
                      {ROLES.map((r) => (
                        <option key={r} value={r}>
                          {r}
                        </option>
                      ))}
                    </select>
                  </label>
                  {newRole === 'PortalContractor' && (
                    <label>
                      Linked customer UUID
                      <input
                        className="screen-input mono"
                        value={newPortalCustomerId}
                        disabled={saving}
                        placeholder="Customer UUID"
                        onChange={(e) => setNewPortalCustomerId(e.target.value)}
                        required
                      />
                    </label>
                  )}
                </div>
                <div className="screen-form-row">
                  <label>
                    Password (min 8 chars)
                    <input
                      type="password"
                      className="screen-input"
                      value={newPassword}
                      disabled={saving}
                      autoComplete="new-password"
                      onChange={(e) => setNewPassword(e.target.value)}
                    />
                  </label>
                  <label>
                    Counter PIN (4–12 digits, optional)
                    <input
                      type="password"
                      inputMode="numeric"
                      className="screen-input mono"
                      value={newPin}
                      disabled={saving}
                      autoComplete="off"
                      onChange={(e) => setNewPin(e.target.value)}
                    />
                  </label>
                  <label>
                    Badge token (optional)
                    <input
                      type="password"
                      className="screen-input mono"
                      value={newBadge}
                      disabled={saving}
                      autoComplete="off"
                      onChange={(e) => setNewBadge(e.target.value)}
                    />
                  </label>
                </div>
                <div className="screen-actions">
                  <button type="submit" className="screen-btn screen-btn-primary" disabled={saving}>
                    {saving ? 'Creating…' : 'Create account'}
                  </button>
                  <button
                    type="button"
                    className="screen-btn"
                    disabled={saving}
                    onClick={() => setCreatingUser(false)}
                  >
                    Cancel
                  </button>
                </div>
              </form>
            ) : (
              <form className="screen-panel" onSubmit={(e) => void doUpdateUser(e)}>
                <h3>Edit account</h3>
                {!selectedUser ? (
                  <p className="screen-empty">Select a user from the table to edit credentials or role.</p>
                ) : (
                  <>
                    <p className="screen-meta mono">
                      {selectedUser.email}
                      {isEditingSelf ? ' (signed in as you)' : ''}
                    </p>
                    <div className="screen-form-row">
                      <label>
                        Display name
                        <input
                          className="screen-input"
                          value={editName}
                          disabled={saving}
                          onChange={(e) => setEditName(e.target.value)}
                          required
                        />
                      </label>
                      <label>
                        Role
                        <select
                          className="screen-input"
                          value={editRole}
                          disabled={saving || isEditingSelf}
                          onChange={(e) => setEditRole(e.target.value)}
                        >
                          {ROLES.map((r) => (
                            <option key={r} value={r}>
                              {r}
                            </option>
                          ))}
                        </select>
                      </label>
                    </div>
                    {editRole === 'PortalContractor' && (
                      <div className="screen-form-row">
                        <label>
                          Linked customer UUID
                          <input
                            className="screen-input mono"
                            value={editPortalCustomerId}
                            disabled={saving}
                            onChange={(e) => setEditPortalCustomerId(e.target.value)}
                            required
                          />
                        </label>
                      </div>
                    )}
                    <div className="screen-form-row">
                      <label style={{ flexDirection: 'row', alignItems: 'center', gap: 8 }}>
                        <input
                          type="checkbox"
                          checked={editActive}
                          disabled={saving || isEditingSelf}
                          onChange={(e) => setEditActive(e.target.checked)}
                        />
                        <span>Account active</span>
                      </label>
                    </div>
                    {isEditingSelf && (
                      <p className="screen-note" style={{ marginBottom: 8 }}>
                        You cannot deactivate or demote your own active Admin session.
                      </p>
                    )}

                    <div className="screen-sub">
                      <h4>Rotate credentials (leave blank to keep existing)</h4>
                      <div className="screen-form-row">
                        <label>
                          New password (min 8 chars)
                          <input
                            type="password"
                            className="screen-input"
                            value={editPassword}
                            disabled={saving}
                            autoComplete="new-password"
                            placeholder={selectedUser.has_password ? 'Password set' : 'Not set'}
                            onChange={(e) => setEditPassword(e.target.value)}
                          />
                        </label>
                        <label>
                          New counter PIN
                          <input
                            type="password"
                            inputMode="numeric"
                            className="screen-input mono"
                            value={editPin}
                            disabled={saving || editClearPin}
                            autoComplete="off"
                            placeholder={selectedUser.has_pin ? 'PIN set' : 'Not set'}
                            onChange={(e) => setEditPin(e.target.value)}
                          />
                        </label>
                        <label>
                          New badge token
                          <input
                            type="password"
                            className="screen-input mono"
                            value={editBadge}
                            disabled={saving || editClearBadge}
                            autoComplete="off"
                            placeholder={selectedUser.has_badge ? 'Badge set' : 'Not set'}
                            onChange={(e) => setEditBadge(e.target.value)}
                          />
                        </label>
                      </div>
                      <div className="screen-form-row">
                        {selectedUser.has_pin && (
                          <label style={{ flexDirection: 'row', alignItems: 'center', gap: 8 }}>
                            <input
                              type="checkbox"
                              checked={editClearPin}
                              disabled={saving}
                              onChange={(e) => setEditClearPin(e.target.checked)}
                            />
                            <span>Clear counter PIN</span>
                          </label>
                        )}
                        {selectedUser.has_badge && (
                          <label style={{ flexDirection: 'row', alignItems: 'center', gap: 8 }}>
                            <input
                              type="checkbox"
                              checked={editClearBadge}
                              disabled={saving}
                              onChange={(e) => setEditClearBadge(e.target.checked)}
                            />
                            <span>Clear badge token</span>
                          </label>
                        )}
                      </div>
                    </div>

                    <div className="screen-actions">
                      <button type="submit" className="screen-btn screen-btn-primary" disabled={saving}>
                        {saving ? 'Saving…' : 'Save changes'}
                      </button>
                      <button
                        type="button"
                        className="screen-btn"
                        disabled={saving}
                        onClick={() => void doRevokeSessions(selectedUser)}
                      >
                        Revoke all sessions
                      </button>
                    </div>
                  </>
                )}
              </form>
            )}
          </div>
        )
      )}

      {tab === 'tokens' && (
        !isAdmin ? (
          <div className="screen-panel">
            <h3>Store service tokens</h3>
            <p className="screen-note">
              Signed in as <strong>{userRole}</strong>. Issuing or revoking <span className="mono">dfs_</span>{' '}
              service tokens requires the <strong>Admin</strong> role.
            </p>
          </div>
        ) : (
          <>
            {mintedToken && (
              <div className="notice" role="status" style={{ flexDirection: 'column', alignItems: 'stretch', gap: 6 }}>
                <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', gap: 8 }}>
                  <strong>
                    New service token issued for “{mintedToken.label}” — copy now (shown once only):
                  </strong>
                  <button
                    type="button"
                    className="banner-x"
                    aria-label="Dismiss token reveal"
                    onClick={() => setMintedToken(null)}
                  >
                    ✕
                  </button>
                </div>
                <pre className="screen-pre" style={{ margin: 0 }}>{mintedToken.token}</pre>
                <div className="screen-actions">
                  <button
                    type="button"
                    className="screen-btn screen-btn-primary"
                    onClick={() => void copyText(mintedToken.token, 'Service token')}
                  >
                    Copy raw token
                  </button>
                  <button
                    type="button"
                    className="screen-btn"
                    onClick={() => void copyText(`DF_HQ_TOKEN=${mintedToken.token}`, 'Env export')}
                  >
                    Copy DF_HQ_TOKEN=…
                  </button>
                </div>
              </div>
            )}

            <form className="screen-panel" onSubmit={(e) => void doCreateToken(e)}>
              <h3>Issue store sync service token</h3>
              <p className="screen-note" style={{ marginBottom: 8 }}>
                Service tokens (<span className="mono">dfs_…</span>) are scoped strictly to{' '}
                <span className="mono">sync:ingest</span> for a single store and are hashed with SHA-256 at rest.
              </p>
              <div className="screen-form-row">
                <label>
                  Label
                  <input
                    className="screen-input"
                    value={tokenLabel}
                    disabled={saving}
                    placeholder="e.g. Front Counter Edge Daemon"
                    onChange={(e) => setTokenLabel(e.target.value)}
                    required
                  />
                </label>
                <label>
                  Store
                  {stores.length > 0 ? (
                    <select
                      className="screen-input"
                      value={tokenStoreId}
                      disabled={saving}
                      onChange={(e) => setTokenStoreId(e.target.value)}
                    >
                      {stores.map((s) => (
                        <option key={s.id} value={s.id}>
                          {s.code} — {s.name}
                        </option>
                      ))}
                    </select>
                  ) : (
                    <input
                      className="screen-input mono"
                      value={tokenStoreId}
                      disabled={saving}
                      placeholder="Defaults to this store"
                      onChange={(e) => setTokenStoreId(e.target.value)}
                    />
                  )}
                </label>
                <label>
                  TTL days (optional)
                  <input
                    type="number"
                    min={1}
                    className="screen-input mono"
                    value={tokenTtlDays}
                    disabled={saving}
                    placeholder="Non-expiring"
                    onChange={(e) => setTokenTtlDays(e.target.value)}
                  />
                </label>
                <button type="submit" className="screen-btn screen-btn-primary" disabled={saving}>
                  {saving ? 'Issuing…' : 'Issue token'}
                </button>
              </div>
            </form>

            <div className="screen-panel">
              <div className="screen-group-head">
                <span>Service tokens ({tokens.length})</span>
                <span />
                <button type="button" className="screen-btn" disabled={loading} onClick={() => void loadTokens()}>
                  Refresh
                </button>
              </div>
              {tokens.length === 0 ? (
                <p className="screen-empty">No store service tokens issued yet.</p>
              ) : (
                <table className="screen-table">
                  <thead>
                    <tr>
                      <th>Label</th>
                      <th>Store</th>
                      <th>Scopes</th>
                      <th>Created</th>
                      <th>Expires</th>
                      <th>Last used</th>
                      <th>Status</th>
                      <th aria-label="Action" />
                    </tr>
                  </thead>
                  <tbody>
                    {tokens.map((tok) => {
                      const storeLabel =
                        stores.find((s) => s.id === tok.store_id)?.code ?? tok.store_id.slice(0, 8)
                      const isRevoked = !!tok.revoked_at
                      return (
                        <tr key={tok.id}>
                          <td>{tok.label}</td>
                          <td className="mono" title={tok.store_id}>{storeLabel}</td>
                          <td className="mono">{tok.scopes.join(', ')}</td>
                          <td>{fmtTime(tok.created_at)}</td>
                          <td>{tok.expires_at ? fmtTime(tok.expires_at) : 'Never'}</td>
                          <td>{fmtTime(tok.last_used_at)}</td>
                          <td>{isRevoked ? 'Revoked' : 'Active'}</td>
                          <td>
                            {!isRevoked && (
                              <button
                                type="button"
                                className="screen-btn"
                                disabled={saving}
                                onClick={() => void doRevokeToken(tok)}
                              >
                                Revoke
                              </button>
                            )}
                          </td>
                        </tr>
                      )
                    })}
                  </tbody>
                </table>
              )}
            </div>
          </>
        )
      )}
    </section>
  )
}
