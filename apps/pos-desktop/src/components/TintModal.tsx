import { useEffect, useMemo, useState } from 'react'
import { listTintColorants, listTintColors, listTintFormulas, saveTintFormula } from '../api'
import type { CartLine, TintColorEntry, TintColorantAmount, TintColorantEntry, TintFormulaEntry, TintInfo } from '../types'

/** The 10-colour grid — real Benjamin Moore colours, used
 * only when the colour endpoint returns nothing. No ounces are
 * attached here: ounces come from a stored formula or the clerk. */
const FALLBACK_COLORS: { code: string; name: string; hex: string; base: string }[] = [
  { code: 'HC-154', name: 'Hale Navy', hex: '#3d4a57', base: 'Base 4' },
  { code: 'HC-172', name: 'Revere Pewter', hex: '#c6bda9', base: 'Base 1' },
  { code: 'OC-17', name: 'White Dove', hex: '#edeade', base: 'Base 1' },
  { code: 'OC-117', name: 'Simply White', hex: '#f4f1e8', base: 'Base 1' },
  { code: 'OC-65', name: 'Chantilly Lace', hex: '#f5f2ec', base: 'Base 1' },
  { code: 'HC-166', name: 'Kendall Charcoal', hex: '#4a4a47', base: 'Base 4' },
  { code: 'HC-168', name: 'Chelsea Gray', hex: '#a39d93', base: 'Base 2' },
  { code: 'HC-143', name: 'Wythe Blue', hex: '#6e7f74', base: 'Base 3' },
  { code: '1495', name: 'October Mist', hex: '#c3c7b1', base: 'Base 1' },
  { code: 'HC-173', name: 'Edgecomb Gray', hex: '#c5bba7', base: 'Base 1' },
]

/** Gennex 0229 colorants, from price book 10005510 (real SKUs
 * 0229<code>-004). Fallback when the colorant endpoint is empty. */
const FALLBACK_COLORANTS: { code: string; name: string }[] = [
  { code: 'B1', name: 'Thalo Blue' },
  { code: 'G1', name: 'Thalo Green' },
  { code: 'M1', name: 'Magenta' },
  { code: 'O1', name: 'Orange' },
  { code: 'R1', name: 'Red Toner' },
  { code: 'R2', name: 'Organic Red' },
  { code: 'R3', name: 'Oxide Red' },
  { code: 'S1', name: 'Black' },
  { code: 'S2', name: 'Gray' },
  { code: 'W1', name: 'White' },
  { code: 'Y1', name: 'Organic Yellow 1' },
  { code: 'Y2', name: 'Organic Yellow 2' },
  { code: 'Y3', name: 'Oxide Yellow' },
]

interface TintModalProps {
  line: CartLine
  customerId: string | null
  onApply: (tint: TintInfo | null) => void
  onClose: () => void
  onToast: (msg: string) => void
}

