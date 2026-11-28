import { cad, discountedPrice, fmtMoney, mulDecimals, type CartLine, type PricePreview } from '../types'

interface TicketProps {
  lines: CartLine[]
  preview: PricePreview | null
  onQty: (key: string, qty: string) => void
  onDisc: (key: string, pct: string) => void
  onRemove: (key: string) => void
  onTint: (line: CartLine) => void
  onClearTint: (key: string) => void
}

/** Line amount: the server preview is authoritative; before it
 * arrives the cell shows the discounted line amount as provisional. */
function lineAmount(l: CartLine, index: number, preview: PricePreview | null): string {
  const pv = preview?.lines[index]
  if (pv && pv.product_id === l.product_id) return fmtMoney(pv.line_total)
  return fmtMoney(cad(mulDecimals(discountedPrice(l.unit_price, l.discPct), l.qty || '0')))
}

export default function Ticket({ lines, preview, onQty, onDisc, onRemove, onTint, onClearTint }: TicketProps) {
  return (
    <div className="ticket-wrap">
      <table className="ticket mono">
        <thead>
          <tr>
            <th className="c-num">#</th>
            <th className="c-sku">SKU</th>
            <th className="c-desc">Description</th>
            <th className="c-retail">Retail</th>
            <th className="c-disc">Disc %</th>
            <th className="c-qty">Qty</th>
            <th className="c-uom">UOM</th>
            <th className="c-amt">Amount</th>
            <th className="c-x"><span aria-hidden="true">✕</span><span className="sr-only">Remove</span></th>
          </tr>
        </thead>
        <tbody>
          {lines.map((l, i) => (
            <tr key={l.key} className="lrow">
              <td className="c-num">{i + 1}</td>
              <td className="c-sku">{l.sku}</td>
              <td className="c-desc">
                {l.description}
                {l.sizeLabel && <span className="size-tag">{l.sizeLabel}</span>}
                {l.tint && (
                  <span className="tint-line">
                    <span className="swatch" style={{ background: l.tint.colorHex ?? '#888888' }} aria-hidden="true" />
                    TINT: {l.tint.colorName} {l.tint.colorCode} · {l.tint.base} ·{' '}
                    {l.tint.formula.length > 0
                      ? `Formula: ${l.tint.formula.map((f) => `${f.code} ${f.oz} oz`).join(', ')}`
                      : 'Formula on file'}
                    <button type="button" className="link-btn" onClick={() => onTint(l)}>Edit tint</button>
                    <button type="button" className="link-btn" onClick={() => onClearTint(l.key)}>Remove tint</button>
                  </span>
                )}
                {!l.tint && l.containerVolumeMl != null && (
                  <span className="tint-line untinted">
                    UNTINTED — stock colour
                    <button type="button" className="link-btn" onClick={() => onTint(l)}>Add tint / formula</button>
                  </span>
                )}
              </td>
              <td className="c-retail">{fmtMoney(cad(l.unit_price))}</td>
              <td className="c-disc">
                <input
                  className="disc-input"
                  value={l.discPct}
                  inputMode="decimal"
                  aria-label={`Discount percent for ${l.description}`}
                  onChange={(e) => {
                    const v = e.target.value.replace(/[^0-9.]/g, '')
                    onDisc(l.key, v === '' ? '0' : String(Math.min(90, Math.max(0, Number(v) || 0))))
                  }}
                />
              </td>
              <td className="c-qty">
                <span className="qty-stepper">
                  <button
                    type="button"
                    aria-label={`Decrease quantity of ${l.description}`}
                    onClick={() => onQty(l.key, String(Math.max(0, Number(l.qty || '0') - 1)))}
                  >
                    −
                  </button>
                  <input
                    value={l.qty}
                    inputMode="decimal"
                    aria-label={`Quantity for ${l.description}`}
                    onChange={(e) => onQty(l.key, e.target.value)}
                  />
                  <button
                    type="button"
                    aria-label={`Increase quantity of ${l.description}`}
                    onClick={() => onQty(l.key, String(Number(l.qty || '0') + 1))}
                  >
                    +
                  </button>
                </span>
              </td>
              <td className="c-uom">{l.uom}</td>
              <td className="c-amt">{lineAmount(l, i, preview)}</td>
              <td className="c-x">
                <button type="button" aria-label={`Remove ${l.description}`} onClick={() => onRemove(l.key)}>✕</button>
              </td>
            </tr>
          ))}
          {lines.length === 0 && (
            <tr>
              <td colSpan={9} className="ticket-empty">No lines yet — scan, search, or enter a line.</td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
  )
}
