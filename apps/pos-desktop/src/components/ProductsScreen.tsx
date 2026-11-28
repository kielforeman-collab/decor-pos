// Products — search, on-hand stock, detail (prices, price levels,
// reorder point, warehouse stock) and the GUI product import.
// The import parses the Benjamin Moore price list in the browser
// (see productImport.ts) — the file itself is never uploaded — and
// POSTs only the parsed rows, upserting by SKU.

import { useCallback, useEffect, useRef, useState } from 'react'
import {
  getPriceLevels,
  importProducts,
  inventoryBalances,
  listReorderPoints,
  searchProducts,
  setReorderPoint,
  type InventoryBalance,
  type ReorderPoint,
} from '../api'
import { getShelfTag } from '../backofficeApi'
import type { ShelfTagResponse } from '../backofficeTypes'
import { parseImportFile, type ImportPreview } from '../productImport'
import PricingRulesPanel from './PricingRulesPanel'
import {
  cad,
  discountedPrice,
  fmtMoney,
  fmtQty,
  sizeLabelForVolume,
  type PriceLevel,
  type ProductImportResponse,
  type ProductSearchResult,
} from '../types'

const ALDERGROVE_NOTE =
  'Aldergrove figure is an illustrative sample, not a live count. There is no live BM inventory feed yet — the Retailer Gateway is a login web portal only, and BM EDI is 850 purchase orders out / 810 invoices back, with no inventory feed.'

interface DetailProduct {
  product_id: string
  sku: string
  name: string
  base_uom: string
  list_price: string
  standard_cost: string
  msrp: string | null
  map: string | null
  container_volume_ml: number | null
  on_hand: string | null
}

function detailFromBalance(b: InventoryBalance): DetailProduct {
  return {
    product_id: b.product_id,
    sku: b.sku,
    name: b.name,
    base_uom: b.base_uom,
    list_price: b.list_price,
    standard_cost: b.standard_cost,
    msrp: b.msrp,
    map: b.map,
    container_volume_ml: null,
    on_hand: b.on_hand,
  }
}

function detailFromSearch(p: ProductSearchResult): DetailProduct {
  return {
    product_id: p.product_id,
    sku: p.sku,
    name: p.name,
    base_uom: p.base_uom,
    list_price: p.list_price,
    standard_cost: p.standard_cost,
    msrp: p.msrp,
    map: p.map,
    container_volume_ml: p.container_volume_ml,
    on_hand: p.on_hand,
  }
}