export default function TintModal({ line, customerId, onApply, onClose, onToast }: TintModalProps) {
  const [query, setQuery] = useState('')
  const [colors, setColors] = useState<TintColorEntry[]>([])
  const [colorants, setColorants] = useState<TintColorantEntry[]>([])
  const [picked, setPicked] = useState<TintColorEntry | { color_code: string; color_name: string; rgb_hex: string | null; recommended_base: string } | null>(
    line.tint ? { color_code: line.tint.colorCode, color_name: line.tint.colorName, rgb_hex: line.tint.colorHex ?? null, recommended_base: line.tint.base } : null,
  )
  const [ounces, setOunces] = useState<Record<string, string>>(() => {
    const init: Record<string, string> = {}
    line.tint?.formula.forEach((f) => {
      init[f.code] = f.oz
    })
    return init
  })
  const [storedFormula, setStoredFormula] = useState<TintFormulaEntry | null>(null)
  const [loading, setLoading] = useState(true)

  useEffect(() => {
    let live = true
    void Promise.all([listTintColors('', 'Benjamin Moore'), listTintColorants()]).then(([cols, cts]) => {
      if (!live) return
      setColors(cols)
      setColorants(cts)
      setLoading(false)
    })
    return () => {
      live = false
    }
  }, [])

  // Stored formula for this customer + colour, when one exists.
  useEffect(() => {
    if (!picked || !customerId) {
      setStoredFormula(null)
      return
    }
    let live = true
    void listTintFormulas({ customer_id: customerId, query: picked.color_code }).then((fs) => {
      if (!live) return
      const match = fs.find(
        (f) => f.color_code.toLowerCase() === picked.color_code.toLowerCase(),
      )
      if (match) {
        setStoredFormula(match)
        const init: Record<string, string> = {}
        match.colorants.forEach((c) => {
          init[c.code] = c.oz
        })
        setOunces(init)
      } else {
        setStoredFormula(null)
      }
    })
    return () => {
      live = false
    }
  }, [picked, customerId])

  const shownColors = useMemo(() => {
    const q = query.trim().toLowerCase()
    const list: { code: string; name: string; hex: string; base: string }[] =
      colors.length > 0
        ? colors.map((c) => ({
            code: c.color_code,
            name: c.color_name,
            hex: c.rgb_hex ?? '#888888',
            base: c.recommended_base ?? 'Base 1',
          }))
        : FALLBACK_COLORS
    return q
      ? list.filter((c) => c.name.toLowerCase().includes(q) || c.code.toLowerCase().includes(q))
      : list
  }, [colors, query])

  const shownColorants: { code: string; name: string }[] =
    colorants.length > 0 ? colorants.map((c) => ({ code: c.code, name: c.name })) : FALLBACK_COLORANTS

  const base = picked
    ? 'recommended_base' in picked && picked.recommended_base
      ? picked.recommended_base
      : 'Base 1'
    : null

  const pickedOunces: TintColorantAmount[] = shownColorants
    .filter((c) => {
      const v = ounces[c.code]
      return v != null && v !== '' && Number(v) > 0
    })
    .map((c) => ({ code: c.code, oz: ounces[c.code], shots: 0 }))

  async function applyTint() {
    if (!picked) return
    const tint: TintInfo = {
      colorCode: picked.color_code,
      colorName: picked.color_name,
      colorHex: picked.rgb_hex ?? null,
      base: base ?? 'Base 1',
      formula: pickedOunces,
    }
    // Save the formula for the customer when there is one to save.
    if (customerId && pickedOunces.length > 0) {
      try {
        const saved = await saveTintFormula({
          customer_id: customerId,
          color_code: picked.color_code,
          color_name: picked.color_name,
          base_product_id: line.product_id,
          container_volume_ml: line.containerVolumeMl ?? 3785,
          colorants: pickedOunces,
        })
        if (!saved) onToast('Formula kept on this ticket — the tint-formula endpoint did not save it (not implemented yet).')
      } catch (e) {
        onToast(`Formula kept on this ticket — save failed: ${e instanceof Error ? e.message : String(e)}`)
      }
    }
    onApply(tint)
  }

  return (
    <div className="modal-overlay" role="dialog" aria-modal="true" aria-label="Tint">
      <div className="modal tint-modal">
        <div className="modal-head">
          <h2>Tint</h2>
          <button type="button" className="modal-x" onClick={onClose} aria-label="Close tint">✕</button>
        </div>
        <p className="tint-for">
          For: {line.sku} — {line.description} ({line.sizeLabel ?? line.uom})
        </p>
        <input
          className="tint-search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search colours…"
          aria-label="Search colours"
          autoFocus
        />
        {loading && <p className="hint">Loading colours…</p>}
        <div className="color-grid">
          {shownColors.map((c) => (
            <button
              key={c.code}
              type="button"
              className={`color-cell${picked?.color_code === c.code ? ' selected' : ''}`}
              aria-pressed={picked?.color_code === c.code}
              onClick={() => {
                setPicked({ color_code: c.code, color_name: c.name, rgb_hex: c.hex, recommended_base: c.base })
                setOunces({})
                setStoredFormula(null)
              }}
            >
              <span className="color-swatch" style={{ background: c.hex }} aria-hidden="true" />
              <span className="color-name">{c.name}</span>
              <span className="color-meta">{c.code} · {c.base}</span>
            </button>
          ))}
          {shownColors.length === 0 && <p className="hint">No colours match.</p>}
        </div>

        {picked && (
          <div className="tint-detail">
            <div><span>Colour</span><strong>{picked.color_name} {picked.color_code}</strong></div>
            <div><span>Base to pull</span><strong>{base} can</strong></div>
            <div className="formula-head">
              <span>Formula (per gallon) — Gennex 0229</span>
              {storedFormula && <em>Stored formula for this customer — real ounces from the file.</em>}
              {!storedFormula && <em>No stored formula — enter ounces from the BM formula source.</em>}
            </div>
            <div className="formula-rows">
              {shownColorants.map((c) => (
                <label key={c.code} className="formula-row">
                  <span>0229{c.code}-004 · {c.code} — {c.name}</span>
                  <input
                    value={ounces[c.code] ?? ''}
                    inputMode="decimal"
                    aria-label={`Ounces of ${c.code} ${c.name}`}
                    placeholder="0 oz"
                    onChange={(e) => setOunces((prev) => ({ ...prev, [c.code]: e.target.value.replace(/[^0-9.]/g, '') }))}
                  />
                  <span>oz</span>
                </label>
              ))}
            </div>
          </div>
        )}

        <div className="modal-foot">
          <button type="button" onClick={onClose}>Cancel</button>
          <button
            type="button"
            onClick={() => onApply(null)}
          >
            No tint (stock colour)
          </button>
          <button type="button" className="complete-btn" disabled={!picked} onClick={() => void applyTint()}>
            Apply tint
          </button>
        </div>
      </div>
    </div>
  )
}
