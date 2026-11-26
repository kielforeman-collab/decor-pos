# web-portal — 24/7 Contractor Portal (Phase 5, v0.6)

Self-serve portal for charge-account contractors, against the real
edge API (`df-server edge`). No separate portal login: the customer
id IS the scope, and every call re-checks `portal_enabled`
server-side (403 otherwise). No frontend-only mocks: every view
calls the edge.

## Dev path (verified in this VM)

Terminal 1 — the edge daemon, on a temp database seeded with the
demo store + the Phase 5 demo contractor:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
mkdir -p /tmp/portal-verify && cd /tmp/portal-verify
cargo run -p df-server -- edge \
  --db sqlite:edge.db \
  --store-id 00000000-0000-0000-0000-00000000a000 \
  --org-id 00000000-0000-0000-0000-00000000b000 \
  --listen 127.0.0.1:8080
# in another shell, before starting the edge (or stop it first):
sqlite3 edge.db < apps/pos-desktop/seed-demo.sql
sqlite3 edge.db < apps/phase5-seed-demo.sql
```

Terminal 2 — the UI (Vite proxies `/api/*` to the edge on :8080):

```sh
cd apps/web-portal
npm install
npm run dev   # http://localhost:1423
```

Sign in with the demo contractor id
`00000000-0000-7000-8000-000000000101`, or open
`http://localhost:1423/?customer=00000000-0000-7000-8000-000000000101`.
The id is remembered in `localStorage` (`portal-customer-id`).

## Views

- **Overview** — customer details + open AR balance.
- **Invoices** — the contractor's transactions, newest first
  (doc number, type, status, grand total, created).
- **Statements** — frozen month-end statements (period, opening,
  charges, payments, closing). Empty until a month-end statement
  run exists for the customer.
- **Formulas** — the contractor's stored formulas (colour history)
  with a reorder form per formula: qty in gallons + pickup/delivery
  fulfillment. A reorder looks up the stored formula and re-creates
  the sale line for it, priced through the normal matrix-pricing
  path; the created order (doc number, total, fulfillment) is shown.
  Formula tracking only — the portal never drives a tint machine.
- **Bill Pay** — pay an amount by card or ACH. Payments are
  authorized by the mock provider in dev (no live payment API, no
  card data stored) and no surcharge is applied to bill-pay in
  v0.6. A payment applies FIFO (oldest open charge first); the
  allocations are shown and the open balance is re-fetched after
  the payment. Each attempt carries a fresh client-generated
  idempotency key, so a retried payment posts only once. Past
  portal payments are listed below the form.

## Demo figures (seeded)

The demo contractor (Portal Painting, `C-PORTAL`) has two open
charges, 400.00 + 300.00 = 700.00, and one stored formula (OC-17
White Dove, Eggshell). A 150.00 bill payment applies FIFO to the
oldest charge and drops the open balance to exactly 550.00.

## Notes

- Money is decimal strings inside `{amount, currency}` objects —
  never floats; amounts are displayed exactly as the server sends
  them. `Currency` serializes as its Rust variant name (`Usd` /
  `Cad`), so bill-pay sends `currency: "Usd"` — `"USD"` is rejected
  by the API. Likewise the reorder request sends
  `fulfillment: "Pickup" | "Delivery"` (variant names); the reorder
  *response* returns fulfillment lowercase (`"pickup"`).
- Statements return raw `StatementRow` decimals (e.g.
  `opening_balance: "400.0000"`), not `Money` objects — that is
  what the edge sends; see `src/types.ts`.
- The native Tauri shell is out of scope for this app (plain web
  UI, like the counter's dev path).
