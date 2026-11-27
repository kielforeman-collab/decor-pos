// Dashboards — five views over the real edge ledgers: Sales,
// Tenders, Products, Tint & colour, Customers. Date range + Load;
// empty states say so plainly — no sample numbers are ever shown.

import { useCallback, useEffect, useState } from 'react'
import { getDashboardAggregate, getKpiDashboard } from '../api'
import { fmtMoney, fmtQty, type DashboardAggregate, type KpiDashboard } from '../types'

type View = 'sales' | 'tenders' | 'products' | 'tint' | 'customers'

const VIEWS: { id: View; label: string }[] = [
  { id: 'sales', label: 'Sales' },
  { id: 'tenders', label: 'Tenders' },
  { id: 'products', label: 'Products' },
  { id: 'tint', label: 'Tint & colour' },
  { id: 'customers', label: 'Customers' },
]

function today(): string {
  return new Date().toISOString().slice(0, 10)
}

function pct(v: number): string {
  return `${v.toFixed(1)}%`
}

export default function DashboardsScreen({
  storeId,
  onToast,
}: {
  storeId: string | null
  onToast: (msg: string) => void
}) {
  const [view, setView] = useState<View>('sales')
  const [start, setStart] = useState(today())
  const [end, setEnd] = useState(today())
  const [kpi, setKpi] = useState<KpiDashboard | null>(null)
  const [agg, setAgg] = useState<DashboardAggregate | null>(null)
  const [loading, setLoading] = useState(false)
  const [loaded, setLoaded] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    if (!storeId) return
    if (start > end) {
      setError('Start date must be on or before the end date.')
      return
    }
    setLoading(true)
    setError(null)
    try {
      const [k, a] = await Promise.all([
        getKpiDashboard(storeId, start, end),
        getDashboardAggregate(storeId, start, end),
      ])
      setKpi(k)
      setAgg(a)
      setLoaded(true)
      if (!k && !a) onToast('Dashboard endpoints are not available yet — no data loaded.')
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [storeId, start, end, onToast])

  useEffect(() => {
    void load()
  }, [load])

  if (!storeId) return <section className="screen"><p className="screen-empty">Loading store…</p></section>

  const sales = agg?.sales ?? null
  const hasSales = sales !== null && (sales.sale_count > 0 || sales.return_count > 0)

  return (
    <section className="screen" aria-label="Dashboards">
      <div className="screen-head">
        <h2>Dashboards</h2>
        <div className="screen-actions">
          <label className="screen-date">
            From
            <input type="date" className="screen-input" value={start} aria-label="Start date" onChange={(e) => setStart(e.target.value)} />
          </label>
          <label className="screen-date">
            To
            <input type="date" className="screen-input" value={end} aria-label="End date" onChange={(e) => setEnd(e.target.value)} />
          </label>
          <button type="button" className="screen-btn screen-btn-primary" disabled={loading} onClick={() => void load()}>
            {loading ? 'Loading…' : 'Load'}
          </button>
        </div>
      </div>

      {error && (
        <div className="error" role="alert">
          <span>{error}</span>
          <button type="button" className="banner-x" aria-label="Dismiss error" onClick={() => setError(null)}>✕</button>
        </div>
      )}

      <div className="screen-tabs" role="tablist" aria-label="Dashboard views">
        {VIEWS.map((v) => (
          <button
            key={v.id}
            type="button"
            role="tab"
            aria-selected={view === v.id}
            className={view === v.id ? 'active' : ''}
            onClick={() => setView(v.id)}
          >
            {v.label}
          </button>
        ))}
      </div>

      {loaded && !kpi && !agg && (
        <p className="screen-empty">No dashboard data available for this period.</p>
      )}

      {view === 'sales' && (
        <div className="screen-panel">
          <h3>Sales — {start} to {end}</h3>
          {!hasSales ? (
            <p className="screen-empty">No sales in this period.</p>
          ) : (
            <>
              <dl className="screen-facts">
                <div><dt>Sales</dt><dd>{sales!.sale_count}</dd></div>
                <div><dt>Returns</dt><dd>{sales!.return_count}</dd></div>
                <div><dt>Gross sales</dt><dd>{fmtMoney(sales!.gross_sales)}</dd></div>
                <div><dt>Net sales</dt><dd>{fmtMoney(sales!.net_sales)}</dd></div>
                <div><dt>Grand total</dt><dd>{fmtMoney(sales!.grand_total)}</dd></div>
                <div><dt>Average ticket</dt><dd>{sales!.avg_ticket ? fmtMoney(sales!.avg_ticket) : '—'}</dd></div>
                {kpi && (
                  <>
                    <div><dt>Gross profit</dt><dd>{fmtMoney(kpi.margin.gross_profit)}</dd></div>
                    <div><dt>Margin</dt><dd>{kpi.margin.margin_pct !== null ? `${kpi.margin.margin_pct}%` : '—'}</dd></div>
                    <div><dt>Tax</dt><dd>{fmtMoney(kpi.sales.tax_total)}</dd></div>
                    <div><dt>Eco fees</dt><dd>{fmtMoney(kpi.sales.eco_fee_total)}</dd></div>
                  </>
                )}
              </dl>
              <h4>Daily sales</h4>
              {!agg || agg.daily.length === 0 ? (
                <p className="screen-empty">No daily sales in this period.</p>
              ) : (
                <table className="screen-table">
                  <thead>
                    <tr>
                      <th>Date</th>
                      <th className="num">Sales</th>
                      <th className="num">Net sales</th>
                    </tr>
                  </thead>
                  <tbody>
                    {agg.daily.map((d) => (
                      <tr key={d.date}>
                        <td>{d.date}</td>
                        <td className="num">{d.sale_count}</td>
                        <td className="num">{fmtMoney(d.net_sales)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </>
          )}
        </div>
      )}

      {view === 'tenders' && (
        <div className="screen-panel">
          <h3>Tenders — {start} to {end}</h3>
          {!agg || agg.tenders.length === 0 ? (
            <p className="screen-empty">No tenders in this period.</p>
          ) : (
            <table className="screen-table">
              <thead>
                <tr>
                  <th>Tender</th>
                  <th className="num">Count</th>
                  <th className="num">Amount</th>
                  <th className="num">Share</th>
                </tr>
              </thead>
              <tbody>
                {agg.tenders.map((t) => (
                  <tr key={t.tender}>
                    <td>{t.tender}</td>
                    <td className="num">{t.count}</td>
                    <td className="num">{fmtMoney(t.amount)}</td>
                    <td className="num">{pct(t.share_pct)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      )}

      {view === 'products' && (
        <>
          <div className="screen-panel">
            <h3>Top products — {start} to {end}</h3>
            {!agg || agg.top_products.length === 0 ? (
              <p className="screen-empty">No products sold in this period.</p>
            ) : (
              <table className="screen-table">
                <thead>
                  <tr>
                    <th>SKU</th>
                    <th>Product</th>
                    <th className="num">Qty</th>
                    <th className="num">Revenue</th>
                  </tr>
                </thead>
                <tbody>
                  {agg.top_products.map((p) => (
                    <tr key={p.product_id}>
                      <td className="mono">{p.sku}</td>
                      <td>{p.name}</td>
                      <td className="num">{fmtQty(p.qty)}</td>
                      <td className="num">{fmtMoney(p.revenue)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
          <div className="screen-panel">
            <h3>Sales by category</h3>
            {!agg || agg.by_category.length === 0 ? (
              <p className="screen-empty">No category sales in this period.</p>
            ) : (
              <table className="screen-table">
                <thead>
                  <tr>
                    <th>Category</th>
                    <th className="num">Net sales</th>
                    <th className="num">Share</th>
                  </tr>
                </thead>
                <tbody>
                  {agg.by_category.map((c) => (
                    <tr key={c.category}>
                      <td>{c.category || '—'}</td>
                      <td className="num">{fmtMoney(c.net_sales)}</td>
                      <td className="num">{pct(c.share_pct)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
          <div className="screen-panel">
            <h3>Low stock</h3>
            {!agg || agg.low_stock.length === 0 ? (
              <p className="screen-empty">Nothing below its reorder point.</p>
            ) : (
              <table className="screen-table">
                <thead>
                  <tr>
                    <th>SKU</th>
                    <th>Product</th>
                    <th className="num">On hand</th>
                    <th className="num">Min</th>
                  </tr>
                </thead>
                <tbody>
                  {agg.low_stock.map((p) => (
                    <tr key={p.product_id}>
                      <td className="mono">{p.sku}</td>
                      <td>{p.name}</td>
                      <td className="num">{fmtQty(p.on_hand)}</td>
                      <td className="num">{p.min_qty !== null ? fmtQty(p.min_qty) : '—'}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
        </>
      )}

      {view === 'tint' && (
        <div className="screen-panel">
          <h3>Tint &amp; colour — {start} to {end}</h3>
          {!agg || agg.tint.formulas_created === 0 ? (
            <p className="screen-empty">No tint formulas in this period.</p>
          ) : (
            <>
              <dl className="screen-facts">
                <div><dt>Formulas created</dt><dd>{agg.tint.formulas_created}</dd></div>
                <div><dt>Total tint cost</dt><dd>{fmtMoney(agg.tint.total_tint_cost)}</dd></div>
                {kpi && <div><dt>Formulas (KPI)</dt><dd>{kpi.formulas.formulas_created}</dd></div>}
              </dl>
              {agg.tint.per_color.length > 0 && (
                <table className="screen-table">
                  <thead>
                    <tr>
                      <th>Colour code</th>
                      <th>Colour</th>
                      <th className="num">Formulas</th>
                    </tr>
                  </thead>
                  <tbody>
                    {agg.tint.per_color.map((c) => (
                      <tr key={c.color_code}>
                        <td className="mono">{c.color_code}</td>
                        <td>{c.color_name}</td>
                        <td className="num">{c.formulas}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
              <p className="screen-note">Formula tracking only — the POS records formulas, it doesn&apos;t drive the tint machine.</p>
            </>
          )}
        </div>
      )}

      {view === 'customers' && (
        <>
          <div className="screen-panel">
            <h3>Top customers — {start} to {end}</h3>
            {!agg || agg.top_customers.length === 0 ? (
              <p className="screen-empty">No customer sales in this period.</p>
            ) : (
              <table className="screen-table">
                <thead>
                  <tr>
                    <th>Customer</th>
                    <th className="num">Sales</th>
                    <th className="num">Net sales</th>
                  </tr>
                </thead>
                <tbody>
                  {agg.top_customers.map((c) => (
                    <tr key={c.customer_id}>
                      <td>{c.name}</td>
                      <td className="num">{c.sale_count}</td>
                      <td className="num">{fmtMoney(c.net_sales)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
          <div className="screen-panel">
            <h3>Accounts receivable</h3>
            {!kpi ? (
              <p className="screen-empty">No AR data for this period.</p>
            ) : (
              <dl className="screen-facts">
                <div><dt>Total open AR</dt><dd>{fmtMoney(kpi.total_open_ar)}</dd></div>
                <div><dt>Current</dt><dd>{fmtMoney(kpi.ar_aging.current)}</dd></div>
                <div><dt>31–60 days</dt><dd>{fmtMoney(kpi.ar_aging.days_31_60)}</dd></div>
                <div><dt>61–90 days</dt><dd>{fmtMoney(kpi.ar_aging.days_61_90)}</dd></div>
                <div><dt>91–120 days</dt><dd>{fmtMoney(kpi.ar_aging.days_91_120)}</dd></div>
                <div><dt>Over 120 days</dt><dd>{fmtMoney(kpi.ar_aging.over_120)}</dd></div>
                <div><dt>On-hand value</dt><dd>{fmtMoney(kpi.inventory.on_hand_value)}</dd></div>
                <div><dt>Inventory turns</dt><dd>{kpi.inventory.turns !== null ? `${kpi.inventory.turns}x` : '—'}</dd></div>
              </dl>
            )}
            <p className="screen-note">AR as of {kpi ? kpi.as_of : end}.</p>
          </div>
        </>
      )}
    </section>
  )
}
