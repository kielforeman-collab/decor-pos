// Sync — edge outbox status, on-demand HQ sync trigger, and F8 dead-letter
// triage (inspect quarantined entries, requeue for retry, or dismiss with an
// operator reason). Uses the existing pos-desktop CSS token system.

import { useCallback, useEffect, useState } from 'react'
import { syncRun, syncState } from '../api'
import {
  dismissSyncDeadLetter,
  listSyncDeadLetters,
  requeueSyncDeadLetter,
} from '../backofficeApi'
import type { DeadLetterEntry } from '../backofficeTypes'
import type { ApplyReport, SyncState } from '../types'

function fmtTime(iso: string | null): string {
  if (!iso) return '—'
  const d = new Date(iso)
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString()
}

export default function SyncScreen({
  userRole,
  onSyncUpdated,
  onToast,
}: {
  userRole: string
  onSyncUpdated: (state: SyncState) => void
  onToast: (msg: string) => void
}) {
  const canTriage = userRole === 'Admin' || userRole === 'Manager'

  const [state, setState] = useState<SyncState | null>(null)
  const [deadLetters, setDeadLetters] = useState<DeadLetterEntry[]>([])
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [lastReport, setLastReport] = useState<ApplyReport | null>(null)
  const [dismissReason, setDismissReason] = useState('')
  const [loading, setLoading] = useState(false)
  const [syncing, setSyncing] = useState(false)
  const [acting, setActing] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const s = await syncState()
      setState(s)
      onSyncUpdated(s)
      if (canTriage) {
        const dl = await listSyncDeadLetters(200)
        setDeadLetters(dl)
        setSelectedId((prev) => (prev && dl.some((e) => e.id === prev) ? prev : (dl[0]?.id ?? null)))
      } else {
        setDeadLetters([])
        setSelectedId(null)
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [canTriage, onSyncUpdated])

  useEffect(() => {
    void refresh()
  }, [refresh])

  async function doSyncNow() {
    setSyncing(true)
    setError(null)
    try {
      const report = await syncRun()
      setLastReport(report)
      const conflicts = report.conflict_ids?.length ?? 0
      onToast(
        conflicts > 0
          ? `Sync finished: ${report.applied} applied, ${report.duplicates} duplicate, ${conflicts} quarantined.`
          : `Sync finished: ${report.applied} applied, ${report.duplicates} duplicate.`,
      )
      await refresh()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setSyncing(false)
    }
  }

  async function doRequeue(id: string) {
    setActing(true)
    setError(null)
    try {
      await requeueSyncDeadLetter(id)
      onToast('Entry moved back to pending outbox (retry count reset).')
      await refresh()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setActing(false)
    }
  }

  async function doDismiss(id: string) {
    const reason = dismissReason.trim()
    if (!reason) {
      setError('Enter an operator reason before dismissing a dead-letter entry.')
      return
    }
    setActing(true)
    setError(null)
    try {
      await dismissSyncDeadLetter(id, reason)
      setDismissReason('')
      onToast('Dead-letter entry dismissed.')
      await refresh()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setActing(false)
    }
  }

  const selected = deadLetters.find((d) => d.id === selectedId) ?? null
  const dlCount = state?.dead_letter_count ?? deadLetters.length

  return (
    <section className="screen" aria-label="Sync and dead-letter queue">
      <div className="screen-head">
        <h2>Sync &amp; outbox</h2>
        <div className="screen-actions">
          <button
            type="button"
            className="screen-btn"
            disabled={loading || syncing}
            onClick={() => void refresh()}
          >
            {loading ? 'Refreshing…' : 'Refresh'}
          </button>
          <button
            type="button"
            className="screen-btn screen-btn-primary"
            disabled={syncing}
            onClick={() => void doSyncNow()}
          >
            {syncing ? 'Syncing…' : 'Sync now'}
          </button>
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

      <div className="screen-panel">
        <h3>Edge sync health</h3>
        <dl className="screen-facts">
          <div>
            <dt>Pending outbox</dt>
            <dd className="mono">{state ? state.pending_count : '—'}</dd>
          </div>
          <div>
            <dt>Dead-lettered</dt>
            <dd className="mono">{state ? dlCount : '—'}</dd>
          </div>
          <div>
            <dt>Last synced</dt>
            <dd>{state ? fmtTime(state.last_synced_at) : '—'}</dd>
          </div>
          <div>
            <dt>Last batch</dt>
            <dd className="mono" title={state?.last_batch_id ?? undefined}>
              {state?.last_batch_id ? state.last_batch_id.slice(0, 8) : '—'}
            </dd>
          </div>
          <div>
            <dt>Store ID</dt>
            <dd className="mono" title={state?.store_id}>
              {state ? state.store_id.slice(0, 8) : '—'}
            </dd>
          </div>
        </dl>
        {lastReport && (
          <p className="screen-meta">
            Last manual sync: <strong>{lastReport.applied}</strong> applied ·{' '}
            <strong>{lastReport.duplicates}</strong> duplicate ·{' '}
            <strong>{lastReport.conflict_ids?.length ?? 0}</strong> quarantined conflict(s).
          </p>
        )}
        <p className="screen-note">
          Edge outbox entries push to HQ in UUIDv7 creation order. Entries rejected as conflicts by HQ or
          exceeding the retry cap are quarantined below so healthy sales continue syncing without stalling.
        </p>
      </div>

      {!canTriage ? (
        <div className="screen-panel">
          <h3>Dead-letter queue</h3>
          <p className="screen-note">
            Signed in as <strong>{userRole}</strong>. Inspecting, requeuing, or dismissing quarantined sync
            entries requires a <strong>Manager</strong> or <strong>Admin</strong> role.
          </p>
        </div>
      ) : (
        <div className="screen-split">
          <div className="screen-panel">
            <h3>Quarantined entries ({deadLetters.length})</h3>
            {deadLetters.length === 0 ? (
              <p className="screen-empty">
                No quarantined dead-letter entries. All sync batches are clear.
              </p>
            ) : (
              <table className="screen-table screen-table-clickable">
                <thead>
                  <tr>
                    <th>Type</th>
                    <th>Created</th>
                    <th className="num">Retries</th>
                    <th>Reason</th>
                  </tr>
                </thead>
                <tbody>
                  {deadLetters.map((entry) => (
                    <tr
                      key={entry.id}
                      className={entry.id === selectedId ? 'selected' : ''}
                      onClick={() => {
                        setSelectedId(entry.id)
                        setDismissReason('')
                      }}
                    >
                      <td className="mono">{entry.aggregate_type}</td>
                      <td>{fmtTime(entry.created_at)}</td>
                      <td className="num">{entry.retry_count}</td>
                      <td>{entry.quarantine_reason ?? entry.last_error ?? 'Quarantined'}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>

          <div className="screen-panel">
            <h3>Entry detail &amp; triage</h3>
            {!selected ? (
              <p className="screen-empty">Select a quarantined entry to inspect its payload and triage it.</p>
            ) : (
              <>
                <dl className="screen-facts">
                  <div>
                    <dt>Entry ID</dt>
                    <dd className="mono" title={selected.id}>{selected.id.slice(0, 13)}…</dd>
                  </div>
                  <div>
                    <dt>Aggregate</dt>
                    <dd className="mono">{selected.aggregate_type}</dd>
                  </div>
                  <div>
                    <dt>Aggregate ID</dt>
                    <dd className="mono" title={selected.aggregate_id}>{selected.aggregate_id.slice(0, 13)}…</dd>
                  </div>
                  <div>
                    <dt>Quarantined</dt>
                    <dd>{fmtTime(selected.quarantined_at)}</dd>
                  </div>
                  <div>
                    <dt>Retries</dt>
                    <dd className="mono">{selected.retry_count}</dd>
                  </div>
                </dl>

                {selected.quarantine_reason && (
                  <p className="screen-meta">
                    <strong>Quarantine reason:</strong> {selected.quarantine_reason}
                  </p>
                )}
                {selected.last_error && (
                  <p className="screen-meta">
                    <strong>Last error:</strong> {selected.last_error}
                  </p>
                )}

                <div className="screen-actions" style={{ marginBottom: 8 }}>
                  <button
                    type="button"
                    className="screen-btn screen-btn-primary"
                    disabled={acting}
                    onClick={() => void doRequeue(selected.id)}
                  >
                    Requeue for retry
                  </button>
                </div>

                <div className="screen-sub">
                  <h4>Dismiss permanently</h4>
                  <p className="screen-note" style={{ marginBottom: 6 }}>
                    Only dismiss an entry once the discrepancy is resolved on HQ (e.g. via a reversing
                    transaction). The reason is stored on the outbox record.
                  </p>
                  <div className="screen-form-row">
                    <label>
                      Operator reason
                      <input
                        type="text"
                        className="screen-input"
                        value={dismissReason}
                        disabled={acting}
                        placeholder="e.g. Resolved via reversing TX-000142 on HQ"
                        onChange={(e) => setDismissReason(e.target.value)}
                      />
                    </label>
                    <button
                      type="button"
                      className="screen-btn"
                      disabled={acting || !dismissReason.trim()}
                      onClick={() => void doDismiss(selected.id)}
                    >
                      Dismiss entry
                    </button>
                  </div>
                </div>

                <div className="screen-sub">
                  <h4>Payload JSON</h4>
                  <pre className="screen-pre">{JSON.stringify(selected.payload, null, 2)}</pre>
                </div>
              </>
            )}
          </div>
        </div>
      )}
    </section>
  )
}
