import { useEffect, useMemo, useState } from 'react'
import {
  completePickup,
  getBopisOrder,
  getCatalog,
  getHealth,
  placeBopisOrder,
} from './api'
import type { BopisOrderResponse, BopisOrderView, CatalogProduct } from './types'
import { addDecimals, fmtAmount, fmtMoney, fmtQty, mulDecimals } from './types'

type Tab = 'shop' | 'lookup'

const DEMO_STORE_ID = '00000000-0000-0000-0000-00000000a000'

function isPositiveDecimal(s: string): boolean {
  if (!/^\d+(\.\d+)?$/.test(s.trim())) return false
  const n = s.trim().replace(/\.?0+$/, '')
  return n !== '' && n !== '0'
}

export default function App() {
  const [storeId, setStoreId] = useState('')
  const [tab, setTab] = useState<Tab>('shop')
  const [notice, setNotice] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const [products, setProducts] = useState<CatalogProduct[]>([])
  const [search, setSearch] = useState('')
  const [cart, setCart] = useState<Record<string, string>>({})

  const [pickupName, setPickupName] = useState('')
  const [contact, setContact] = useState('')
  const [scheduledAt, setScheduledAt] = useState('')
  const [placed, setPlaced] = useState<BopisOrderResponse | null>(null)

  const [lookupId, setLookupId] = useState('')
  const [lookedUp, setLookedUp] = useState<BopisOrderView | null>(null)

  const say = (msg: string) => {
    setNotice(msg)
    setError(null)
  }
  const fail = (e: unknown) => {
    setError(e instanceof Error ? e.message : String(e))
    setNotice(null)
  }

  useEffect(() => {
    getHealth()
      .then((health) => setStoreId(health.store_id))
      .catch(fail)
    getCatalog().then(setProducts).catch(fail)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const visible = useMemo(() => {
    const q = search.trim().toLowerCase()
    if (!q) return products
    return products.filter(
      (p) => p.sku.toLowerCase().includes(q) || p.name.toLowerCase().includes(q),
    )
  }, [products, search])

  const cartLines = useMemo(
    () =>
      Object.entries(cart)
        .filter(([, qty]) => isPositiveDecimal(qty))
        .map(([productId, qty]) => ({
          product: products.find((p) => p.id === productId)!,
          qty,
        }))
        .filter((l) => l.product !== undefined),
    [cart, products],
  )

  // Display-only estimate from list prices. The server prices the
  // order — tax, eco fees and the final grand total come from the
  // POST /bopis/orders response, not from this number.
  const cartSubtotal = useMemo(
    () =>
      addDecimals(
        cartLines.map((l) => mulDecimals(l.qty, l.product.list_price)),
      ),
    [cartLines],
  )

  function setQty(productId: string, qty: string) {
    setCart((prev) => {
      if (qty.trim() === '') {
        const next = { ...prev }
        delete next[productId]
        return next
      }
      return { ...prev, [productId]: qty }
    })
  }

  async function doCheckout() {
    if (cartLines.length === 0) {
      setError('Add at least one item to the cart first.')
      return
    }
    if (!pickupName.trim()) {
      setError('A pickup name is required.')
      return
    }
    if (!contact.trim()) {
      setError('A contact (email or phone) is required — the store uses it for the ready notification.')
      return
    }
    if (!scheduledAt) {
      setError('Pick a pickup slot.')
      return
    }
    const slot = new Date(scheduledAt)
    if (Number.isNaN(slot.getTime())) {
      setError('That pickup slot is not a valid date and time.')
      return
    }
    if (slot.getTime() <= Date.now()) {
      setError('The pickup slot must be in the future — the server enforces this too.')
      return
    }
    setBusy(true)
    try {
      const order = await placeBopisOrder({
        lines: cartLines.map((l) => ({ product_id: l.product.id, qty: l.qty })),
        pickup_name: pickupName.trim(),
        contact: contact.trim(),
        scheduled_at: slot.toISOString(),
      })
      setPlaced(order)
      setLookupId(order.order_id)
      setCart({})
      say(
        `Order ${order.doc_number} placed — ${fmtMoney(order.grand_total)} authorized, ` +
          `pickup ${new Date(order.scheduled_at).toLocaleString()}. ` +
          `Keep the order id below to check its status.`,
      )
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doLookup(orderId?: string) {
    const id = (orderId ?? lookupId).trim()
    if (!id) {
      setError('Enter an order id to look up.')
      return
    }
    setBusy(true)
    try {
      const view = await getBopisOrder(id)
      setLookedUp(view)
      setLookupId(id)
      say(`Order ${view.order.doc_number} is ${view.order.status}.`)
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  async function doCompletePickup() {
    if (!lookedUp) return
    setBusy(true)
    try {
      const view = await completePickup(lookedUp.order.id)
      setLookedUp(view)
      say(
        `Pickup complete — order ${view.order.doc_number} is ${view.order.status} ` +
          `and ${view.lines.map((l) => `${fmtQty(l.fulfilled_qty)} × ${l.description}`).join(', ')} fulfilled.`,
      )
    } catch (e) {
      // The server refuses pickup when on-hand stock is short; the
      // order stays open. Surface that refusal as-is — do not
      // pretend the pickup happened.
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="app">
      <header>
        <h1>Decor POS — Storefront</h1>
        <div className="status">
          <span>Store: {storeId ? storeId.slice(0, 8) : '…'}</span>
          <span>Buy online, pick up in store — no account needed</span>
        </div>
      </header>

      <nav className="tabs">
        <button
          type="button"
          className={tab === 'shop' ? 'active' : ''}
          onClick={() => setTab('shop')}
        >
          Shop &amp; checkout
        </button>
        <button
          type="button"
          className={tab === 'lookup' ? 'active' : ''}
          onClick={() => setTab('lookup')}
        >
          Order status
        </button>
      </nav>

      {error && (
        <div className="error" role="alert">
          {error}
        </div>
      )}
      {notice && <div className="notice">{notice}</div>}

      <main>
        {tab === 'shop' && (
          <>
            <section>
              <h2>Catalog</h2>
              <div className="entry">
                <input
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                  placeholder="Search by SKU or name (e.g. BM-REGAL-1G)"
                  aria-label="Search catalog by SKU or name"
                />
                <button
                  type="button"
                  onClick={() => {
                    getCatalog().then(setProducts).catch(fail)
                  }}
                >
                  Refresh
                </button>
              </div>
              {products.length === 0 && (
                <p className="hint">
                  No products yet — seed the demo catalog into the edge database
                  (<code>sqlite3 edge.db &lt; apps/pos-desktop/seed-demo.sql</code>
                  {' '}then <code>apps/phase5-seed-demo.sql</code>).
                </p>
              )}
              <table>
                <thead>
                  <tr>
                    <th>SKU</th>
                    <th>Product</th>
                    <th>Unit</th>
                    <th>List price</th>
                    <th>Qty</th>
                  </tr>
                </thead>
                <tbody>
                  {visible.map((p: CatalogProduct) => (
                    <tr key={p.id}>
                      <td>{p.sku}</td>
                      <td>{p.name}</td>
                      <td>{p.base_uom}</td>
                      <td>{fmtAmount(p.list_price, 'USD')}</td>
                      <td>
                        <input
                          value={cart[p.id] ?? ''}
                          onChange={(e) => setQty(p.id, e.target.value)}
                          placeholder="0"
                          inputMode="decimal"
                          aria-label={`Quantity of ${p.sku}`}
                        />
                      </td>
                    </tr>
                  ))}
                  {products.length > 0 && visible.length === 0 && (
                    <tr>
                      <td colSpan={5}>Nothing matches “{search}”.</td>
                    </tr>
                  )}
                </tbody>
              </table>
            </section>

            <section>
              <h2>Cart — {cartLines.length} line(s)</h2>
              {cartLines.length === 0 ? (
                <p className="hint">Your cart is empty. Enter a quantity next to a product above.</p>
              ) : (
                <>
                  <ul>
                    {cartLines.map((l) => (
                      <li key={l.product.id}>
                        {fmtQty(l.qty)} × {l.product.name} ({l.product.sku}) —{' '}
                        {fmtAmount(mulDecimals(l.qty, l.product.list_price), 'USD')}
                        <button type="button" onClick={() => setQty(l.product.id, '')}>
                          Remove
                        </button>
                      </li>
                    ))}
                  </ul>
                  <p>
                    Subtotal (list prices, display only):{' '}
                    <strong>{fmtAmount(cartSubtotal, 'USD')}</strong>
                  </p>
                  <p className="hint">
                    The server prices the order — tax and eco fees are added at checkout and
                    the grand total in the confirmation is the priced total.
                  </p>
                </>
              )}

              <h3>Pickup &amp; checkout</h3>
              <label>
                Pickup name
                <input
                  value={pickupName}
                  onChange={(e) => setPickupName(e.target.value)}
                  placeholder="Who collects the order"
                />
              </label>
              <label>
                Contact (email or phone)
                <input
                  value={contact}
                  onChange={(e) => setContact(e.target.value)}
                  placeholder="For the ready notification"
                />
              </label>
              <label>
                Pickup slot
                <input
                  type="datetime-local"
                  value={scheduledAt}
                  onChange={(e) => setScheduledAt(e.target.value)}
                />
              </label>
              <p className="hint">
                Checkout is authorized by the mock payment provider in dev — no live payment
                API is called, no card is charged, and no card data is collected or stored.
                Placing the order reserves the stock for pickup.
              </p>
              <div className="actions">
                <button type="button" onClick={() => void doCheckout()} disabled={busy || cartLines.length === 0}>
                  Place pickup order
                </button>
              </div>
            </section>

            {placed && (
              <section>
                <h2>Order placed — {placed.doc_number}</h2>
                <p>
                  Order id: <code>{placed.order_id}</code>
                </p>
                <p>
                  Grand total: <strong>{fmtMoney(placed.grand_total)}</strong> · Pickup:{' '}
                  {placed.pickup_name} ({placed.contact}) · Slot:{' '}
                  {new Date(placed.scheduled_at).toLocaleString()}
                </p>
                <p>
                  Payment reference: <code>{placed.payment_reference}</code>
                </p>
                <div className="actions">
                  <button
                    type="button"
                    onClick={() => {
                      setTab('lookup')
                      void doLookup(placed.order_id)
                    }}
                  >
                    Check this order’s status
                  </button>
                </div>
              </section>
            )}
          </>
        )}

        {tab === 'lookup' && (
          <section>
            <h2>Order status</h2>
            <p className="hint">
              Look up a pickup order by its order id (shown on the confirmation after
              checkout). The counter uses the button below to hand the order over.
            </p>
            <div className="entry">
              <input
                value={lookupId}
                onChange={(e) => setLookupId(e.target.value)}
                placeholder="Order id (UUID)"
                aria-label="Order id"
              />
              <button type="button" onClick={() => void doLookup()} disabled={busy}>
                Look up order
              </button>
            </div>
            {lookedUp && (
              <>
                <h3>
                  {lookedUp.order.doc_number} — {lookedUp.order.status}
                </h3>
                <p>
                  Pickup: {lookedUp.pickup_name} ({lookedUp.contact})
                  <br />
                  Scheduled: {lookedUp.order.scheduled_at
                    ? new Date(lookedUp.order.scheduled_at).toLocaleString()
                    : '—'}
                  <br />
                  Placed: {new Date(lookedUp.order.created_at).toLocaleString()}
                  {lookedUp.order.completed_at && (
                    <>
                      <br />
                      Completed: {new Date(lookedUp.order.completed_at).toLocaleString()}
                    </>
                  )}
                </p>
                <table>
                  <thead>
                    <tr>
                      <th>Item</th>
                      <th>Qty</th>
                      <th>Unit price</th>
                      <th>Line total</th>
                      <th>Fulfilled</th>
                    </tr>
                  </thead>
                  <tbody>
                    {lookedUp.lines.map((l) => (
                      <tr key={l.id}>
                        <td>
                          {l.description} <small>{l.uom_code}</small>
                        </td>
                        <td>{fmtQty(l.qty)}</td>
                        <td>{fmtAmount(l.unit_price, lookedUp.order.currency)}</td>
                        <td>{fmtAmount(l.line_total, lookedUp.order.currency)}</td>
                        <td>{fmtQty(l.fulfilled_qty)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                <div className="totals">
                  <div>
                    <span>Subtotal</span>
                    <span>{fmtAmount(lookedUp.order.subtotal, lookedUp.order.currency)}</span>
                  </div>
                  <div>
                    <span>Tax</span>
                    <span>{fmtAmount(lookedUp.order.tax_total, lookedUp.order.currency)}</span>
                  </div>
                  <div>
                    <span>Eco fees</span>
                    <span>{fmtAmount(lookedUp.order.eco_fee_total, lookedUp.order.currency)}</span>
                  </div>
                  <div>
                    <span>Grand total</span>
                    <strong>{fmtAmount(lookedUp.order.grand_total, lookedUp.order.currency)}</strong>
                  </div>
                </div>
                <div className="actions">
                  <button
                    type="button"
                    onClick={() => void doCompletePickup()}
                    disabled={busy || lookedUp.order.status === 'completed'}
                  >
                    Complete pickup (counter)
                  </button>
                  <button type="button" onClick={() => void doLookup()} disabled={busy}>
                    Refresh status
                  </button>
                </div>
                {lookedUp.order.status === 'completed' ? (
                  <p className="hint">
                    Picked up — stock moved from committed to sold and the order is closed.
                  </p>
                ) : (
                  <p className="hint">
                    Completing the pickup moves the stock from committed to sold. If on-hand
                    stock is short, the server refuses and the order stays open.
                  </p>
                )}
              </>
            )}
          </section>
        )}
      </main>

      <footer>
        Demo store {DEMO_STORE_ID.slice(0, 8)}… · Edge 127.0.0.1:8080 (dev proxy) ·
        BOPIS lines are catalog products from stock.
      </footer>
    </div>
  )
}
