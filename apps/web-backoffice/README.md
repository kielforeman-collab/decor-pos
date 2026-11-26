# web-backoffice — Back-Office Admin Web (Phase 3, v0.4)

Inventory, purchasing, inter-store transfers, physical counts,
shelf tags, bulk matrix pricing, and contractor AR reads — all
against the real edge API (`df-server edge`), the same SQLite edge
database the counter POS uses. No frontend-only mocks: every view
calls the edge; the only demo data is the product list in
`src/catalog.ts` (shared with the counter), used for product names
where the edge has no row yet.

## Dev path (verified in this VM)

Terminal 1 — the edge daemon:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
cargo run -p df-server -- edge \
  --db sqlite:edge.db \
  --store-id 00000000-0000-0000-0000-00000000a000 \
  --org-id 00000000-0000-0000-0000-00000000b000 \
  --listen 127.0.0.1:8080
```

Terminal 2 — the UI (Vite proxies `/api/*` to the edge on :8080):

```sh
cd apps/web-backoffice
npm install
npm run dev   # http://localhost:1421
```

Seed the demo catalog into the edge DB first so the inventory
browser has products (same seed as the counter):

```sh
sqlite3 edge.db < apps/pos-desktop/seed-demo.sql
```

## Views

- **Inventory** — on-hand / committed / on-order / in-transit per
  product (derived from the append-only ledger), plus the min/max
  reorder-point editor (`reorder_points`, migration 0005).
- **Purchasing** — replenishment suggestions from min/max +
  trailing-30-day velocity (lead time 7 days), PO creation, and PO
  receiving (partial receives move a PO to `partially_received`).
- **Transfers** — draft a transfer, ship it (source loses on-hand,
  gains in-transit), receive it (in-transit closes, destination
  gains stock). Both stores' ledgers carry the matching entries.
- **Counts** — start a count, scan/enter counted quantities,
  reconcile (posts `count_adj` entries so on-hand equals counted).
- **Shelf Tags** — printable ZPL per product (real `df-hw` template).
- **Pricing** — bulk matrix-pricing editor; the min-margin floor
  guardrail rejects a batch that would price any product below its
  floor, and nothing changes.
- **AR** — read-only contractor AR: customer summary, 30/60/90/120
  aging, collections workspace. AR writes (payments, statements,
  finance charges) live elsewhere in Phase 3, not here.

## Notes

- The native Tauri shell is out of scope for this app (plain web
  UI, like the counter's dev path).
- Multi-store transfers name both stores explicitly; the edge
  database holds ledger rows for each side, so a transfer is
  verifiable from one edge without a second store row.
