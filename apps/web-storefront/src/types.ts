// Shared JSON shapes for the BOPIS storefront API (Phase 5, v0.6).
// Money serializes as {amount: "12.3400", currency: "Usd"};
// decimals serialize as strings; datetimes as RFC 3339 strings.
// Never floats, never parseFloat for arithmetic — the helpers
// below do exact decimal math with BigInt.

export type Money = { amount: string; currency: 'Usd' | 'Cad' }

/** A catalog product: the public fields of the edge `Product` row. */
export interface CatalogProduct {
  id: string
  org_id: string
  sku: string
  manufacturer_code: string | null
  name: string
  base_uom: string
  list_price: string
  standard_cost: string
  is_active: boolean
}

export interface BopisOrderResponse {
  order_id: string
  doc_number: string
  grand_total: Money
  pickup_name: string
  contact: string
  scheduled_at: string
  payment_reference: string
}

/** An order header as stored: totals are decimal strings, and
 * `currency` is the ISO code string (e.g. "USD"). */
export interface BopisOrderHeader {
  id: string
  doc_number: string
  doc_type: string
  status: string
  currency: string
  subtotal: string
  tax_total: string
  eco_fee_total: string
  grand_total: string
  fulfillment_mode: string
  scheduled_at: string | null
  completed_at: string | null
  created_at: string
}

export interface BopisOrderLine {
  id: string
  line_no: number
  product_id: string
  description: string
  qty: string
  uom_code: string
  unit_price: string
  line_total: string
  fulfilled_qty: string
}

export interface BopisOrderView {
  order: BopisOrderHeader
  lines: BopisOrderLine[]
  pickup_name: string
  contact: string
}

// --- Exact decimal-string arithmetic (BigInt, no floats) ---------------

interface Scaled {
  coeff: bigint
  scale: number
}

function parseDecimal(s: string): Scaled {
  const m = /^(-?)(\d*)(?:\.(\d*))?$/.exec(s.trim())
  if (!m) throw new Error(`not a decimal: ${s}`)
  const sign = m[1] === '-' ? -1n : 1n
  const intPart = m[2] === '' ? '0' : m[2]
  const fracPart = m[3] ?? ''
  return { coeff: sign * BigInt(intPart + fracPart || '0'), scale: fracPart.length }
}

function align(a: Scaled, b: Scaled): [bigint, bigint, number] {
  const scale = Math.max(a.scale, b.scale)
  const pad = (x: Scaled) => x.coeff * 10n ** BigInt(scale - x.scale)
  return [pad(a), pad(b), scale]
}

/** Exact sum of decimal strings, returned at the widest input scale. */
export function addDecimals(values: string[]): string {
  let total: Scaled = { coeff: 0n, scale: 0 }
  for (const v of values) {
    const [a, b, scale] = align(total, parseDecimal(v))
    total = { coeff: a + b, scale }
  }
  return formatScaled(total)
}

/** Exact product of two decimal strings. */
export function mulDecimals(a: string, b: string): string {
  const x = parseDecimal(a)
  const y = parseDecimal(b)
  return formatScaled({ coeff: x.coeff * y.coeff, scale: x.scale + y.scale })
}

function formatScaled(v: Scaled): string {
  const sign = v.coeff < 0n ? '-' : ''
  const digits = (v.coeff < 0n ? -v.coeff : v.coeff).toString().padStart(v.scale + 1, '0')
  if (v.scale === 0) return sign + digits
  const intPart = digits.slice(0, digits.length - v.scale)
  const fracPart = digits.slice(digits.length - v.scale)
  return `${sign}${intPart}.${fracPart}`
}

/** Display a decimal string with exactly 2 fraction digits,
 * rounded half away from zero (display only). */
export function fmtDecimal2(amount: string): string {
  const v = parseDecimal(amount)
  // Round to scale 2.
  let coeff = v.coeff
  if (v.scale > 2) {
    const drop = v.scale - 2
    const divisor = 10n ** BigInt(drop)
    const rem = coeff % divisor
    coeff = coeff / divisor
    const half = divisor / 2n
    if (rem >= half || rem <= -half) coeff += coeff < 0n ? -1n : 1n
  } else {
    coeff = coeff * 10n ** BigInt(2 - v.scale)
  }
  return formatScaled({ coeff, scale: 2 })
}

export function currencySymbol(currency: string): string {
  const c = currency.toLowerCase()
  if (c === 'cad') return 'CA$'
  return '$'
}

/** Format a Money object ({amount, currency}) for display. */
export function fmtMoney(m: Money | null | undefined): string {
  if (!m) return '$0.00'
  const sign = parseDecimal(m.amount).coeff < 0n ? '-' : ''
  return `${sign}${currencySymbol(m.currency)}${fmtDecimal2(m.amount.replace(/^-/, ''))}`
}

/** Format a bare decimal-string amount with an ISO currency code
 * (the shape order headers carry: grand_total "123.4500", "USD"). */
export function fmtAmount(amount: string, currencyCode: string): string {
  const sign = parseDecimal(amount).coeff < 0n ? '-' : ''
  return `${sign}${currencySymbol(currencyCode)}${fmtDecimal2(amount.replace(/^-/, ''))}`
}

/** Display a decimal quantity string as-is, trimming trailing
 * zeros after the point (e.g. "2.0000" -> "2"). */
export function fmtQty(qty: string): string {
  if (!qty.includes('.')) return qty
  return qty.replace(/\.?0+$/, '') || '0'
}
