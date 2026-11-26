// Browser-side parsing for the GUI product import (Inventory tab).
//
// The spreadsheet is read here, in the browser — the file itself is
// never uploaded. Only the parsed rows (as JSON) are POSTed to
// `POST /pos/inventory/import`, where the backend validates them
// again and upserts by SKU. The checks here mirror the backend's
// (`df_db::product_import::validate_import_row`) so the preview
// shows what will actually be skipped; the backend stays
// authoritative and its result is shown after confirm.
//
// Expected layout: a Benjamin Moore price list. Sheet "EXCEL" when
// present, else the first sheet. First row is the header:
// SKU | Product Name | French Description | Category | French
// Category Name | Line | Color | Size | Product UPC | MSRP | MAP |
// NET Price | List Price | Brand Code | Brand Discount Y/N |
// Brand Discount | Attention: Net Price Change |
// Attention: New Catalog SKU. Prices arrive as strings like
// "$39.34" — the "$" and commas are stripped before parsing.

import * as XLSX from 'xlsx'
import type { ProductImportRow } from './types'

export interface PreviewRow {
  data: ProductImportRow
  /** Why this row will be skipped, or null when it is ready. */
  problem: string | null
}

export interface ImportPreview {
  /** Rows that will be sent (problem-free) + rows that will be skipped. */
  rows: PreviewRow[]
  /** Whole-file problem (missing sheet / missing required columns). */
  error: string | null
}

const EMPTY_ROW: Omit<ProductImportRow, 'row'> = {
  sku: '',
  name: '',
  category: '',
  line: '',
  color: '',
  size: '',
  upc: '',
  msrp: '',
  map: '',
  net_price: '',
  list_price: '',
  brand_code: '',
}

/** Header names we map, normalized (trim + lowercase) → field. */
const HEADER_MAP: Record<string, keyof Omit<ProductImportRow, 'row'>> = {
  sku: 'sku',
  'product name': 'name',
  category: 'category',
  line: 'line',
  color: 'color',
  size: 'size',
  'product upc': 'upc',
  msrp: 'msrp',
  map: 'map',
  'net price': 'net_price',
  'list price': 'list_price',
  'brand code': 'brand_code',
}

/**
 * Parse a money cell: "$39.34" / "1,234.56" / "39.34" → the amount.
 * Blank → null (no value). Anything else, or a negative, → an
 * error string. This is what catches the malformed shifted row in
 * the real price list, whose price cells hold a SKU ("K7931X-001").
 */
export function moneyCell(value: string): { amount: number | null; error: string | null } {
  const cleaned = value.replace(/[$,]/g, '').trim()
  if (cleaned === '') return { amount: null, error: null }
  const amount = Number(cleaned)
  if (!Number.isFinite(amount)) return { amount: null, error: `price ${JSON.stringify(value)} is not a valid price` }
  if (amount < 0) return { amount: null, error: `price ${JSON.stringify(value)} must not be negative` }
  return { amount, error: null }
}

/** Client-side mirror of the backend row validation. */
export function validatePreviewRow(data: ProductImportRow): string | null {
  if (!data.sku.trim()) return 'SKU is empty'
  if (!data.name.trim()) return `${data.sku.trim()}: Product Name is empty`
  const list = moneyCell(data.list_price)
  if (list.error) return `${data.sku.trim()}: List Price ${list.error}`
  if (list.amount === null) return `${data.sku.trim()}: List Price is missing`
  const net = moneyCell(data.net_price)
  if (net.error) return `${data.sku.trim()}: NET Price ${net.error}`
  if (net.amount === null) return `${data.sku.trim()}: NET Price is missing`
  const msrp = moneyCell(data.msrp)
  if (msrp.error) return `${data.sku.trim()}: MSRP ${msrp.error}`
  const map = moneyCell(data.map)
  if (map.error) return `${data.sku.trim()}: MAP ${map.error}`
  return null
}

function sheetToRows(sheet: XLSX.WorkSheet): ProductImportRow[] {
  const grid = XLSX.utils.sheet_to_json<unknown[]>(sheet, { header: 1, defval: '', raw: false })
  if (grid.length === 0) return []
  const header = (grid[0] as unknown[]).map((cell) => String(cell ?? '').trim().toLowerCase())
  const colIndex = new Map<keyof Omit<ProductImportRow, 'row'>, number>()
  for (const [name, field] of Object.entries(HEADER_MAP)) {
    const idx = header.indexOf(name)
    if (idx >= 0) colIndex.set(field, idx)
  }
  for (const required of ['sku', 'name', 'net_price', 'list_price'] as const) {
    if (!colIndex.has(required)) {
      throw new Error(`header is missing required column ${JSON.stringify(required)}`)
    }
  }
  const rows: ProductImportRow[] = []
  for (let i = 1; i < grid.length; i++) {
    const cells = grid[i] as unknown[]
    const data: ProductImportRow = { row: i + 1, ...EMPTY_ROW }
    for (const [field, idx] of colIndex) {
      data[field] = String(cells[idx] ?? '').trim()
    }
    // Skip fully blank lines (trailing empty rows in the sheet).
    const { row: _rowNo, ...fields } = data
    if (Object.values(fields).every((v) => v === '')) continue
    rows.push(data)
  }
  return rows
}

/**
 * Read a .xlsx or .csv file picked in the browser and return the
 * preview rows with per-row problems. Reads the sheet named
 * "EXCEL" when present, else the first sheet.
 */
export async function parseImportFile(file: File): Promise<ImportPreview> {
  const isCsv = file.name.toLowerCase().endsWith('.csv')
  const workbook = isCsv
    ? XLSX.read(await file.text(), { type: 'string' })
    : XLSX.read(await file.arrayBuffer(), { type: 'array' })
  const sheetName = workbook.SheetNames.includes('EXCEL') ? 'EXCEL' : workbook.SheetNames[0]
  if (!sheetName) return { rows: [], error: 'file has no sheets' }
  let parsed: ProductImportRow[]
  try {
    parsed = sheetToRows(workbook.Sheets[sheetName])
  } catch (e) {
    return { rows: [], error: e instanceof Error ? e.message : String(e) }
  }
  if (parsed.length === 0) return { rows: [], error: 'no data rows found (is the first row the header?)' }
  const seen = new Map<string, number>()
  const rows: PreviewRow[] = parsed.map((data) => {
    let problem = validatePreviewRow(data)
    const sku = data.sku.trim()
    if (!problem && sku) {
      const firstRow = seen.get(sku)
      if (firstRow !== undefined) {
        problem = `duplicate SKU in this file (first seen at row ${firstRow})`
      } else {
        seen.set(sku, data.row)
      }
    }
    return { data, problem }
  })
  return { rows, error: null }
}
