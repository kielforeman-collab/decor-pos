// Demo catalog for the counter: quick-pick products and barcode
// lookup. These are UI-side conveniences — the edge prices lines by
// product id (matrix rules, tax, eco fees), so for eco fees and the
// matrix rule to apply, seed the same products into the edge database
// first (see apps/pos-desktop/README.md "Seed the demo catalog").
// Unseeded products still sell at their entered list price, with tax
// from the store's country/region and no eco fee.

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

export function findByBarcode(barcode: string): CatalogProduct | undefined {
  return DEMO_CATALOG.find((p) => p.barcode === barcode)
}

export function findBySku(sku: string): CatalogProduct | undefined {
  return DEMO_CATALOG.find((p) => p.sku.toLowerCase() === sku.trim().toLowerCase())
}
