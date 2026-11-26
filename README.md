# decor-pos — Open-Source Rust Paint & Decorating POS

Open-source, self-hostable POS & retail management for independent paint
& decorating retailers. Full functional parity with Epicor Decor Fusion
is the goal; see the canonical spec at
`../your_files/decor-fusion-pos-plan/architectural-specification.md`
(spec §1–§9).

> **Phase 2 (v0.3) is what this repo contains right now.** Paint
> tinting is real end to end: the virtual tinter dispenses through
> `df-edge`, `df-reports` and `df-jobs` are real, the HQ admin API
> serves formulas/levels/reports, and `df-server reports` /
> `df-server jobs run` retrieve them from the CLI. See "What's real
> vs stub" below.

## Build & test

Rust is at `~/.cargo/bin` — put it on `PATH` for every cargo call:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt --check
cargo clippy --workspace --all-targets
cargo test --workspace          # or: cargo nextest run --workspace
```

Builds are offline-safe for sqlx: the workspace uses runtime-checked
queries only (no `query!` macros), so no live database is needed
(`SQLX_OFFLINE=true` is set in `.cargo/config.toml`).

Run the edge daemon + counter POS (dev path):

```sh
cargo run -p df-server -- edge --db sqlite:edge.db \
  --store-id <uuid> --listen 127.0.0.1:8080
cd apps/pos-desktop && npm install && npm run dev   # http://localhost:1420
```

Or run the API skeleton (admin endpoints need `--db` + `--org-id`;
without them they return 503):

```sh
cargo run -p df-api -- --db sqlite:edge.db --org-id <uuid> --store-id <uuid>
cargo run -p df-server -- --help
```

Tinting + reports (edge mode serves these on the same port):

```sh
# Dispense a customer's past color against a sale line:
curl -X POST http://127.0.0.1:8080/pos/tinting/dispense \
  -H 'content-type: application/json' \
  -d '{"formula_id":null,"customer_id":"<uuid>","job_id":null,"color_query":"White Dove","transaction_id":"<uuid>","line_no":1,"dispenser":null}'
curl 'http://127.0.0.1:8080/pos/tinting/formulas?customer_id=<uuid>'
curl http://127.0.0.1:8080/pos/tinting/colorants
curl 'http://127.0.0.1:8080/pos/reports/daily-usage?date=2026-11-25'
curl 'http://127.0.0.1:8080/pos/reports/low-stock?date=2026-11-25'

# Same reports/jobs from the CLI (no server; the DB must exist):
cargo run -p df-server -- reports --db sqlite:edge.db --org-id <uuid> \
  --store-id <uuid> --kind daily-usage --currency USD
cargo run -p df-server -- reports --db sqlite:edge.db --org-id <uuid> \
  --store-id <uuid> --kind shortage
cargo run -p df-server -- jobs run --db sqlite:edge.db --org-id <uuid> \
  --store-id <uuid> --currency USD
cargo run -p df-server -- jobs run --db sqlite:edge.db --org-id <uuid> \
  --store-id <uuid> --job shortage-alerts --currency USD
