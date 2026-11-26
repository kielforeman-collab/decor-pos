# web-storefront — Retail BOPIS Storefront (Phase 5, v0.6)

Public buy-online-pick-up-in-store storefront against the real edge
API (`df-server edge`), the same SQLite edge database the counter
POS uses. No login, no account — guest checkout. No frontend-only
mocks: every view calls the edge.

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
cd apps/web-storefront
npm install
npm run dev   # http://localhost:1424
```

Seed the demo catalog into the edge DB first so the catalog has
products (same seed as the counter, plus the Phase 5 seed for the
10 gallons on hand):

```sh
sqlite3 edge.db < apps/pos-desktop/seed-demo.sql
sqlite3 edge.db < apps/phase5-seed-demo.sql
```

## Views

- **Shop & checkout** — catalog grid (`GET /catalog/products`,
  active products, cheapest by SKU) with search by SKU/name; a
  cart with quantities per product and a display-only subtotal
  from list prices. Checkout takes a pickup name, a contact
  (email or phone) and a pickup slot (must be in the future —
  the server enforces it) and posts `POST /bopis/orders`. The
  confirmation shows the order id, doc number, server-priced
  grand total, pickup slot and payment reference.
- **Order status** — look up an order by id
  (`GET /bopis/orders/{id}`): doc number, status, lines with
  quantities, unit prices, line totals and fulfilled quantities,
  plus subtotal / tax / eco fees / grand total. The
  **Complete pickup (counter)** button posts
  `POST /bopis/orders/{id}/complete-pickup`; on success the order
  shows completed with fulfilled quantities. If on-hand stock is
  short the server refuses and the order stays open — the error
  is shown as-is.

## Notes

- Money is decimal strings inside `{amount, currency}` objects
  (and decimal strings on order headers/lines) — never floats.
  Cart math uses exact BigInt decimal arithmetic
  (`src/types.ts`); the cart subtotal is display only, the
  server prices the order (tax + eco fees).
- Checkout is authorized by the mock payment provider in dev —
  no live payment API is called, no card is charged, and no card
  data is collected or stored. There are no card fields anywhere
  in this app.
- A BOPIS line is a catalog product from stock. Tinting is
  formula tracking only — there is no dispense / tint-machine /
  colorant UI here, and none is wired to the BOPIS flow.
- The native Tauri shell is out of scope for this app (plain web
  UI, like the counter's dev path).
