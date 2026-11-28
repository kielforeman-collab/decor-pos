// Bulk Matrix Pricing Rules editor (absorbed from web-backoffice).
// Enforces the server-side min-margin floor guardrail on batch save.

import { useCallback, useEffect, useState } from 'react'
import { bulkUpdatePricingRules, listPricingRules } from '../backofficeApi'
import type { PricingRuleRow } from '../backofficeTypes'

interface RuleDraft {
  id: string
  calc_method: string
  calc_value: string
  min_margin_floor: string
}

export default function PricingRulesPanel({
  onToast,
}: {
  onToast: (msg: string) => void
}) {
  const [rules, setRules] = useState<PricingRuleRow[]>([])
  const [drafts, setDrafts] = useState<Record<string, RuleDraft>>({})
  const [loading, setLoading] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const list = await listPricingRules()
      setRules(list)
      const next: Record<string, RuleDraft> = {}
      for (const r of list) {
        next[r.id] = {
          id: r.id,
          calc_method: r.calc_method,
          calc_value: r.calc_value,
          min_margin_floor: r.min_margin_floor ?? '',
        }
      }
      setDrafts(next)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void load()
  }, [load])

  async function doSave() {
    setSaving(true)
    setError(null)
    try {
      const payload = rules.map((r) => {
        const d = drafts[r.id]
        return {
          id: r.id,
          calc_method: d?.calc_method ?? r.calc_method,
          calc_value: d?.calc_value ?? r.calc_value,
          min_margin_floor: d && d.min_margin_floor.trim() !== '' ? d.min_margin_floor.trim() : null,
        }
      })
      const saved = await bulkUpdatePricingRules(payload)
      setRules(saved)
      onToast(`Saved ${saved.length} pricing rule(s).`)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="screen-panel">
      <div className="screen-head" style={{ marginBottom: 8 }}>
        <h3>Matrix pricing rules ({rules.length})</h3>
        <div className="screen-actions">
          <button type="button" className="screen-btn" disabled={loading} onClick={() => void load()}>
            {loading ? 'Loading…' : 'Refresh'}
          </button>
          <button
            type="button"
            className="screen-btn screen-btn-primary"
            disabled={saving || rules.length === 0}
            onClick={() => void doSave()}
          >
            {saving ? 'Saving…' : 'Save all rules'}
          </button>
        </div>
      </div>

      <p className="screen-note" style={{ marginBottom: 8 }}>
        Any rule whose computed price falls below its min-margin floor rejects the entire batch so nothing changes partially.
      </p>

      {error && (
        <div className="error" role="alert">
          <span>{error}</span>
          <button type="button" className="banner-x" aria-label="Dismiss error" onClick={() => setError(null)}>✕</button>
        </div>
      )}

      {rules.length === 0 ? (
        <p className="screen-empty">No matrix pricing rules configured yet.</p>
      ) : (
        <table className="screen-table">
          <thead>
            <tr>
              <th>Priority</th>
              <th>Rule ID</th>
              <th className="num">Min qty</th>
              <th>Method</th>
              <th className="num">Value</th>
              <th className="num">Min margin %</th>
            </tr>
          </thead>
          <tbody>
            {rules.map((r) => {
              const d = drafts[r.id]
              return (
                <tr key={r.id}>
                  <td className="mono">{r.priority}</td>
                  <td className="mono">{r.id.slice(0, 8)}…</td>
                  <td className="num">{r.min_qty}</td>
                  <td>
                    <select
                      className="screen-input"
                      value={d?.calc_method ?? r.calc_method}
                      onChange={(e) =>
                        setDrafts((prev) => ({
                          ...prev,
                          [r.id]: { ...prev[r.id], calc_method: e.target.value },
                        }))
                      }
                    >
                      <option value="pct_off_list">pct_off_list</option>
                      <option value="cost_plus_pct">cost_plus_pct</option>
                      <option value="fixed_price">fixed_price</option>
                      <option value="margin_pct">margin_pct</option>
                    </select>
                  </td>
                  <td className="num">
                    <input
                      className="screen-input screen-qty"
                      inputMode="decimal"
                      value={d?.calc_value ?? r.calc_value}
                      onChange={(e) =>
                        setDrafts((prev) => ({
                          ...prev,
                          [r.id]: { ...prev[r.id], calc_value: e.target.value },
                        }))
                      }
                    />
                  </td>
                  <td className="num">
                    <input
                      className="screen-input screen-qty"
                      inputMode="decimal"
                      placeholder="none"
                      value={d?.min_margin_floor ?? ''}
                      onChange={(e) =>
                        setDrafts((prev) => ({
                          ...prev,
                          [r.id]: { ...prev[r.id], min_margin_floor: e.target.value },
                        }))
                      }
                    />
                  </td>
                </tr>
              )
            })}
          </tbody>
        </table>
      )}
    </div>
  )
}
