// Shared bits for the Orders / Transfers / Accounts screens: a
// product search picker and a compact line editor. Same tokens as
// the register — no hard-coded colours, no per-theme branches.

import { useEffect, useRef, useState } from 'react'
import { searchProducts } from '../api'
import { cad, fmtMoney, type ProductSearchResult } from '../types'

export interface DraftLine {
  key: string
  product_id: string
  sku: string
  name: string
  qty: string
  /** '' = leave to the server default (PO unit cost). */
  unit_cost: string
}

export function newDraftLine(p: ProductSearchResult): DraftLine {
  return {
    key: `${p.product_id}-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
    product_id: p.product_id,
    sku: p.sku,
    name: p.name,
    qty: '1',
    unit_cost: '',
  }
}

export function ProductPicker({
  onAdd,
  disabled,
  placeholder = 'Search products by SKU or name…',
}: {
  onAdd: (p: ProductSearchResult) => void
  disabled?: boolean
  placeholder?: string
}) {
  const [q, setQ] = useState('')
  const [results, setResults] = useState<ProductSearchResult[]>([])
  const [open, setOpen] = useState(false)
  const boxRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!q.trim()) {
      setResults([])
      setOpen(false)
      return
    }
    const h = setTimeout(() => {
      searchProducts({ q: q.trim(), limit: 8 })
        .then((r) => {
          setResults(r)
          setOpen(true)
        })
        .catch(() => setResults([]))
    }, 200)
    return () => clearTimeout(h)
  }, [q])

  useEffect(() => {
    const close = (e: MouseEvent) => {
      if (boxRef.current && !boxRef.current.contains(e.target as Node)) setOpen(false)
    }
    document.addEventListener('mousedown', close)
    return () => document.removeEventListener('mousedown', close)
  }, [])

  return (
    <div className="screen-picker" ref={boxRef}>
      <input
        type="search"
        className="screen-input"
        value={q}
        disabled={disabled}
        placeholder={placeholder}
        aria-label="Search products"
        onChange={(e) => setQ(e.target.value)}
        onFocus={() => results.length > 0 && setOpen(true)}
      />
      {open && results.length > 0 && (
        <ul className="screen-picker-results" role="listbox">
          {results.map((r) => (
            <li key={r.product_id}>
              <button
                type="button"
                onClick={() => {
                  onAdd(r)
                  setQ('')
                  setResults([])
                  setOpen(false)
                }}
              >
                <span className="screen-picker-sku">{r.sku}</span>
                <span className="screen-picker-name">{r.name}</span>
                <span className="screen-picker-price">{fmtMoney(cad(r.list_price))}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
      {open && q.trim() && results.length === 0 && (
        <div className="screen-picker-empty">No products match “{q.trim()}”.</div>
      )}
    </div>
  )
}

export function LineEditor({
  lines,
  onChange,
  onRemove,
  showCost,
}: {
  lines: DraftLine[]
  onChange: (key: string, patch: Partial<DraftLine>) => void
  onRemove: (key: string) => void
  showCost?: boolean
}) {
  if (lines.length === 0) return <p className="screen-empty">No lines yet — search above to add products.</p>
  return (
    <table className="screen-table">
      <thead>
        <tr>
          <th>SKU</th>
          <th>Product</th>
          <th className="num">Qty</th>
          {showCost && <th className="num">Unit cost</th>}
          <th aria-label="Remove" />
        </tr>
      </thead>
      <tbody>
        {lines.map((l) => (
          <tr key={l.key}>
            <td className="mono">{l.sku}</td>
            <td>{l.name}</td>
            <td className="num">
              <input
                type="text"
                inputMode="decimal"
                className="screen-input screen-qty"
                value={l.qty}
                aria-label={`Quantity for ${l.sku}`}
                onChange={(e) => onChange(l.key, { qty: e.target.value })}
              />
            </td>
            {showCost && (
              <td className="num">
                <input
                  type="text"
                  inputMode="decimal"
                  className="screen-input screen-qty"
                  value={l.unit_cost}
                  placeholder="default"
                  aria-label={`Unit cost for ${l.sku}`}
                  onChange={(e) => onChange(l.key, { unit_cost: e.target.value })}
                />
              </td>
            )}
            <td>
              <button type="button" className="screen-btn screen-btn-ghost" onClick={() => onRemove(l.key)} aria-label={`Remove ${l.sku}`}>
                ✕
              </button>
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}
