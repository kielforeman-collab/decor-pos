import { useEffect, useRef, useState } from 'react'
import { searchProducts } from '../api'
import { fmtMoney, cad, type ProductSearchResult } from '../types'

interface ProductSearchProps {
  inputRef?: React.RefObject<HTMLInputElement | null>
  placeholder?: string
  onPick: (p: ProductSearchResult) => void
  onManualText?: (text: string) => void
  /** Enter with no dropdown results (e.g. a scanner dump): the caller
   * resolves the raw text as a barcode/SKU. */
  onEnterNoResult?: (text: string) => void
  autoFocus?: boolean
}

/**
 * Entry search: SKU/description substring, UPC prefix (≥6 digits).
 * Results cap 8. ArrowUp/Down highlight, Enter adds, Esc closes.
 * Shows 'List $x · NET $x' (+ MAP/MSRP when present).
 */
export default function ProductSearch({
  inputRef,
  placeholder = 'SKU, UPC, or product name',
  onPick,
  onManualText,
  onEnterNoResult,
  autoFocus,
}: ProductSearchProps) {
  const [query, setQuery] = useState('')
  const [results, setResults] = useState<ProductSearchResult[]>([])
  const [open, setOpen] = useState(false)
  const [highlight, setHighlight] = useState(0)
  const [searching, setSearching] = useState(false)
  const localRef = useRef<HTMLInputElement>(null)
  const ref = inputRef ?? localRef
  const boxRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const q = query.trim()
    if (q.length < 2) {
      setResults([])
      setOpen(false)
      return
    }
    setSearching(true)
    const h = window.setTimeout(() => {
      const isUpc = /^\d{6,}$/.test(q)
      const call = isUpc
        ? searchProducts({ barcode: q, limit: 8 })
        : searchProducts({ q, limit: 8 })
      void call
        .then((r) => {
          setResults(r.slice(0, 8))
          setOpen(true)
          setHighlight(0)
        })
        .catch(() => {
          setResults([])
          setOpen(true)
        })
        .finally(() => setSearching(false))
    }, 200)
    return () => window.clearTimeout(h)
  }, [query])

  useEffect(() => {
    const onDoc = (e: MouseEvent) => {
      if (boxRef.current && !boxRef.current.contains(e.target as Node)) setOpen(false)
    }
    document.addEventListener('mousedown', onDoc)
    return () => document.removeEventListener('mousedown', onDoc)
  }, [])

  const pick = (p: ProductSearchResult) => {
    onPick(p)
    setQuery('')
    setResults([])
    setOpen(false)
    ref.current?.focus()
  }

  return (
    <div className="product-search" ref={boxRef}>
      <input
        ref={ref}
        value={query}
        onChange={(e) => {
          setQuery(e.target.value)
          onManualText?.(e.target.value)
        }}
        onFocus={() => {
          if (results.length > 0) setOpen(true)
        }}
        onKeyDown={(e) => {
          if (e.key === 'ArrowDown' && results.length > 0) {
            e.preventDefault()
            setHighlight((h) => Math.min(h + 1, results.length - 1))
          } else if (e.key === 'ArrowUp' && results.length > 0) {
            e.preventDefault()
            setHighlight((h) => Math.max(h - 1, 0))
          } else if (e.key === 'Enter') {
            if (results.length > 0) {
              e.preventDefault()
              pick(results[highlight] ?? results[0])
            } else if (onEnterNoResult && query.trim()) {
              e.preventDefault()
              const text = query.trim()
              setQuery('')
              setOpen(false)
              onEnterNoResult(text)
            }
          } else if (e.key === 'Escape') {
            setOpen(false)
            ref.current?.blur()
          }
        }}
        placeholder={placeholder}
        aria-label={placeholder}
        autoFocus={autoFocus}
      />
      {open && (
        <div className="search-results" role="listbox">
          {searching && <div className="search-status">Searching…</div>}
          {!searching && results.length === 0 && (
            <div className="search-status">No match — use manual entry below, or scan again.</div>
          )}
          {results.map((r, i) => (
            <button
              key={r.product_id}
              type="button"
              role="option"
              aria-selected={i === highlight}
              className={`search-result${i === highlight ? ' hl' : ''}`}
              onClick={() => pick(r)}
            >
              <span className="sr-sku">{r.sku}</span>
              <span className="sr-name">{r.name}</span>
              <span className="sr-price">
                List {fmtMoney(cad(r.list_price))} · NET {fmtMoney(cad(r.standard_cost))}
                {r.map ? ` · MAP ${fmtMoney(cad(r.map))}` : ''}
                {r.msrp ? ` · MSRP ${fmtMoney(cad(r.msrp))}` : ''}
              </span>
              {r.on_hand != null && <span className="sr-stock">On hand {r.on_hand}</span>}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}
