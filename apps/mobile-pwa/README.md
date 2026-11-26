# mobile-pwa — Offline-First Mobile PWA (Phase 5, v0.6)

Warehouse, outside sales and delivery driver in one phone-sized
app, against the real edge API (`df-server edge`). No mocks: every
read and every mutation hits the edge; the only demo data is the
product picker in `src/catalog.ts` (same three products as the
counter), used for names/SKUs/prices where the UI needs a picker.

## Dev path (verified in this VM)

Terminal 1 — the edge daemon, with a temp database:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
rm -f /tmp/mobile-pwa-edge.db
sqlite3 /tmp/mobile-pwa-edge.db < apps/pos-desktop/seed-demo.sql
sqlite3 /tmp/mobile-pwa-edge.db < apps/phase5-seed-demo.sql
cargo run -p df-server -- edge \
  --db sqlite:/tmp/mobile-pwa-edge.db \
  --store-id 00000000-0000-0000-0000-00000000a000 \
  --org-id 00000000-0000-0000-0000-00000000b000 \
  --listen 127.0.0.1:8080
```

(The prebuilt `target/debug/df-server` binary works too — same flags.)

Terminal 2 — the UI (Vite proxies `/api/*` to the edge on :8080):

```sh
cd apps/mobile-pwa
npm install
npm run dev   # http://localhost:1422
```

## Role tabs

- **Warehouse** — PO receive (`POST /purchasing/orders/{id}/receive`,
  `{lines:[{product_id, qty}]}`), physical counts (start / scan /
  reconcile — reconcile posts `count_adj` entries so on-hand equals
  counted), and inter-store transfers (create / ship / receive).
  Transfers need two different store ids; the destination is typed
  in, since the demo seed has one store.
- **Outside Sales** — KPIs from `GET /mobile/kpis?store_id=` (sales
  today, open quotes/orders, open AR, deliveries out today), CRM
  activities (list by customer, log call/visit/email/note/follow-up),
  and quote builder (`POST /pos/quotes` with a `SaleInput`: lines
  carry `unit_price`/`unit_cost` as `{amount, currency}` Money,
  tenders `[]`, optional `po_number`). Demo customer id is the
  Phase 5 portal contractor `00000000-0000-7000-8000-000000000101`.
- **Delivery Driver** — dispatch board (`GET /deliveries/dispatch?status=`,
  filterable; tap a delivery to open it). The board lists open
  deliveries only — the server drops delivered/cancelled rows, so
  they never appear here — and POD submit. POD is a
  canvas signature pad plus a photo file input: the photo's SHA-256
  is computed in-browser (WebCrypto) and sent with the file name as
  `photo_ref`. The server refuses a POD without BOTH signature and
  photo, and only while the delivery is `out_for_delivery` — the UI
  enforces the same rules up front and offers "Mark out for delivery"
  for `assigned` deliveries.

## Offline-first

- `public/manifest.webmanifest` + hand-rolled `public/sw.js` (no new
  npm deps): the app shell (HTML, JS/CSS assets, manifest, icons)
  is cached on install; API calls are network-only and never cached.
- `src/offline.ts` keeps an outbox queue in localStorage. Mutations
  made while offline — or when the fetch fails at the network level —
  are queued instead of lost. The header shows a pending count and a
  **Sync now** button that replays them in order when back online;
  an honest offline banner shows while `navigator.onLine` is false.
- **Idempotency warning:** server-side idempotency exists ONLY for
  portal bill-pay, NOT for POD, counts, PO receives or transfers.
  A replayed mutation that actually reached the server before the
  connection dropped will be applied TWICE. The Sync button asks for
  confirmation and says this plainly. Server rejections (4xx/5xx
  with an error body) are never queued — replaying them cannot help.

## Photo note

The photo file itself is NOT uploaded anywhere: the edge has no
photo-upload endpoint. Only the file reference (`photo_ref`) and its
SHA-256 hash are recorded with the POD, so the photo on the device
can be matched to the delivery later.

## Tauri 2 Mobile wrapper — UNVERIFIED

`src-tauri/tauri.conf.json` exists (identifier `com.decorpos.mobile`,
product name "Decor POS — Mobile", frontend dist `../dist`, dev URL
`http://localhost:1422`, phone-sized default window 390×844), but
the native wrapper has NOT been built or run in this VM — there is
no Android SDK here. Do not claim it builds until someone runs
`cargo tauri android dev` (or `ios dev`) on a machine with the SDK.

## Rescope — tinting is formula tracking only

This app contains no dispense / tint-machine / VirtualTinter /
colorant UI of any kind. Formula lookup lives in the portal, not
here. Nothing in this app drives a tint machine.

## Known limits / still stubbed

- No push notifications, no background sync (the outbox replays only
  when the user taps Sync while the app is open).
- No photo upload (see above); no camera capture beyond the file
  input's `capture="environment"` hint.
- The service worker caches only the app shell; first load must be
  online. Registration is skipped on non-localhost/non-HTTPS origins.
- Counts show product ids, not names, on scanned lines (the count
  rows carry only `product_id`).
- Store id is a free-text field (demo store prefilled) — there is no
  store picker or login; auth is out of scope for this phase, same as
  the other dev-path apps.