```

`--currency` may be omitted when the database has a store row
(`df-server edge` seeds no store row, so pass it for edge
databases). The low-stock product half needs operator-configured
thresholds — set them with
`POST /pos/reports/low-stock/thresholds` on the edge, or wait for
the Phase 3 min/max table; the colorant half always works.

## What's real vs stub (Phase 4)

Real, tested:

- `df-domain` — `Money` (rust_decimal, scale 4, never floats),
  `Currency` (USD/CAD), UOM conversions + flooring waste/box rounding,
  matrix pricing (wildcards, priority, margin floor), US/CA tax and
  PaintCare/eco-fee engine (sample presets — verify rates before
  production), AR aging + finance charges, tint colorant costing,
  UUIDv7 ids, feature flags. Unit + proptest property tests.
- `df-db` — sqlx repositories + migrations for PostgreSQL
  (`migrations/postgres/`) and SQLite (`migrations/sqlite/`) covering
  spec §5 tables (organizations, stores, users, products, color
  catalog, colorants, tint formulas, customers, pricing rules, AR
  ledger, transactions, inventory ledger, tills, payments, sync
  outbox) plus the Phase 2 tint tables (`tint_jobs`,
  `colorant_levels`), the EDI tables (`purchase_orders`,
  `purchase_order_lines`, `edi_documents`), and the Phase 3 tables
  (`statements`, `inventory_transfers` + `transfer_lines`,
  `physical_counts` + `count_lines`, `reorder_points`, plus credit
  fields `terms_days` / `finance_rate_pct_monthly` on customers —
  `credit_limit` and `credit_hold` already existed since 0001),
  and the Phase 4 tables (`crm_activities`, `appointments`,
  `crm_saved_views`, `deliveries` + `delivery_events` +
  `delivery_pods` + `packing_slips`, `po_acknowledgments`,
  `invoice_match_reports`, `price_catalog_applications`,
  `transport_records`).
  SQLite round-trip tests pass, including file-backed databases
  (`sqlite:edge.db` creates the file on open); the PostgreSQL path
  is migration-embedded and compile-checked — a live-Postgres
  round-trip test is still open (needs a running PG 16+).
- `df-auth` — Argon2id password hashing/verification, counter PIN and
  badge-token hashing, RBAC roles/permissions. WebAuthn/Passkeys and
  OIDC are trait + stub only (return `NotImplemented`).
- `df-api` — Axum skeleton, OpenAPI 3.1 via utoipa
  (`/api-docs/openapi.json`), working `/health`. Auth routes exist
  and return 501. Phase 2 admin endpoints are real when the binary
  is started with `--db` (+ `--org-id`, `--store-id`): color search,
  tint formulas (list/search by color and by customer, create),
  colorant levels (list, set level/threshold), reports (daily usage,
  low stock, shortage alerts) — all backed by the `df-db` repos over
  `AnyPool`. Without `--db` they return 503. Phase 4 adds HQ reads
  over the same store: `GET /api/v1/crm/activities`,
  `/api/v1/crm/appointments`, `/api/v1/deliveries/dispatch`,
  `/api/v1/edi/documents` — plain JSON reads, not in the OpenAPI
  `paths` (the row types carry no utoipa schema, following the
  existing inventory/AR read pattern).
- `df-sync` — UUIDv7 outbox (`Outbox`, `DeltaBatch`), `HqSink`
  trait (boxed-future form), idempotent `MemoryHq` (replays are
  no-ops, counted as duplicates), conflict resolver
  (append-only ledgers merge by union, completed transactions
  immutable). Tested, including replay no-op.
- `df-edge` — offline checkout (matrix pricing + tax + eco fees,
  card surcharge from configurable `EdgeConfig.surcharge_rules`,
  default 3% on card), tills + Z-reports, holds (in-memory),
  quotes, layaways (deposit on an open layaway), returns (inventory
  + AR reversal), voids, receipt printing + cash-drawer kick, and
  the local HTTP/WS API (`/pos/*`, `/ws/v1/store/{id}`). Uses the
  real `df-payments` and `df-hw` crates (its placeholder copies
  were deleted in the integration). Sync drains the outbox to an
  `HqSink`; `df-server edge` wires an in-process `MemoryHq`, so
  sync works out of the box in the dev path. Phase 2: hosts an
  in-process `VirtualTinter` backed by the `colorant_levels` +
  `tint_jobs` tables — `POST /pos/tinting/dispense` resolves a
  formula by id or by a customer's past color, dispenses, and
  persists the job + decrements the levels in one SQLite
  transaction (a failed dispense decrements nothing and still
  records a failed job); `GET /pos/tinting/formulas`,
  `/pos/tinting/colors`, `/pos/tinting/colorants`,
  `/pos/reports/daily-usage`, `/pos/reports/low-stock`, and
  `POST /pos/reports/low-stock/thresholds` (operator-configured
  product thresholds — superseded in Phase 3 by the `reorder_points`
  table). Phase 3: checkout credit-checks charge tenders (requires
  a customer, rejects on-hold or over-limit accounts with HTTP 409,
  posts the `ar_ledger` charge entry in the same transaction as the
  sale); AR payment / finance-charge / collections / statement-batch
  methods; and the inventory back-office module
  (`/inventory/balances`, `/inventory/reorder-points`,
  `/purchasing/replenishment/suggest`, `/purchasing/orders` +
  `/{id}/receive`, `/inventory/transfers` + `/{id}/ship|receive`,
  `/inventory/counts` + `/{id}/scan|reconcile`,
  `/inventory/shelf-tags`, `/pricing/rules`, `/ar/*` reads) — all
  ledger-posting mutations in a single SQLite transaction.
  Phase 4: CRM (`/crm/activities` CRUD, `/crm/reminders/due|overdue`,
  complete/snooze, `/crm/appointments` CRUD + by-day/by-contractor
  lists, `/crm/saved-views` CRUD + run, `/crm/lapsed` report),
  delivery (`/deliveries` create, `/deliveries/dispatch` board,
  assign, status transitions, `/deliveries/{id}/packing-slip`,
  `/deliveries/{id}/pod` — signature + photo as JSON base64 +
  SHA-256 hash, no multipart), and EDI (`/edi/send`,
  `/edi/intake`, `/edi/match`, `/edi/acknowledgments`,
  `/edi/price-applications`, `/edi/transport`, `/edi/catalog-import`).
  A POD without both a signature and a photo is refused up front
  and writes nothing; a signature-only POD never reaches the
  close-out that requires both.
- `df-crm` — Phase 4, pure domain (no I/O, Decimal only):
  activity log with `ActivityType`, tickler rules (due = max of
  due date and snooze; completing or snoozing a completed
  reminder is an error), appointments (valid window, overlap =
  same contractor OR same staff, scheduled only), saved views
  (two dialects: `CustomerFilter` for customer segments and
  `ActivityFilter` for the activity log — the edge keeps them on
  separate endpoints), and the lapsed-contractor report
  (never-purchased first, then oldest purchase first, strictly
  more than N days since the last sale).
- `df-delivery` — Phase 4, pure domain (no I/O, Decimal only):
  dispatch state machine (`draft → assigned → out_for_delivery →
  delivered`, `cancelled` from anywhere; a direct jump to
  `delivered` is refused — only a complete POD closes out),
  POD (signature captured as bytes + SHA-256, photo as reference
  + SHA-256; complete only with both), and packing slips
  (42-column text, ESC/POS, ZPL from the same data).
- `df-hw` — ESC/POS receipts (`ReceiptBuilder`, 42-column, partial
  cut), ZPL can/shelf labels, HID scanner parsing (`qty*barcode`
  prefix, case multipliers), `MockPrinter` / `MockDrawer`,
  `drawer_kick_bytes`. Pure + unit-tested; no hardware attached.
- `df-payments` — `PaymentProvider` trait (boxed-future form, so
  `&dyn PaymentProvider` works), `MockTerminal` (approve-all /
  declining / scripted, idempotent on idempotency key),
  `StandaloneTerminal` (dumb external terminal, reconciliation log,
  optional manual confirmation), `StripeTerminal` skeleton
  (credentials from env only; live calls not implemented — returns
  `NotSupported`, no network), per-tender surcharge rules
  (`compute_surcharge`: pct + flat, capped, never negative).
- `df-tinter` — `TinterPlugin` trait (sync, deterministic),
  `VirtualTinter` simulator (exact Decimal shot math, atomic
  failure — nothing decremented on a short or unknown colorant),
  tint-job lifecycle (`Queued → Dispensing → Done/Failed`), color
  search + per-customer formula history (in-memory), ZPL can
  labels, demo seed catalog/colorants for tests. No hardware
  bridge in this crate (file-drop/TCP/serial bridges are not
  built); the `df-plugin-sdk` HTTP adapter is not unified with
  this trait yet (see below).
- `df-edi` — file-based vendor EDI: low-stock PO generation (one
  PO per vendor, exact Decimal math), X12 850 build/parse
  round-trip, 856/810 intake parsing, CSV intake + vendor catalog
  import, PO/EDI-document repos. Phase 4 completes the round-trip:
  855 acknowledgment parsing + application (a report — the PO is
  never mutated by an ack), 832 price-catalog parsing + apply
  report (review-only; `mark-applied` records the review decision
  and writes no prices), three-way match (PO vs shipment vs
  invoice, per-line status + variance), and transports: a local
  SFTP stand-in (stages files under `outbox/`/`inbox/` — no SSH,
  no remote server, no delivery confirmation) and AS2 MIME
  packaging + MDN (sign/encrypt return `NotSupported`, no
  network). No live vendor endpoint anywhere — a file in
  `outbox/` has only been staged locally.
- `df-plugin-sdk` — plugin manifest (`plugin.toml`) + `PluginHost`
  over HTTP (real: spawns the child, polls health, hand-rolled
  loopback HTTP/1.1 client, tested against a real axum server) and
  `HttpTinterPlugin` (the spec §6.2 tinter interface over
  HTTP/JSON). gRPC and WASM transports are honest stubs
  (`NotSupported`). **Not wired to `df-tinter`**: the SDK adapter
  speaks HTTP/JSON and the trait is sync in-process — unifying them
  (an adapter that presents an `HttpTinterPlugin` as a
  `TinterPlugin`, or vice versa) is follow-up wiring, not done in
  Phase 2. Phase 4 implements the EDI side of the SDK
  (`EdiVendorPlugin` trait, `FileEdiVendorPlugin` over the local
  stand-in, `HttpEdiVendorPlugin` over HTTP); `PaymentPlugin` is
  not implemented in the SDK. `AccountingPlugin` lives in the Phase 3
  `df-accounting` crate instead (file-based CSV/IIF adapters, no
  live API).
- `df-reports` — real (JSON + plain text, all Decimal, no floats):
  daily usage report (tints dispensed + per-color breakdown,
  colorant consumed in fl oz + cost, sales totals from completed
  transactions), low-stock report (products below caller-supplied
  minimums from ledger balances + colorants at/below threshold,
  with draft PO suggestions via `df-edi`), colorant shortage
  alerts (standalone or as a report section). Phase 3: monthly
  statement PDFs via real Typst 0.15.1 (prebuilt binary at
  `~/workspace/tools/typst/`; `DF_TYPST_BIN` or `--typst-bin`
  overrides; template has header, summary, line items, aging,
  remittance stub) — `render_statement_pdf` returns valid PDF
  bytes and the sha256 is stored on the statement row. No Excel
  output — that is Phase 6.
- `df-jobs` — real scheduled job runner: named jobs with daily
  (06:00 UTC) or manual schedules, `run_now` / `run_all` /
  `run_due(now)` entry points (testable without wall-clock
  waiting), a thin periodic tokio loop wrapper, and every run
  recorded (name, `ran_at`, success/failure, summary, JSON
  output) in the runner's in-memory history. The three Phase 2
  daily jobs (`daily-usage`, `low-stock`, `shortage-alerts`)
  produce their reports via `df-reports`. Phase 3 adds
  `monthly-statements` (1st, 06:00 UTC: builds last month's
  statements, renders each PDF, records sha256) and
  `finance-charges` (1st, 06:05 UTC: idempotent per-period
  assessment, posts to `ar_ledger`). Run history is **not
  persisted** — there is no `job_runs` table yet (deferred again in
  Phase 3); it lives as long as the process does. There is no
  daemon worker yet — `df-server jobs run` is the run-once path;
  schedule it with cron/systemd for daily runs.
- `df-server` — `edge` mode real (local SQLite + POS HTTP/WS API
  incl. the Phase 2 tinting/report endpoints, mock terminal +
  in-process `MemoryHq`); `reports` mode real (prints a report as
  JSON: `--kind daily-usage | low-stock | shortage`); `jobs run`
  real (runs the daily jobs once and prints the recorded runs as
  JSON, exit 1 if any run failed); Phase 3: `statements run
  --period YYYY-MM` (persists statement rows, writes Typst PDFs,
  prints per-statement summary + sha256) and `accounting export
  --kind csv|iif --ledger ar|inventory --out <path>`;
  `standalone`, `hq`, `worker`, `migrate` still exit 2. Phase 4:
  `edi send|intake|match|catalog` — send a PO through the local
  SFTP stand-in, intake an X12 file, run a three-way match, or
  build an 832 apply report, all printed as JSON (SQLite only;
  the database file must already exist).
- `df-ar` — Phase 3, pure domain (no I/O, Decimal only): charge
  accounts (`COD`/`NET<n>`/`EOM` terms, due-date math), credit
  check (blocks at or over the limit; zero limit = no credit),
  open-charge derivation with FIFO payment allocation (overpayment
  is unallocated credit), finance charges on 31+ day overdue
  balances (idempotent per statement period), collections queue
  (oldest bucket first, then amount), statement batch data with an
  exact identity check (`opening + charges − payments + finance =
  closing`). Charge sales for a customer with no row on the edge
  (reference data not yet synced) are not blocked — HQ enforces
  credit on sync.
- `df-accounting` — Phase 3: CSV export of the AR + inventory
  ledgers (RFC 4180, deterministic order, round-trip parsers) and
  QuickBooks IIF export (`!TRNS/!SPLNS/!ENDTRNS`, every transaction
  balanced, with a validating parser), plus the
  `AccountingPlugin` trait and file-based CSV/IIF adapters. No
  live QuickBooks/Xero API — live sync is a Phase 4+ / vendor-pack
  concern.
- `web-backoffice` — Phase 3 back-office web app
  (`apps/web-backoffice/`, React 19 + TS + Vite on port 1421,
  `/api` → `df-server edge` on 127.0.0.1:8080): nine tabs, all
  calling the real edge API — Inventory browser (on-hand /
  committed / on-order / in-transit + min/max editor),
  Purchasing (replenishment suggestions from min/max + 30-day
  velocity, PO create/receive), Transfers (draft → ship →
  receive), Counts (scan/reconcile with variance table), Shelf
  Tags (ZPL), Pricing (bulk matrix editor with min-margin
  guardrail), AR (read-only summary/aging/collections). Phase 4
  adds a CRM tab (activities, tickler, appointments, saved views,
  lapsed report), a Delivery tab (dispatch board + assign + POD
  status + packing slip), and an EDI view under Purchasing (PO
  send, intake, three-way match, acks, price-application review,
  transport records, catalog CSV import). Build verified (`tsc`
  + Vite clean). No Tauri shell — web app only.
- `pos-desktop` — Tauri 2 counter POS (`apps/pos-desktop/`):
  React 19 + TypeScript + Vite frontend, complete and build-verified
  (`tsc` + Vite clean): keyboard-first checkout, scan/entry,
  matrix-priced totals, quotes, returns, holds, layaways, split
  tender, tills + Z-report, mock/standalone provider selection,
  receipt preview. The Rust backend (`PosApp`: in-process `Edge` +
  capturing `MockPrinter` + provider switching + real `df-hw`
  scanner command) compiles as a plain crate and is unit-tested.
  **The native Tauri shell build is unverified in this VM**:
  `libwebkit2gtk-4.1-dev` cannot be installed here (held broken
  `libzstd` packages; see `apps/pos-desktop/README.md`), so the
  served-frontend dev path (Vite → `df-server edge`) is the
  verified way to run it; build the shell with
  `cargo run -p pos-desktop --features tauri-shell` on a machine
  with the Tauri 2 system libraries.

Exit tests:

- Phase 1: `crates/df-edge/tests/offline_checkout_parity.rs`
  (`offline_checkout_print_reconnect_parity`,
  `offline_return_reverses_ledger`) — offline cash sale → print →
  reconnect → HQ inventory/transaction/AR parity, replay is a no-op,
  return reverses the ledgers on the next sync.
- Phase 2: `crates/df-edge/tests/tint_dispense_end_to_end.rs`
  (`tint_dispense_end_to_end`, `failed_dispense_decrements_nothing`)
  — formula lookup by past color → virtual tinter dispenses it →
  levels decrement by exactly the formula amounts → tint job
  recorded against the sale line → daily usage report shows the
  consumption → a colorant at its threshold raises a shortage
  alert; and a short dispense 409s with nothing decremented and a
  failed job recorded.
- Phase 3: `crates/df-edge/tests/ar_statement_batch.rs` (3 tests)
  — month-end statement batch: opening + charges − payments +
  finance charges = closing exactly, aging buckets match the
  ledger, re-running the batch changes nothing (idempotent finance
  charges), over-limit and on-hold charges are rejected with zero
  `ar_ledger` rows, AR payment allocates FIFO to the oldest
  charge. `crates/df-edge/tests/inventory_backoffice.rs` (9 tests)
  — multi-store transfer (5 units A→B, ship + receive, A −5 /
  B +5, in-transit leg in both ledgers), count reconciliation
  (10→8 posts `count_adj` −2, on-hand 8; 12→+2), PO receiving
  (partial 4/10, over-receive rejected, rest → received).
  `crates/df-reports` statement tests — realistic statement
  renders a real Typst PDF (`%PDF`, >5 KB) and `pdftotext`
  extraction contains all key figures.
- Phase 4: `crates/df-edge/tests/phase4_edi_round_trip.rs`
  (`edi_round_trip_850_855_856_810_match_832`) — PO → 850 sent
  through the local SFTP stand-in → 855/856/810 intake → three-way
  match `matched` at 477.00 → 832 apply report (reprice, add,
  skip) → mark-applied records the review only, product prices
  unchanged; re-sending a sent PO fails, an 855 for an unknown PO
  errors. `phase4_pod_closeout.rs`
  (`pod_closeout_gates_on_signature_and_photo`) — cash sale →
  delivery → assign (Van 2) → direct jump to `delivered` refused
  → POD without a photo refused up front with nothing written →
  POD with signature + photo closes out, audit event present,
  packing slip renders with the sale's line/doc number, and a
  linked open order auto-closes to `completed`.
  `phase4_crm_lapsed.rs`
  (`crm_lapsed_tickler_appointments_saved_views`) — lapsed report
  (never-purchased first, then lapsed; retail excluded; 200-day
  window clears the lapsed row), tickler (due/overdue, complete,
  snooze rules and their errors), appointments (same-contractor
  and same-staff overlap blocked, adjacent and parallel slots
  free, cancel frees the slot), saved views (activity dialect
  matches open follow-ups only; customer dialect matches buyers
  only; the customer endpoint rejects an activity-dialect view),
  activity CRUD round-trip.

Stubs (crate exists, `phase0_stub()`, no implementation):

- `df-server` — `standalone`, `hq`, `worker`, `migrate` modes
  report "not implemented".

Known gaps (stated plainly, not hidden):

- Live-Postgres round-trip is untested for every crate (no PG 16+
  in this VM); the PostgreSQL path is migration-embedded and
  compile-checked only.
- `df-ar`'s open-charge model only reduces charges via
  `ar_allocations` — a payment with no allocation rows counts in
  `payments_total`/closing but not in aging, so aging can exceed
  closing until payments are allocated.
- Typst embeds `CreationDate`, so re-rendering the same statement
  yields a different sha256.
- `HttpTinterPlugin` (SDK, HTTP/JSON) is not unified with the
  `df-tinter` trait (sync, in-process); the edge hosts only the
  virtual tinter. No file-drop/TCP/serial hardware bridge exists.
- A stored formula with nonzero `sub_shots` cannot be dispensed
  in v0.3 (the domain model is ounces + shots only); it fails with
  a clear error instead of guessing.
- Job-run history is in-memory only; no daemon worker; the
  `df-api` binary runs no migrations (the database must already
  exist).
- Phase 4, stated plainly: the SFTP transport is a local
  directory stand-in — no SSH, no remote server, no delivery
  confirmation; AS2 has no network and no crypto (sign/encrypt
  return `NotSupported`). POD is signature bytes + a photo
  reference + SHA-256 hashes over JSON — there is no camera
  hardware in this VM and no photo is stored. An 855 ack
  application is a report only; it never changes the PO. An 832
  `mark-applied` records the review decision only; no product
  price is rewritten. The lapsed-contractor report is an edge
  endpoint only (no `df-api` read) because it needs per-contractor
  summary assembly. The activity-filter endpoint parses leniently
  (`#[serde(default)]`, unknown keys ignored), so running it on a
  customer-dialect view matches every activity — use the customer
  endpoint for customer views; it rejects activity-dialect JSON.

## Deferred

Benjamin Moore COLORx tint machine integration is **deferred to
post-v1.0** (spec §9). There is no COLORx code, plugin, or protocol
work anywhere in this repo. The generic `df-tinter` bridge (Phase 2)
and the `TinterPlugin` trait are the integration point for later.

## Phase 2 spec items deliberately left out

The spec §7 Phase 2 row also names the feature-flagged decorating
verticals — `vertical_flooring`, `vertical_window_treatments`,
`vertical_wallpaper`, `vertical_sample_loans` — and the generic
watch-folder/TCP/serial tinter bridge. None of those are built in
v0.3: this phase ships the paint & tinting core (catalog, formulas,
colorant COGS costing, customer/job formula history, virtual
simulator + ZPL can labels, reports, daily jobs) and the file-based
EDI that was pulled forward. The verticals and the hardware bridge
are the remaining Phase 2 work; the feature flags for them exist in
`df-domain` but no vertical code is behind them yet.

## What's next (Phase 5, v0.6)

Unified offline-first `mobile-pwa` + Tauri 2 Mobile wrapper
(Warehouse PO receive/counts/transfers, Outside Sales KPIs/CRM/
quotes, Delivery Driver POD). `web-portal` 24/7 Contractor
Portal (invoices, statements, formula reorders, US/CA online bill
pay) + Retail BOPIS storefront. Plus the still-open items: a
`job_runs` table + daemon worker so scheduled jobs persist and
run without cron, and the `HttpTinterPlugin` ↔ `TinterPlugin`
unification. Benjamin Moore COLORx stays deferred (spec §9).

## License

Dual-licensed Apache-2.0 OR MIT — see `LICENSE-APACHE` and
`LICENSE-MIT`.
