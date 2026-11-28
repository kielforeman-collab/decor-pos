// Scheduling+ — shift templates, daily staff shifts & status,
// timeclock clock-in/out, and sales-driven labor forecast & coverage.

import { useCallback, useEffect, useState } from 'react'
import {
  clockIn,
  clockOut,
  createShiftTemplate,
  createStaffShift,
  getLaborCoverage,
  getLaborForecast,
  listShiftTemplates,
  listStaffShifts,
  listTimeclock,
  setShiftStatus,
} from '../backofficeApi'
import type {
  CoverageCheck,
  ForecastReport,
  ShiftTemplate,
  StaffShift,
  TimeclockEntry,
} from '../backofficeTypes'

type SchedTab = 'shifts' | 'timeclock' | 'forecast'

const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']

function today(): string {
  return new Date().toISOString().slice(0, 10)
}

export default function SchedulingScreen({
  onToast,
}: {
  onToast: (msg: string) => void
}) {
  const [tab, setTab] = useState<SchedTab>('shifts')
  const [date, setDate] = useState(today())
  const [loading, setLoading] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const [templates, setTemplates] = useState<ShiftTemplate[]>([])
  const [shifts, setShifts] = useState<StaffShift[]>([])
  const [timeclock, setTimeclock] = useState<TimeclockEntry[]>([])
  const [forecast, setForecast] = useState<ForecastReport | null>(null)
  const [coverage, setCoverage] = useState<CoverageCheck | null>(null)

  // Template form
  const [tplName, setTplName] = useState('')
  const [tplWeekday, setTplWeekday] = useState('0')
  const [tplStarts, setTplStarts] = useState('08:00')
  const [tplEnds, setTplEnds] = useState('16:00')
  const [tplRequired, setTplRequired] = useState('2')

  // Shift / timeclock forms
  const [staffId, setStaffId] = useState('')
  const [shiftStarts, setShiftStarts] = useState(`${today()}T08:00:00Z`)
  const [shiftEnds, setShiftEnds] = useState(`${today()}T16:00:00Z`)
  const [forecastDays, setForecastDays] = useState('7')
  const [salesPerHour, setSalesPerHour] = useState('')

  const loadShifts = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const [t, s] = await Promise.all([listShiftTemplates(), listStaffShifts(date)])
      setTemplates(t)
      setShifts(s)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [date])

  const loadTimeclock = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      setTimeclock(await listTimeclock(date))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [date])

  const loadForecast = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const [f, c] = await Promise.all([
        getLaborForecast({
          from: date,
          days: Math.max(1, Number(forecastDays) || 7),
          salesPerLaborHour: salesPerHour.trim() || undefined,
        }),
        getLaborCoverage(date),
      ])
      setForecast(f)
      setCoverage(c)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [date, forecastDays, salesPerHour])

  useEffect(() => {
    if (tab === 'shifts') void loadShifts()
    else if (tab === 'timeclock') void loadTimeclock()
    else void loadForecast()
  }, [tab, loadShifts, loadTimeclock, loadForecast])

  async function doCreateTemplate() {
    if (!tplName.trim()) {
      setError('Template name is required.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      await createShiftTemplate({
        name: tplName.trim(),
        weekday: Number(tplWeekday) || 0,
        starts_at: tplStarts.trim(),
        ends_at: tplEnds.trim(),
        required_count: Math.max(1, Number(tplRequired) || 1),
      })
      setTplName('')
      onToast('Shift template saved.')
      await loadShifts()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  async function doCreateShift() {
    if (!staffId.trim()) {
      setError('Staff UUID is required.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      await createStaffShift({
        staff_id: staffId.trim(),
        shift_date: date,
        starts_at: shiftStarts.trim(),
        ends_at: shiftEnds.trim(),
      })
      onToast('Staff shift scheduled.')
      await loadShifts()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <section className="screen" aria-label="Scheduling">
      <div className="screen-head">
        <h2>Scheduling+</h2>
        <div className="screen-actions">
          <label className="screen-date">
            Date
            <input
              type="date"
              className="screen-input"
              value={date}
              onChange={(e) => {
                setDate(e.target.value)
                setShiftStarts(`${e.target.value}T08:00:00Z`)
                setShiftEnds(`${e.target.value}T16:00:00Z`)
              }}
            />
          </label>
          <div className="screen-tabs" role="tablist">
            {(
              [
                ['shifts', 'Shifts & templates'],
                ['timeclock', 'Timeclock'],
                ['forecast', 'Forecast & coverage'],
              ] as [SchedTab, string][]
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
      </div>

      {error && (
        <div className="error" role="alert">
          <span>{error}</span>
          <button type="button" className="banner-x" aria-label="Dismiss error" onClick={() => setError(null)}>✕</button>
        </div>
      )}

      {tab === 'shifts' && (
        <div className="screen-split">
          <div className="screen-panel">
            <h3>Shift templates ({templates.length})</h3>
            <div className="screen-form-row">
              <label>
                Name *
                <input className="screen-input" value={tplName} onChange={(e) => setTplName(e.target.value)} placeholder="Opening counter" />
              </label>
              <label>
                Weekday
                <select className="screen-input" value={tplWeekday} onChange={(e) => setTplWeekday(e.target.value)}>
                  {WEEKDAYS.map((w, i) => (
                    <option key={w} value={i}>{w}</option>
                  ))}
                </select>
              </label>
              <label>
                Start
                <input className="screen-input mono" value={tplStarts} onChange={(e) => setTplStarts(e.target.value)} />
              </label>
              <label>
                End
                <input className="screen-input mono" value={tplEnds} onChange={(e) => setTplEnds(e.target.value)} />
              </label>
              <label>
                Staff needed
                <input className="screen-input screen-qty" value={tplRequired} onChange={(e) => setTplRequired(e.target.value)} />
              </label>
              <button type="button" className="screen-btn screen-btn-primary" disabled={busy} onClick={() => void doCreateTemplate()}>
                Add template
              </button>
            </div>
            {templates.length === 0 ? (
              <p className="screen-empty">No shift templates defined.</p>
            ) : (
              <table className="screen-table">
                <thead>
                  <tr>
                    <th>Name</th>
                    <th>Day</th>
                    <th>Hours</th>
                    <th className="num">Req</th>
                  </tr>
                </thead>
                <tbody>
                  {templates.map((t) => (
                    <tr key={t.id}>
                      <td>{t.name}</td>
                      <td>{WEEKDAYS[t.weekday] ?? t.weekday}</td>
                      <td className="mono">{t.starts_at}–{t.ends_at}</td>
                      <td className="num">{t.required_count}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>

          <div className="screen-panel">
            <h3>Shifts on {date} ({shifts.length})</h3>
            <div className="screen-form-row">
              <label>
                Staff UUID *
                <input className="screen-input mono" value={staffId} onChange={(e) => setStaffId(e.target.value)} placeholder="Staff UUID" />
              </label>
              <label>
                Starts (RFC 3339)
                <input className="screen-input mono" value={shiftStarts} onChange={(e) => setShiftStarts(e.target.value)} />
              </label>
              <label>
                Ends (RFC 3339)
                <input className="screen-input mono" value={shiftEnds} onChange={(e) => setShiftEnds(e.target.value)} />
              </label>
              <button type="button" className="screen-btn screen-btn-primary" disabled={busy} onClick={() => void doCreateShift()}>
                Schedule shift
              </button>
            </div>
            {shifts.length === 0 ? (
              <p className="screen-empty">No shifts scheduled for {date}.</p>
            ) : (
              <table className="screen-table">
                <thead>
                  <tr>
                    <th>Staff</th>
                    <th>Window</th>
                    <th>Status</th>
                    <th>Actions</th>
                  </tr>
                </thead>
                <tbody>
                  {shifts.map((s) => (
                    <tr key={s.id}>
                      <td className="mono">{s.staff_id.slice(0, 8)}…</td>
                      <td className="mono">{s.starts_at.slice(11, 16)}–{s.ends_at.slice(11, 16)}</td>
                      <td>{s.status}</td>
                      <td className="screen-actions">
                        {s.status !== 'completed' && (
                          <button
                            type="button"
                            className="screen-btn"
                            disabled={busy}
                            onClick={() => void setShiftStatus(s.id, 'completed').then(loadShifts).catch((e) => setError(String(e)))}
                          >
                            Complete
                          </button>
                        )}
                        {s.status !== 'cancelled' && (
                          <button
                            type="button"
                            className="screen-btn screen-btn-ghost"
                            disabled={busy}
                            onClick={() => void setShiftStatus(s.id, 'cancelled').then(loadShifts).catch((e) => setError(String(e)))}
                          >
                            Cancel
                          </button>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
        </div>
      )}

      {tab === 'timeclock' && (
        <div className="screen-panel">
          <h3>Timeclock ({date})</h3>
          <div className="screen-form-row">
            <label>
              Staff UUID *
              <input className="screen-input mono" value={staffId} onChange={(e) => setStaffId(e.target.value)} placeholder="Staff UUID" />
            </label>
            <button
              type="button"
              className="screen-btn screen-btn-primary"
              disabled={busy || !staffId.trim()}
              onClick={() =>
                void clockIn(staffId.trim())
                  .then(() => {
                    onToast('Clocked in.')
                    return loadTimeclock()
                  })
                  .catch((e) => setError(e instanceof Error ? e.message : String(e)))
              }
            >
              Clock in
            </button>
            <button
              type="button"
              className="screen-btn"
              disabled={busy || !staffId.trim()}
              onClick={() =>
                void clockOut(staffId.trim())
                  .then(() => {
                    onToast('Clocked out.')
                    return loadTimeclock()
                  })
                  .catch((e) => setError(e instanceof Error ? e.message : String(e)))
              }
            >
              Clock out
            </button>
          </div>
          {timeclock.length === 0 ? (
            <p className="screen-empty">No timeclock entries on {date}.</p>
          ) : (
            <table className="screen-table">
              <thead>
                <tr>
                  <th>Staff</th>
                  <th>Clock in</th>
                  <th>Clock out</th>
                  <th>Source</th>
                </tr>
              </thead>
              <tbody>
                {timeclock.map((e) => (
                  <tr key={e.id}>
                    <td className="mono">{e.staff_id.slice(0, 8)}…</td>
                    <td className="mono">{e.clock_in.slice(0, 19).replace('T', ' ')}</td>
                    <td className="mono">{e.clock_out ? e.clock_out.slice(0, 19).replace('T', ' ') : 'Open'}</td>
                    <td>{e.source}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      )}

      {tab === 'forecast' && (
        <div className="screen-panel">
          <h3>Labor forecast &amp; coverage</h3>
          <div className="screen-form-row">
            <label>
              Days
              <input className="screen-input screen-qty" value={forecastDays} onChange={(e) => setForecastDays(e.target.value)} />
            </label>
            <label>
              Sales / labor hr override
              <input className="screen-input screen-qty" value={salesPerHour} onChange={(e) => setSalesPerHour(e.target.value)} placeholder="auto" />
            </label>
            <button type="button" className="screen-btn screen-btn-primary" disabled={loading} onClick={() => void loadForecast()}>
              {loading ? 'Loading…' : 'Refresh'}
            </button>
          </div>
          {coverage && (
            <dl className="screen-facts">
              <div><dt>Coverage ({coverage.date})</dt><dd>{coverage.status}</dd></div>
              <div><dt>Scheduled hrs</dt><dd className="mono">{coverage.scheduled_hours}</dd></div>
              <div><dt>Forecast hrs</dt><dd className="mono">{coverage.forecast_hours ?? '—'}</dd></div>
              <div><dt>Delta</dt><dd className="mono">{coverage.delta_hours ?? '—'}</dd></div>
            </dl>
          )}
          {forecast && (
            <>
              <p className="screen-meta">
                History: {forecast.history_txns} txns over {forecast.history_days} days · sales/labor-hr {forecast.sales_per_labor_hour}
              </p>
              <table className="screen-table">
                <thead>
                  <tr>
                    <th>Date</th>
                    <th>Day</th>
                    <th className="num">Forecast txns</th>
                    <th className="num">Forecast sales</th>
                    <th className="num">Required hrs</th>
                  </tr>
                </thead>
                <tbody>
                  {forecast.days.map((d) => (
                    <tr key={d.date}>
                      <td className="mono">{d.date}</td>
                      <td>{WEEKDAYS[d.weekday] ?? d.weekday}</td>
                      <td className="num">{d.forecast_txns}</td>
                      <td className="num">{d.forecast_sales}</td>
                      <td className="num">{d.required_hours}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </>
          )}
        </div>
      )}
    </section>
  )
}
