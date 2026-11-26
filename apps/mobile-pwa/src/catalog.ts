// Demo catalog: the same products as the counter's quick-pick list
// (`apps/pos-desktop/src/catalog.ts`) and the back office. The app
// reads real stock from the edge; this list only supplies product
// names, SKUs and prices for pickers. Seed the same products into
// the edge database first:
// `sqlite3 edge.db < apps/pos-desktop/seed-demo.sql`.

export interface CatalogProduct {
  product_id: string
  sku: string
  barcode: string
  description: string
  uom: string
  unit_price: string // decimal string, USD
  unit_cost: string
}

export const DEMO_CATALOG: CatalogProduct[] = [
  {
    product_id: '00000000-0000-7000-8000-000000000001',
    sku: 'BM-REGAL-1G',
    barcode: '012345678905',
    description: 'Regal Select Interior Gallon',
    uom: 'gal',
    unit_price: '100.0000',
    unit_cost: '50.0000',
  },
  {
    product_id: '00000000-0000-7000-8000-000000000002',
    sku: 'BRUSH-2IN',
    barcode: '012345678912',
    description: '2 in Angle Brush',
    uom: 'ea',
    unit_price: '12.5000',
    unit_cost: '6.0000',
  },
  {
    product_id: '00000000-0000-7000-8000-000000000003',
    sku: 'ROLLER-9IN',
    barcode: '012345678929',
    description: '9 in Roller Cover',
    uom: 'ea',
    unit_price: '8.9900',
    unit_cost: '4.0000',
  },
]

export function findBySku(sku: string): CatalogProduct | undefined {
  return DEMO_CATALOG.find((p) => p.sku.toLowerCase() === sku.trim().toLowerCase())
}