export default function ProductsScreen({
  storeId,
  onToast,
}: {
  storeId: string | null
  onToast: (msg: string) => void
}) {
  const [balances, setBalances] = useState<InventoryBalance[]>([])
  const [levels, setLevels] = useState<PriceLevel[]>([])
  const [points, setPoints] = useState<ReorderPoint[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const [q, setQ] = useState('')
  const [results, setResults] = useState<ProductSearchResult[]>([])
  const [selected, setSelected] = useState<DetailProduct | null>(null)

  const [minQty, setMinQty] = useState('')
  const [maxQty, setMaxQty] = useState('')
  const [reorderQty, setReorderQty] = useState('')
  const [savingPoint, setSavingPoint] = useState(false)

  const [subView, setSubView] = useState<'catalog' | 'pricing'>('catalog')
  const [shelfTag, setShelfTag] = useState<ShelfTagResponse | null>(null)
  const [loadingTag, setLoadingTag] = useState(false)

  const [importFileName, setImportFileName] = useState('')
  const [importPreview, setImportPreview] = useState<ImportPreview | null>(null)
  const [importResult, setImportResult] = useState<ProductImportResponse | null>(null)
  const [importing, setImporting] = useState(false)
  const importInputRef = useRef<HTMLInputElement>(null)

  const load = useCallback(async () => {
    if (!storeId) return
    setLoading(true)
    setError(null)
    try {
      const [b, l, p] = await Promise.all([
        inventoryBalances(storeId),
        getPriceLevels(),
        listReorderPoints(storeId),
      ])
      setBalances(b)
      setLevels(l)
      setPoints(p)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [storeId])

  useEffect(() => {
    void load()
  }, [load])

  // Debounced product search.
  useEffect(() => {
    if (!q.trim()) {
      setResults([])
      return
    }
    const h = setTimeout(() => {
      searchProducts({ q: q.trim(), limit: 8 })
        .then(setResults)
        .catch(() => setResults([]))
    }, 200)
    return () => clearTimeout(h)
  }, [q])

  const point = selected ? points.find((p) => p.product_id === selected.product_id) ?? null : null

  // Prefill the reorder-point form whenever the selection changes.
  useEffect(() => {
    if (!selected) return
    const p = points.find((x) => x.product_id === selected.product_id)
    setMinQty(p?.min_qty ?? '')
    setMaxQty(p?.max_qty ?? '')
    setReorderQty(p?.reorder_qty ?? '')
  }, [selected, points])

  function selectBalance(b: InventoryBalance) {
    setSelected(detailFromBalance(b))
    // Enrich with the container size, which balances don't carry.
    searchProducts({ sku: b.sku, limit: 1 })
      .then((found) => {
        if (found.length > 0) setSelected((prev) => (prev && prev.product_id === b.product_id ? detailFromSearch(found[0]) : prev))
      })
      .catch(() => {
        // Detail already shows the balance data.
      })
  }

  async function doSavePoint() {
    if (!storeId || !selected) return
    if (minQty.trim() === '' || maxQty.trim() === '') {
      setError('Min and max quantities are required for a reorder point.')
      return
    }
    setSavingPoint(true)
    setError(null)
    try {
      const saved = await setReorderPoint(storeId, selected.product_id, minQty, maxQty, reorderQty || '0')
      setPoints((prev) => {
        const rest = prev.filter((p) => p.product_id !== selected.product_id)
        return [...rest, saved]
      })
      onToast(`Reorder point saved for ${selected.sku}.`)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setSavingPoint(false)
    }
  }

  async function handleImportFilePicked(file: File) {
    setImportFileName(file.name)
    setImportResult(null)
    setImportPreview(null)
    setError(null)
    try {
      setImportPreview(await parseImportFile(file))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  async function doConfirmImport() {
    if (!importPreview) return
    const ready = importPreview.rows.filter((r) => !r.problem)
    if (ready.length === 0) return
    setImporting(true)
    setError(null)
    try {
      const res = await importProducts(ready.map((r) => r.data))
      setImportResult(res)
      setImportPreview(null)
      onToast(`Import done — ${res.created} created, ${res.updated} updated, ${res.failed} failed.`)
      await load()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setImporting(false)
    }
  }

  if (!storeId) return <section className="screen"><p className="screen-empty">Loading store…</p></section>

  const filtered = q.trim()
    ? balances.filter(
        (b) =>
          b.sku.toLowerCase().includes(q.trim().toLowerCase()) ||
          b.name.toLowerCase().includes(q.trim().toLowerCase()),
      )
    : balances
  const readyRows = importPreview ? importPreview.rows.filter((r) => !r.problem) : []
  const skippedRows = importPreview ? importPreview.rows.filter((r) => r.problem) : []

  return (
    <section className="screen" aria-label="Products">
      <div className="screen-head">
        <h2>Products</h2>
        <div className="screen-tabs" role="tablist">
          <button
            type="button"
            role="tab"
            aria-selected={subView === 'catalog'}
            className={subView === 'catalog' ? 'active' : ''}
            onClick={() => setSubView('catalog')}
          >
            Catalog &amp; stock
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={subView === 'pricing'}
            className={subView === 'pricing' ? 'active' : ''}
            onClick={() => setSubView('pricing')}
          >
            Pricing rules
          </button>
        </div>
        {subView === 'catalog' && (
          <div className="screen-actions">
            <button type="button" className="screen-btn" onClick={() => importInputRef.current?.click()} disabled={importing}>
              Import products…
            </button>
            <input
              ref={importInputRef}
              type="file"
              accept=".xlsx,.csv"
              style={{ display: 'none' }}
              onChange={(e) => {
                const f = e.target.files?.[0]
                if (f) void handleImportFilePicked(f)
                e.target.value = ''
              }}
            />
            <button type="button" className="screen-btn" disabled={loading} onClick={() => void load()}>
              Refresh
            </button>
          </div>
        )}
      </div>

      {subView === 'pricing' ? (
        <PricingRulesPanel onToast={onToast} />
      ) : (
        <>

      {error && (
        <div className="error" role="alert">
          <span>{error}</span>
          <button type="button" className="banner-x" aria-label="Dismiss error" onClick={() => setError(null)}>✕</button>
        </div>
      )}

      <div className="screen-form-row">
        <label>
          Search products
          <input
            type="search"
            className="screen-input"
            value={q}
            placeholder="SKU or product name…"
            aria-label="Search products"
            onChange={(e) => setQ(e.target.value)}
          />
        </label>
        {q.trim() && results.length > 0 && (
          <span className="screen-note-inline">
            {results.length} match{results.length === 1 ? '' : 'es'} — pick one below or in the list.
          </span>
        )}
      </div>
      {q.trim() && results.length > 0 && (
        <div className="screen-panel">
          <table className="screen-table screen-table-clickable">
            <thead>
              <tr>
                <th>SKU</th>
                <th>Product</th>
                <th className="num">List price</th>
              </tr>
            </thead>
            <tbody>
              {results.map((r) => (
                <tr key={r.product_id} onClick={() => setSelected(detailFromSearch(r))} className={selected?.product_id === r.product_id ? 'selected' : ''}>
                  <td className="mono">{r.sku}</td>
                  <td>{r.name}</td>
                  <td className="num">{fmtMoney(cad(r.list_price))}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {importFileName && (
        <div className="screen-panel">
          <h3>Import products — {importFileName}</h3>
          <p className="screen-note">
            Reads a Benjamin Moore price list (.xlsx or .csv) in the browser — the file itself is never
            uploaded. Products match by SKU: new SKUs are created, existing SKUs get new prices. Category,
            UPC, MSRP and MAP are not stored (no columns for them).
          </p>
          {importPreview?.error && <p className="screen-empty">{importPreview.error}</p>}
          {importPreview && !importPreview.error && (
            <>
              <p className="screen-meta">
                {importPreview.rows.length} row(s) parsed — {readyRows.length} ready, {skippedRows.length} will be skipped.
              </p>
              {readyRows.length > 0 && (
                <table className="screen-table">
                  <thead>
                    <tr>
                      <th>SKU</th>
                      <th>Product</th>
                      <th className="num">List price</th>
                      <th className="num">Net price</th>
                    </tr>
                  </thead>
                  <tbody>
                    {readyRows.slice(0, 5).map((r) => (
                      <tr key={r.data.row}>
                        <td className="mono">{r.data.sku}</td>
                        <td>{r.data.name}</td>
                        <td className="num">{r.data.list_price}</td>
                        <td className="num">{r.data.net_price}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
              {skippedRows.length > 0 && (
                <>
                  <h4>Rows that will be skipped</h4>
                  <table className="screen-table">
                    <thead>
                      <tr>
                        <th>Row</th>
                        <th>SKU</th>
                        <th>Reason</th>
                      </tr>
                    </thead>
                    <tbody>
                      {skippedRows.slice(0, 20).map((r) => (
                        <tr key={r.data.row}>
                          <td className="num">{r.data.row}</td>
                          <td className="mono">{r.data.sku || '—'}</td>
                          <td>{r.problem}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                  {skippedRows.length > 20 && (
                    <p className="screen-note">…and {skippedRows.length - 20} more skipped row(s).</p>
                  )}
                </>
              )}
              <div className="screen-actions">
                <button type="button" className="screen-btn screen-btn-primary" disabled={importing || readyRows.length === 0} onClick={() => void doConfirmImport()}>
                  Confirm import ({readyRows.length} rows)
                </button>
                <button type="button" className="screen-btn screen-btn-ghost" onClick={() => { setImportPreview(null); setImportFileName('') }}>
                  Cancel
                </button>
              </div>
            </>
          )}
          {importResult && (
            <p className="screen-meta">
              Import done: {importResult.created} created, {importResult.updated} updated, {importResult.failed} failed.
              {importResult.failures.length > 0 && (
                <>
                  {' '}
                  {importResult.failures.map((f) => `${f.sku || 'row ' + f.row}: ${f.reason}`).join('; ')}
                </>
              )}
            </p>
          )}
        </div>
      )}

      <div className="screen-split">
        <div className="screen-panel">
          <h3>Inventory — {filtered.length} product{filtered.length === 1 ? '' : 's'}</h3>
          {loading && balances.length === 0 ? (
            <p className="screen-empty">Loading…</p>
          ) : filtered.length === 0 ? (
            <p className="screen-empty">No products yet — import a price list above.</p>
          ) : (
            <table className="screen-table screen-table-clickable">
              <thead>
                <tr>
                  <th>SKU</th>
                  <th>Product</th>
                  <th className="num">On hand</th>
                  <th className="num">List</th>
                </tr>
              </thead>
              <tbody>
                {filtered.map((b) => (
                  <tr
                    key={b.product_id}
                    className={selected?.product_id === b.product_id ? 'selected' : ''}
                    onClick={() => selectBalance(b)}
                  >
                    <td className="mono">{b.sku}</td>
                    <td>{b.name}</td>
                    <td className="num">{fmtQty(b.on_hand)}</td>
                    <td className="num">{fmtMoney(cad(b.list_price))}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>

        <div className="screen-panel">
          <h3>Product detail</h3>
          {!selected ? (
            <p className="screen-empty">Pick a product on the left.</p>
          ) : (
            <>
              <dl className="screen-facts">
                <div><dt>SKU</dt><dd className="mono">{selected.sku}</dd></div>
                <div><dt>Product</dt><dd>{selected.name}</dd></div>
                <div><dt>List price</dt><dd>{fmtMoney(cad(selected.list_price))}</dd></div>
                <div><dt>Standard cost (NET)</dt><dd>{fmtMoney(cad(selected.standard_cost))}</dd></div>
                <div><dt>MSRP</dt><dd>{selected.msrp ? fmtMoney(cad(selected.msrp)) : '—'}</dd></div>
                <div><dt>MAP</dt><dd>{selected.map ? fmtMoney(cad(selected.map)) : '—'}</dd></div>
                <div><dt>Container size</dt><dd>{sizeLabelForVolume(selected.container_volume_ml, selected.base_uom) ?? '—'}</dd></div>
                <div><dt>On hand</dt><dd>{selected.on_hand != null ? fmtQty(selected.on_hand) : '—'}</dd></div>
              </dl>

              <h4>Price levels</h4>
              {levels.length === 0 ? (
                <p className="screen-empty">No price levels loaded.</p>
              ) : (
                <table className="screen-table">
                  <thead>
                    <tr>
                      <th>Level</th>
                      <th className="num">Discount</th>
                      <th className="num">Price</th>
                    </tr>
                  </thead>
                  <tbody>
                    {levels.map((l) => (
                      <tr key={l.id}>
                        <td>{l.name}</td>
                        <td className="num">{Number(l.discount_pct) > 0 ? `${(Number(l.discount_pct) * 100).toFixed(0)}%` : '—'}</td>
                        <td className="num">{fmtMoney(cad(discountedPrice(selected.list_price, String(Number(l.discount_pct) * 100))))}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}

              <h4>Reorder point</h4>
              <div className="screen-form-row">
                <label>
                  Min
                  <input type="text" inputMode="decimal" className="screen-input screen-qty" value={minQty} aria-label="Minimum quantity" onChange={(e) => setMinQty(e.target.value)} />
                </label>
                <label>
                  Max
                  <input type="text" inputMode="decimal" className="screen-input screen-qty" value={maxQty} aria-label="Maximum quantity" onChange={(e) => setMaxQty(e.target.value)} />
                </label>
                <label>
                  Reorder qty
                  <input type="text" inputMode="decimal" className="screen-input screen-qty" value={reorderQty} aria-label="Reorder quantity" onChange={(e) => setReorderQty(e.target.value)} />
                </label>
                <button type="button" className="screen-btn screen-btn-primary" disabled={savingPoint} onClick={() => void doSavePoint()}>
                  Save reorder point
                </button>
              </div>
              {point && (
                <p className="screen-note">Saved point: min {fmtQty(point.min_qty)} · max {fmtQty(point.max_qty)} · reorder {fmtQty(point.reorder_qty)}</p>
              )}

              <div className="screen-sub">
                <h4>Shelf tag (ZPL)</h4>
                <div className="screen-actions">
                  <button
                    type="button"
                    className="screen-btn"
                    disabled={loadingTag}
                    onClick={() => {
                      setLoadingTag(true)
                      setError(null)
                      void getShelfTag(selected.product_id)
                        .then((tag) => {
                          setShelfTag(tag)
                          onToast(`Shelf tag ZPL generated for ${tag.sku}.`)
                        })
                        .catch((e) => setError(e instanceof Error ? e.message : String(e)))
                        .finally(() => setLoadingTag(false))
                    }}
                  >
                    {loadingTag ? 'Generating…' : 'Generate shelf tag (ZPL)'}
                  </button>
                  {shelfTag && shelfTag.product_id === selected.product_id && (
                    <button
                      type="button"
                      className="screen-btn"
                      onClick={() => {
                        void navigator.clipboard?.writeText(shelfTag.zpl)
                        onToast('ZPL copied to clipboard.')
                      }}
                    >
                      Copy ZPL
                    </button>
                  )}
                </div>
                {shelfTag && shelfTag.product_id === selected.product_id && (
                  <pre className="screen-pre">{shelfTag.zpl}</pre>
                )}
              </div>

              <h4>BM warehouse — Aldergrove</h4>
              <dl className="screen-facts">
                <div><dt>On hand</dt><dd>—</dd></div>
              </dl>
              <p className="screen-note">
                <strong>ILLUSTRATIVE SAMPLE</strong> — {ALDERGROVE_NOTE}
              </p>
            </>
          )}
        </div>
      </div>
        </>
      )}
    </section>
  )
}
