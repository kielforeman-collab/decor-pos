// CRM — contractor activity log, due/overdue reminder tickler,
// appointments, saved views (activity & customer dialects), and
// lapsed-contractor report.

import { useCallback, useEffect, useState } from 'react'
import {
  cancelAppointment,
  completeActivity,
  createActivity,
  createAppointment,
  createSavedView,
  deleteActivity,
  deleteAppointment,
  deleteSavedView,
  getLapsedReport,
  listActivities,
  listAppointments,
  listDueReminders,
  listOverdueReminders,
  listSavedViews,
  runSavedView,
  runSavedViewCustomers,
  snoozeActivity,
} from '../backofficeApi'
import type {
  Appointment,
  CrmActivity,
  CrmSavedView,
  LapsedReport,
  SavedViewCustomer,
} from '../backofficeTypes'
import { fmtMoney } from '../types'

type CrmTab = 'activities' | 'appointments' | 'views' | 'lapsed'

function today(): string {
  return new Date().toISOString().slice(0, 10)
}

export default function CrmScreen({
  storeId,
  onToast,
}: {
  storeId: string | null
  onToast: (msg: string) => void
}) {
  const [tab, setTab] = useState<CrmTab>('activities')
  const [loading, setLoading] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Activities + tickler
  const [activities, setActivities] = useState<CrmActivity[]>([])
  const [due, setDue] = useState<CrmActivity[]>([])
  const [overdue, setOverdue] = useState<CrmActivity[]>([])
  const [actType, setActType] = useState('call')
  const [actSubject, setActSubject] = useState('')
  const [actCustomerId, setActCustomerId] = useState('')
  const [actDueAt, setActDueAt] = useState('')
  const [actNotes, setActNotes] = useState('')

  // Appointments
  const [apptDate, setApptDate] = useState(today())
  const [appointments, setAppointments] = useState<Appointment[]>([])
  const [apptTitle, setApptTitle] = useState('')
  const [apptStarts, setApptStarts] = useState(`${today()}T10:00:00Z`)
  const [apptEnds, setApptEnds] = useState(`${today()}T11:00:00Z`)
  const [apptCustomerId, setApptCustomerId] = useState('')
  const [apptStaffId, setApptStaffId] = useState('')

  // Saved views
  const [views, setViews] = useState<CrmSavedView[]>([])
  const [viewName, setViewName] = useState('')
  const [viewFilterText, setViewFilterText] = useState('{"completed": false}')
  const [viewActivities, setViewActivities] = useState<CrmActivity[] | null>(null)
  const [viewCustomers, setViewCustomers] = useState<SavedViewCustomer[] | null>(null)

  // Lapsed report
  const [inactiveDays, setInactiveDays] = useState('60')
  const [includeNever, setIncludeNever] = useState(true)
  const [lapsed, setLapsed] = useState<LapsedReport | null>(null)

  const loadActivities = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const [a, d, o] = await Promise.all([
        listActivities(),
        listDueReminders(),
        listOverdueReminders(),
      ])
      setActivities(a)
      setDue(d)
      setOverdue(o)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [])

  const loadAppointments = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      setAppointments(await listAppointments({ date: apptDate || undefined }))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [apptDate])

  const loadViews = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      setViews(await listSavedViews())
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [])

  const loadLapsed = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      setLapsed(
        await getLapsedReport({
          store_id: storeId ?? undefined,
          inactive_days: Math.max(1, Number(inactiveDays) || 60),
          include_never: includeNever,
        }),
      )
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [storeId, inactiveDays, includeNever])

  useEffect(() => {
    if (tab === 'activities') void loadActivities()
    else if (tab === 'appointments') void loadAppointments()
    else if (tab === 'views') void loadViews()
    else void loadLapsed()
  }, [tab, loadActivities, loadAppointments, loadViews, loadLapsed])

  async function doCreateActivity() {
    if (!actSubject.trim()) {
      setError('Activity subject is required.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      await createActivity({
        activity_type: actType,
        subject: actSubject.trim(),
        customer_id: actCustomerId.trim() || null,
        due_at: actDueAt.trim() || null,
        notes: actNotes.trim() || null,
      })
      setActSubject('')
      setActNotes('')
      onToast('CRM activity logged.')
      await loadActivities()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  async function doSnoozeOneDay(id: string) {
    setBusy(true)
    setError(null)
    try {
      const tomorrow = new Date(Date.now() + 86_400_000).toISOString()
      await snoozeActivity(id, tomorrow)
      onToast('Reminder snoozed 24h.')
      await loadActivities()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  async function doCreateAppt() {
    if (!apptTitle.trim()) {
      setError('Appointment title is required.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      await createAppointment({
        title: apptTitle.trim(),
        starts_at: apptStarts.trim(),
        ends_at: apptEnds.trim(),
        customer_id: apptCustomerId.trim() || null,
        staff_id: apptStaffId.trim() || null,
      })
      setApptTitle('')
      onToast('Appointment scheduled.')
      await loadAppointments()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  async function doCreateView() {
    if (!viewName.trim()) {
      setError('Saved view name is required.')
      return
    }
    let parsed: unknown
    try {
      parsed = JSON.parse(viewFilterText)
    } catch {
      setError('Filter JSON is not valid JSON.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      await createSavedView({ name: viewName.trim(), filter_json: parsed })
      setViewName('')
      onToast('Saved view created.')
      await loadViews()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <section className="screen" aria-label="CRM">
      <div className="screen-head">
        <h2>CRM</h2>
        <div className="screen-tabs" role="tablist">
          {(
            [
              ['activities', `Activities & tickler (${overdue.length} overdue)`],
              ['appointments', 'Appointments'],
              ['views', 'Saved views'],
              ['lapsed', 'Lapsed contractors'],
            ] as [CrmTab, string][]
          ).map(([id, label]) => (
            <button
              key={id}
              type="button"
              role="tab"
              aria-selected={tab === id}
              className={tab === id ? 'active' : ''}
              onClick={() => setTab(id)}
            >
              {label}
            </button>
          ))}
        </div>
      </div>

      {error && (
        <div className="error" role="alert">
          <span>{error}</span>
          <button type="button" className="banner-x" aria-label="Dismiss error" onClick={() => setError(null)}>✕</button>
        </div>
      )}

      {tab === 'activities' && (
        <>
          <div className="screen-panel">
            <h3>Log activity / reminder</h3>
            <div className="screen-form-row">
              <label>
                Type
                <select className="screen-input" value={actType} onChange={(e) => setActType(e.target.value)}>
                  <option value="call">Call</option>
                  <option value="visit">Site visit</option>
                  <option value="email">Email</option>
                  <option value="quote_followup">Quote follow-up</option>
                  <option value="note">Note</option>
                </select>
              </label>
              <label>
                Subject *
                <input className="screen-input" value={actSubject} onChange={(e) => setActSubject(e.target.value)} placeholder="Follow up on exterior quote" />
              </label>
              <label>
                Customer UUID (optional)
                <input className="screen-input mono" value={actCustomerId} onChange={(e) => setActCustomerId(e.target.value)} placeholder="UUID" />
              </label>
              <label>
                Due (RFC 3339, optional)
                <input className="screen-input mono" value={actDueAt} onChange={(e) => setActDueAt(e.target.value)} placeholder="2026-12-01T17:00:00Z" />
              </label>
              <label>
                Notes
                <input className="screen-input" value={actNotes} onChange={(e) => setActNotes(e.target.value)} />
              </label>
              <button type="button" className="screen-btn screen-btn-primary" disabled={busy} onClick={() => void doCreateActivity()}>
                Log activity
              </button>
            </div>
          </div>

          <div className="screen-split">
            <div className="screen-panel">
              <h3>Due / overdue tickler ({due.length} due · {overdue.length} overdue)</h3>
              {due.length === 0 ? (
                <p className="screen-empty">No reminders due right now.</p>
              ) : (
                <table className="screen-table">
                  <thead>
                    <tr>
                      <th>Subject</th>
                      <th>Due</th>
                      <th>Actions</th>
                    </tr>
                  </thead>
                  <tbody>
                    {due.map((r) => (
                      <tr key={r.id}>
                        <td>{r.subject}</td>
                        <td className="mono">{(r.snoozed_until ?? r.due_at ?? '—').slice(0, 16).replace('T', ' ')}</td>
                        <td className="screen-actions">
                          <button
                            type="button"
                            className="screen-btn"
                            disabled={busy}
                            onClick={() => void completeActivity(r.id).then(loadActivities).catch((e) => setError(String(e)))}
                          >
                            Done
                          </button>
                          <button type="button" className="screen-btn" disabled={busy} onClick={() => void doSnoozeOneDay(r.id)}>
                            +24h
                          </button>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>

            <div className="screen-panel">
              <h3>All activities ({activities.length})</h3>
              {activities.length === 0 ? (
                <p className="screen-empty">No CRM activities logged yet.</p>
              ) : (
                <table className="screen-table">
                  <thead>
                    <tr>
                      <th>Type</th>
                      <th>Subject</th>
                      <th>Status</th>
                      <th />
                    </tr>
                  </thead>
                  <tbody>
                    {activities.map((a) => (
                      <tr key={a.id}>
                        <td>{a.activity_type}</td>
                        <td>{a.subject}</td>
                        <td>{a.completed_at ? 'Completed' : 'Open'}</td>
                        <td>
                          <button
                            type="button"
                            className="screen-btn screen-btn-ghost"
                            disabled={busy}
                            onClick={() => void deleteActivity(a.id).then(loadActivities).catch((e) => setError(String(e)))}
                          >
                            Delete
                          </button>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>
          </div>
        </>
      )}

      {tab === 'appointments' && (
        <div className="screen-panel">
          <h3>Appointments</h3>
          <div className="screen-form-row">
            <label>
              Day filter
              <input type="date" className="screen-input" value={apptDate} onChange={(e) => setApptDate(e.target.value)} />
            </label>
            <button type="button" className="screen-btn" disabled={loading} onClick={() => void loadAppointments()}>
              Refresh
            </button>
          </div>
          <div className="screen-form-row">
            <label>
              Title *
              <input className="screen-input" value={apptTitle} onChange={(e) => setApptTitle(e.target.value)} placeholder="Colour consultation" />
            </label>
            <label>
              Starts (RFC 3339) *
              <input className="screen-input mono" value={apptStarts} onChange={(e) => setApptStarts(e.target.value)} />
            </label>
            <label>
              Ends (RFC 3339) *
              <input className="screen-input mono" value={apptEnds} onChange={(e) => setApptEnds(e.target.value)} />
            </label>
            <label>
              Customer UUID
              <input className="screen-input mono" value={apptCustomerId} onChange={(e) => setApptCustomerId(e.target.value)} placeholder="optional" />
            </label>
            <label>
              Staff UUID
              <input className="screen-input mono" value={apptStaffId} onChange={(e) => setApptStaffId(e.target.value)} placeholder="optional" />
            </label>
            <button type="button" className="screen-btn screen-btn-primary" disabled={busy} onClick={() => void doCreateAppt()}>
              Schedule
            </button>
          </div>

          {appointments.length === 0 ? (
            <p className="screen-empty">No appointments for this day.</p>
          ) : (
            <table className="screen-table">
              <thead>
                <tr>
                  <th>Title</th>
                  <th>Window</th>
                  <th>Status</th>
                  <th>Actions</th>
                </tr>
              </thead>
              <tbody>
                {appointments.map((ap) => (
                  <tr key={ap.id}>
                    <td>{ap.title}</td>
                    <td className="mono">
                      {ap.starts_at.slice(11, 16)}–{ap.ends_at.slice(11, 16)}
                    </td>
                    <td>{ap.status}</td>
                    <td className="screen-actions">
                      {ap.status === 'scheduled' && (
                        <button
                          type="button"
                          className="screen-btn"
                          disabled={busy}
                          onClick={() => void cancelAppointment(ap.id).then(loadAppointments).catch((e) => setError(String(e)))}
                        >
                          Cancel
                        </button>
                      )}
                      <button
                        type="button"
                        className="screen-btn screen-btn-ghost"
                        disabled={busy}
                        onClick={() => void deleteAppointment(ap.id).then(loadAppointments).catch((e) => setError(String(e)))}
                      >
                        Delete
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      )}

      {tab === 'views' && (
        <div className="screen-panel">
          <h3>Saved views</h3>
          <div className="screen-form-row">
            <label>
              Name *
              <input className="screen-input" value={viewName} onChange={(e) => setViewName(e.target.value)} placeholder="Open follow-ups" />
            </label>
            <label style={{ maxWidth: 460 }}>
              Filter JSON *
              <input className="screen-input mono" value={viewFilterText} onChange={(e) => setViewFilterText(e.target.value)} />
            </label>
            <button type="button" className="screen-btn screen-btn-primary" disabled={busy} onClick={() => void doCreateView()}>
              Save view
            </button>
          </div>
          {views.length === 0 ? (
            <p className="screen-empty">No saved views yet.</p>
          ) : (
            <table className="screen-table">
              <thead>
                <tr>
                  <th>Name</th>
                  <th>Filter</th>
                  <th>Run</th>
                </tr>
              </thead>
              <tbody>
                {views.map((v) => (
                  <tr key={v.id}>
                    <td>{v.name}</td>
                    <td className="mono">{JSON.stringify(v.filter_json)}</td>
                    <td className="screen-actions">
                      <button
                        type="button"
                        className="screen-btn"
                        onClick={() => {
                          setViewCustomers(null)
                          void runSavedView(v.id)
                            .then(setViewActivities)
                            .catch((e) => setError(e instanceof Error ? e.message : String(e)))
                        }}
                      >
                        Run activities
                      </button>
                      <button
                        type="button"
                        className="screen-btn"
                        onClick={() => {
                          setViewActivities(null)
                          void runSavedViewCustomers(v.id)
                            .then(setViewCustomers)
                            .catch((e) => setError(e instanceof Error ? e.message : String(e)))
                        }}
                      >
                        Run customers
                      </button>
                      <button
                        type="button"
                        className="screen-btn screen-btn-ghost"
                        onClick={() => void deleteSavedView(v.id).then(loadViews).catch((e) => setError(String(e)))}
                      >
                        Delete
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          {viewActivities && (
            <div className="screen-sub">
              <h4>Activity results ({viewActivities.length})</h4>
              <pre className="screen-pre">{JSON.stringify(viewActivities, null, 2)}</pre>
            </div>
          )}
          {viewCustomers && (
            <div className="screen-sub">
              <h4>Customer results ({viewCustomers.length})</h4>
              <table className="screen-table">
                <thead>
                  <tr>
                    <th>Account</th>
                    <th>Company</th>
                    <th>Type</th>
                    <th className="num">Balance</th>
                  </tr>
                </thead>
                <tbody>
                  {viewCustomers.map((c) => (
                    <tr key={c.customer_id}>
                      <td className="mono">{c.account_number}</td>
                      <td>{c.company_name ?? '—'}</td>
                      <td>{c.customer_type}</td>
                      <td className="num">{fmtMoney(c.balance)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>
      )}

      {tab === 'lapsed' && (
        <div className="screen-panel">
          <h3>Lapsed contractors</h3>
          <div className="screen-form-row">
            <label>
              Inactive days
              <input className="screen-input screen-qty" inputMode="numeric" value={inactiveDays} onChange={(e) => setInactiveDays(e.target.value)} />
            </label>
            <label style={{ flexDirection: 'row', alignItems: 'center', gap: 6 }}>
              <input type="checkbox" checked={includeNever} onChange={(e) => setIncludeNever(e.target.checked)} />
              Include never-purchased
            </label>
            <button type="button" className="screen-btn screen-btn-primary" disabled={loading} onClick={() => void loadLapsed()}>
              Run report
            </button>
          </div>
          {!lapsed || lapsed.customers.length === 0 ? (
            <p className="screen-empty">No lapsed contractors for this threshold.</p>
          ) : (
            <table className="screen-table">
              <thead>
                <tr>
                  <th>Account</th>
                  <th>Company</th>
                  <th>Status</th>
                  <th>Last purchase</th>
                  <th className="num">AR balance</th>
                </tr>
              </thead>
              <tbody>
                {lapsed.customers.map((c) => (
                  <tr key={c.customer_id}>
                    <td className="mono">{c.account_number}</td>
                    <td>{c.company_name ?? '—'}</td>
                    <td>{c.status.replaceAll('_', ' ')}</td>
                    <td className="mono">{c.last_purchase_at ? c.last_purchase_at.slice(0, 10) : 'Never'}</td>
                    <td className="num">{c.ar_balance}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      )}
    </section>
  )
}
