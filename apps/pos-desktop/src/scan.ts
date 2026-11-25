// Scanner parsing, mirroring df-hw `parse_scanner_input` exactly:
// scanners type the barcode plus CR/LF framing; an optional
// `qty*barcode` prefix typed at the counter multiplies the quantity.
// The Tauri backend runs the real Rust parser via the `scan_barcode`
// command; this TS copy is the dev-path (HTTP) fallback, kept
// behavior-identical on purpose.

export interface ParsedScan {
  qty: number
  barcode: string
}

export function parseScannerInput(input: string): ParsedScan | null {
  const trimmed = input.trim()
  if (!trimmed) return null
  const star = trimmed.indexOf('*')
  if (star > 0) {
    const qtyPart = trimmed.slice(0, star).trim()
    const qty = Number(qtyPart)
    if (Number.isFinite(qty) && qty > 0 && /^\d+(\.\d+)?$/.test(qtyPart)) {
      return { qty, barcode: trimmed.slice(star + 1).trim() }
    }
  }
  return { qty: 1, barcode: trimmed }
}
