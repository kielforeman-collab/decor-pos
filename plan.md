# Decor POS — Plan Progress

Source of truth: `~/workspace/your_files/decor-fusion-pos-plan/architectural-specification.md`
Repo: https://github.com/kielforeman-collab/decor-pos (public, `main`)
Local: `~/workspace/decor-pos/` — Rust 1.99 (`~/.cargo/bin`), Rust 2024, Apache-2.0 OR MIT.

Benjamin Moore COLORx is **deferred post-v1.0** (spec §9) — no COLORx code in any phase.
Nov 26 rescope (Kiel): tinting is **formula tracking only** — the POS keeps track of
formulas, it never drives a tint machine (no VirtualTinter, no dispense, no colorant inventory).
Standing caveats: PostgreSQL round-trip never tested live (no PG server in this VM);
tax/eco-fee presets are sample values; native Tauri shell build unverified in VM
(`gdk-3.0` missing, apt broken on `libzstd-dev`) — verified dev path is
`df-server edge :8080` + `npm run dev :1420`.

Status key: ✅ done · 🔄 in progress · ⬜ not started. Each phase's builder updates
only its own section, ticking items as they are verified — not when code merely exists.

## Phase 0 — Core Engine & Blueprint (v0.1) — ✅ DONE Nov 25, 2026
### Scope
- ✅ `df-domain` — Money/rust_decimal, UOM, matrix pricing, US/CA tax + PaintCare/eco fees, AR aging, tint costing
- ✅ `df-db` — sqlx AnyPool, PG + SQLite migrations
- ✅ `df-auth` — real Argon2id + PIN/badge, RBAC (WebAuthn/OIDC stubbed NotImplemented)
- ✅ `df-api` — Axum, OpenAPI 3.1, `/health` live, auth routes 501
### Verification
- ✅ `cargo test --workspace` — 54/54 passing
- ✅ `cargo clippy --workspace --all-targets -- -D warnings` — clean
- ✅ `cargo fmt --all -- --check` — clean
### GitHub
- ✅ 60 files on `main` via browser upload (connector push 403'd at the time), latest `3551557`

## Phase 1 — Offline Counter POS & Hardware (v0.2) — ✅ DONE Nov 25, 2026
### Scope
- ✅ `df-edge` — SQLite WAL daemon, HTTP+WS API, tills/Z-report, holds/quotes/returns/layaways
- ✅ `df-sync` — UUIDv7 outbox, replay-safe ledger merge, append-only union, completed txns immutable
- ✅ `df-hw` — real ESC/POS + ZPL bytes, HID scanner parse, drawer kick, mock printer/drawer
- ✅ `df-payments` — provider trait, mock + standalone terminals, 3% card surcharge (Stripe Terminal skeleton errors without credentials, never calls API)
- ✅ `pos-desktop` — Tauri 2 React checkout, keyboard-first (native shell unverified in VM; dev path `df-server edge :8080` + `npm run dev :1420` verified live end-to-end)
### Exit test
- ✅ Offline cash sale → receipt + drawer → sync → HQ inventory/AR match edge exactly; replay is a no-op; return reverses on next sync
### Verification
- ✅ `cargo test --workspace` — 130/130 passing
- ✅ clippy clean, fmt clean
### GitHub
- ✅ Pushed via connector in ONE commit `267edca` (111 files). Connector writes confirmed working.
### Known limits
- Returns don't refund via provider; holds in-memory; card authorized before a failed commit isn't auto-voided; POS cashier sign-in not wired (fixed demo id).

## Phase 2 — Formula Tracking & Vendor EDI (v0.3) — ✅ DONE Nov 25, 2026; rescoped Nov 26, 2026
### Scope — kept (formula tracking only; Kiel, Nov 26: the POS tracks formulas, it never drives a tint machine)
- ✅ Formula tracking — formula lookup by name/code and by customer colour history
- ✅ `df-edi` — X12 850 PO generation, 856 ASN + 810 invoice intake, vendor catalog CSV import
- ✅ `df-plugin-sdk` — real HTTP transport (spawns/manages plugin child process; gRPC/WASM honest NotSupported stubs)
- ✅ `df-reports` — daily usage, low-stock alerts (JSON + text)
- ✅ `df-jobs` — daily 06:00 UTC registry, run_now/run_due, in-memory history
- ✅ Edge formula endpoints + HQ admin endpoints
### Dropped Nov 26 (machine tinting — not needed)
- ❌ `VirtualTinter` + deterministic dispense simulation
- ❌ `TinterPlugin` machine interface + `HttpTinterPlugin`
- ❌ Colorant inventory, shortage/threshold alerts, tint job lifecycle
- ❌ Edge dispense endpoints
### Exit test
- ✅ Formula tracking — past-colour lookup returns the stored formula (exercised by the Phase 2 exit test's lookup step)
- ❌ Old exit test's dispense leg (virtual dispense → levels decrement → threshold alert) — dropped with the machine scope
### Verification
- ✅ `cargo test --workspace` — 209/209 passing
- ✅ clippy clean, fmt clean
### GitHub
- ✅ Pushed via connector in ONE commit `817d89f` (148 files, 37 new)
### Note
- The machine-tinting code (`df-tinter` dispense, `df-plugin-sdk/src/tinter.rs`, edge dispense endpoints) is still in the repo on `main` — it was built and pushed in Phase 2 before the rescope. Removing it from the codebase is a separate step, not done yet.
- The `HttpTinterPlugin ↔ TinterPlugin unification` open item is closed by this rescope (moot — the whole machine interface is dropped).
### Known limits
- PG round-trip still untested; no product min-level table (low-stock product thresholds caller-supplied); job history in-memory only; decorating verticals (flooring/window/wallpaper/sample loans) deliberately left out — flags only.

## Phase 3 — Contractor AR, Inventory & Back-Office (v0.4) — ✅ DONE Nov 26, 2026
### Scope
- ✅ `df-ar` — charge accounts COD/NET/EOM, credit check blocks at/over limit, FIFO payment allocation, finance charges on 31+ day overdue balances (idempotent per period), collections queue oldest-bucket-first, statement batch data with exact identity check
- ✅ Migration `0005_phase3` — statements, inventory_transfers, physical_counts, reorder_points (credit_limit/credit_hold already existed since 0001)
- ✅ `df-db` AR repos + edge checkout credit-enforcement (409 on hold/over-limit, charge entry in the same tx as the sale) + AR payment/finance-charge/collections/statement methods
- ✅ `df-reports` Typst 0.15.1 statement PDFs (real PDFs; prebuilt binary at `~/workspace/tools/typst/`)
- ✅ `df-accounting` — CSV + QuickBooks IIF exports with round-trip/validating parsers, AccountingPlugin trait + file-based adapters (no live QB/Xero API)
- ✅ `df-jobs` monthly-statements + finance-charges jobs; `df-server` CLI `statements run --period YYYY-MM` + `accounting export`
- ✅ `web-backoffice` — React 19 + TS + Vite on :1421, 7 tabs, all real edge API: Inventory, Purchasing (Min/Max + velocity suggestions, PO create/receive), Transfers (draft→ship→receive), Counts (scan/reconcile), Shelf Tags (ZPL), Pricing (bulk matrix editor, min-margin guardrail), AR reads
- ✅ Edge inventory endpoints + HQ read surface in `df-api`
### Exit tests
- ✅ (a) Statement batch reconciles to the cent (400 + 300 − 150 + 3.75 finance = 553.75 exactly); re-run is a no-op; over-limit/on-hold charges rejected with zero AR rows
- ✅ (b) Multi-store transfer A −5 / B +5 with in-transit leg in both ledgers
- ✅ (c) Count reconciliation posts exact count_adj (−2 / +2) and on-hand equals count; statement PDF verified real via pdftotext extraction of key figures
### Verification
- ✅ `cargo test --workspace` — 267/267 passing (cargo test + nextest)
- ✅ clippy clean, fmt clean, back-office `npm run build` clean
### GitHub
- ✅ Pushed Nov 26 via connector in ONE commit `65e96f0` (224 files, Phases 3 + 4 together, per Kiel's pick)
### Known limits
- PG round-trip still untested; job history still in-memory (job_runs table deferred again); unallocated payments count in closing but not aging; Typst CreationDate changes sha256 on re-render; charge for a customer with no edge row is not blocked (HQ enforces on sync); shelf-tag barcode is the SKU (no barcode repo wired).
- Fixes folded in during verification: df-edi PO `created_at` now derives from the passed PO date (was wall-clock — date-rot test failure), df-edge `let...else` → `?`, test-helper arg bundling.

## Phase 4 — Vendor EDI, CRM & Delivery Logistics (v0.5) — ✅ DONE Nov 26, 2026
### Scope
- ✅ `df-edi` full X12 — 850 PO / 855 PO ack (parse + build, application is a report only — the PO is never mutated) / 856 ASN / 810 invoice / 832 price catalog (parse + build, apply report is review-only; `mark-applied` records the decision, writes no prices)
- ✅ 3-way match (PO ↔ ASN ↔ invoice) with per-line status + variance flags; persisted as match reports
- ✅ SFTP/AS2 file-based transports only, no live vendors — `LocalSftpStandIn` stages files under `outbox/`/`inbox/`/`processed/` (no SSH, no remote server, no delivery confirmation); AS2 MIME packaging + MDN real, sign/encrypt return `NotSupported` (no network, no crypto)
- ✅ `df-plugin-sdk` EDI side — `EdiVendorPlugin` trait, `FileEdiVendorPlugin`, `HttpEdiVendorPlugin`
- ✅ `df-crm` (new crate, pure domain) — activity log + `ActivityType`, reminder tickler (due = max(due, snoozed); complete/snooze errors on completed), appointments (valid window, overlap = same contractor OR same staff, scheduled only), saved views (two distinct dialects — `CustomerFilter` and `ActivityFilter`), lapsed-contractor report (never-purchased first, then oldest first, strictly > N days)
- ✅ `df-delivery` (new crate, pure domain) — dispatch state machine (`draft → assigned → out_for_delivery → delivered`, direct jump to delivered refused), POD (signature bytes + SHA-256, photo ref + SHA-256; complete only with both), packing slips (42-col text, ESC/POS, ZPL)
- ✅ Migrations `0006_phase4`, `0007_phase4_crm_delivery`, `0009_phase4_edi` (both dialects; 0009 reserved — do not reuse) + `df-db` repos (crm_activities, appointments, crm_saved_views, deliveries, delivery_events, delivery_pods, packing_slips, po_acknowledgments, invoice_match_reports, price_catalog_applications, transport_records)
- ✅ `df-edge` wiring (delegates to the domain crates, no second implementation) — CRM activities CRUD, reminders due/overdue + complete/snooze, appointments CRUD + by-day/by-contractor, saved views CRUD + run (activity dialect) and run-customers (customer dialect via `evaluate_saved_view`, which rejects activity-dialect JSON), lapsed report, deliveries create/dispatch/assign/status/packing-slip/POD (JSON base64 signature + photo ref + SHA-256; a POD without both is refused up front and writes nothing), EDI send/intake/match/acks/price-applications/transport/catalog-import
- ✅ `df-api` HQ reads — `GET /api/v1/crm/activities`, `/crm/appointments`, `/deliveries/dispatch`, `/edi/documents` (plain JSON reads, not in OpenAPI `paths`)
- ✅ `df-server` CLI `edi send|intake|match|catalog` (JSON out; SQLite file must already exist)
- ✅ `web-backoffice` — now 9 tabs, all real edge API: new CRM tab (activities, tickler, appointments, saved views, lapsed report) and Delivery tab (dispatch board + assign + POD status + packing slip); EDI view under Purchasing (PO send, intake, three-way match, acks, price-application review, transport records, catalog CSV import)
### Exit tests
- ✅ (a) X12 850/856 round-trip — PO → 850 sent via the local SFTP stand-in → 855/856/810 intake → three-way match `matched` at 477.00 → 832 apply report (reprice/add/skip); mark-applied leaves product prices unchanged; re-sending a sent PO fails; an 855 for an unknown PO errors
- ✅ (b) Delivery cannot close without recorded signature + photo reference (POD gating) — direct jump to `delivered` refused; signature-only POD refused up front with nothing written (delivery stays `out_for_delivery`, no audit event, no pod row); sig+photo closes out with audit event + packing slip rendering the sale's line/doc number; linked open order auto-closes to `completed`
- ✅ (c) Lapsed-contractor report includes a stale contractor, excludes a recent one — never-purchased (B) first, then lapsed (A, 100 days); recent buyer (C) and retail (D) absent; 200-day window clears A; tickler complete/snooze rules and errors; same-contractor and same-staff overlap blocked, adjacent/parallel free, cancel frees the slot; saved-view dialects kept distinct (activity view matches open follow-ups only; customer view matches buyers only; customer endpoint rejects activity-dialect JSON)
### Verification
- ✅ `cargo test --workspace` — 327/327 passing (58 suites, 0 failed; `cargo nextest run --workspace` 325/325)
- ✅ clippy clean (`-D warnings`), fmt clean, back-office `npm run build` clean (tsc + Vite, 32 modules, 278.60 kB JS)
### GitHub
- ✅ Pushed Nov 26 via connector in ONE commit `65e96f0` (224 files, Phases 3 + 4 together, per Kiel's pick)
### Known limits
- SFTP is a local stand-in only (no SSH/remote/delivery confirmation); AS2 has no network/crypto; POD is bytes+hash over JSON, no camera hardware, no photo stored; ack application is a report, PO untouched; 832 mark-applied records review only, no repricing; lapsed report is edge-only (no df-api read); activity-filter endpoint parses leniently, so running it on a customer-dialect view matches every activity — use the customer endpoint for customer views; PG round-trip still untested.
- Fixes folded in during verification: df-api version strings 0.4.0 → 0.5.0 (health test), df-edge re-exports/signatures aligned to the sibling crates (old 4-arg `assign_delivery` removed, 7-arg state-machine version kept), `Serialize` added to df-edi row types, df-server `run_edi_intake` refactored behind an `EdiCtx` struct (clippy too-many-arguments).

## Phase 5 — Mobile PWA/Tauri & Contractor Portal (v0.6) — ✅ DONE Nov 26, 2026
### Scope
- ✅ `df-portal` (new crate, pure domain) — portal access (`portal_enabled` gate), bill-pay intent validation, formula-reorder requests, BOPIS order validation, mobile KPIs view
- ✅ Migration `0010_phase5` — `portal_payments` (idempotency-keyed bill payments, provider reference only, no PAN/CVV) + `bopis_orders` (guest pickup contact) (both dialects)
- ✅ `df-edge` portal wiring — portal customer/invoices/statements/formulas/payments, formula reorder (priced via matrix path, line linked to formula), bill pay (mock-provider authorize → FIFO AR post, idempotent replay), catalog, BOPIS order + pickup completion, mobile KPIs
- ✅ `mobile-pwa` — React 19 + TS + Vite on :1422, offline-first (hand-rolled SW + manifest, localStorage outbox + manual Sync), Tauri 2 Mobile wrapper files (`src-tauri/tauri.conf.json`, unverified — no Android SDK in VM)
- ✅ Warehouse — PO receive, counts, transfers (all real edge API)
- ✅ Outside Sales — KPIs, CRM, quotes (all real edge API)
- ✅ Delivery Driver — POD flow (canvas signature + photo file → SHA-256; server gates on both)
- ✅ `web-portal` — React 19 + TS + Vite on :1423, 24/7 Contractor Portal: overview, invoices, statements, formula reorders, US/CA online bill pay (mock provider)
- ✅ Retail BOPIS storefront — React 19 + TS + Vite on :1424: public catalog, cart, mock checkout, order status + counter pickup completion
- ✅ Demo seed `apps/phase5-seed-demo.sql` — portal contractor C-PORTAL (700.00 open), formula OC-17 White Dove, 10 gal on hand
### Exit test
- ✅ Contractor online formula reorder + statement payment auto-applied to AR — Rust exit test `phase5_portal_exit` + my own live HTTP re-verification: reorder TX-000001 (2 gal, grand 216.1088) → bill pay 150.00 card (mock_1, FIFO to oldest 400.00 charge) → balance 700.00 → 550.00 exactly, replay returns same entry, statement closing 550.00; BOPIS order TX-000002 → pickup completed, on-hand 10 → 8, committed 0
### Verification
- ✅ `cargo test --workspace` — 328/328 passing (Phase 4 was 327; Phase 5 added 1 exit test)
- ✅ clippy clean, fmt clean
- ✅ Frontend `npm run build` clean — mobile-pwa (255 kB JS), web-portal (237 kB), web-storefront (236 kB)
- ✅ Live HTTP verification by me from a clean seeded edge: portal reorder + bill pay + replay + BOPIS order/pickup + KPIs (open_orders 2, AR 550.00)
### GitHub
- ✅ Pushed Nov 26 via connector in ONE commit `e4ae658` (279 files, 57 new; 2 binary PWA icons excluded — connector cannot carry binaries)
### Known limits
- PG round-trip still untested; bill pay has no surcharge in v0.6; portal has no separate login (customer id is the scope — anyone with the id can view that contractor's portal); mobile KPIs use the UTC calendar day (store-local day is a follow-up); BOPIS pickup completion is not idempotent (a double-complete is refused as already-completed, but the mobile outbox replay of POD/counts/PO-receives can double-apply — only portal bill-pay is idempotent); POD stores photo ref + hash only, no photo bytes, no upload endpoint; Tauri Mobile wrapper unverified (no Android SDK in VM); statements tab is empty until a month-end run exists; API-shape notes: Money currency serializes as `"Usd"`/`"Cad"` (not `"USD"`), reorder fulfillment over HTTP is `"Pickup"`/`"Delivery"` (variant names), statements return raw decimal strings (not Money objects); Vite dev binds `::1` only — use `http://localhost:<port>`, not `127.0.0.1`.
- Fixes folded in during verification: health-version test assert 0.5.0 → 0.6.0; `default_provider` made `pub(crate)` for the portal module; `insert_order_tx` args bundled into `OrderTxParams` (clippy too-many-arguments).

## Phase 6 — BI, Scheduling+, Migrator & v1.0 — ✅ DONE Nov 26, 2026
### Scope
- ✅ `df-reports` KPI dashboards + ad-hoc inquiries — sales, margin (revenue − COGS), inventory turns from the append-only ledger, AR aging (same derivation as statements), formula usage (tracking only); `GET /bi/kpi`, `/bi/inquiry`, BI tab in web-backoffice
- ✅ PaintCare/Eco remittance reports — eco-fee totals by (country, region, store, category) from real transaction lines, JSON + CSV with TOTAL row; `GET /bi/eco?format=json|csv`
- ✅ Excel/PDF scheduled emails — `scheduled-report-email` job (Daily 07:00 UTC) renders KPI PDF + eco CSV into a file-based email outbox (`manifest.json`, status `"queued"` — never `"sent"`); no SMTP anywhere; no native `.xlsx` (no xlsx crate vendored offline — CSV + PDF only)
- ✅ `df-scheduling` (new crate) — shift templates, staff shifts (overlap-checked), timeclock (double clock-in refused), sales-driven labor forecasting (sales per labor-hour ratio, per-day-of-week + per-hour buckets, `InsufficientHistory` rather than inventing a number), coverage check (±10% band); edge routes + Scheduling tab in web-backoffice; `labor-forecast-refresh` job (Daily 05:30 UTC)
- ✅ `df-migrator` (new crate, lib + standalone binary) — imports legacy Decor Fusion CSVs (customers, products, formulas as tracking records, AR balances, inventory) with SHA-256 file/row checksums, per-row imported/skipped/failed with reasons, deterministic UUIDv5 re-import is a no-op; `df-server migrate import` + `df-migrator import`; fixtures under `crates/df-migrator/fixtures/`
- ✅ Chaos-testing offline sync — `crates/df-sync/tests/chaos_sync.rs` + `crates/df-edge/tests/chaos_sync_edge.rs`: dropped/reordered/duplicated messages, two-edge same-aggregate edits, conflicting edits, outbox redrain, 40-round randomized sweep — all model-checked; found and fixed 2 real bugs (F6 partial applies silently lost — `sync_now` now marks only `applied_ids` synced; F7 one poisoned entry bricked a store's sync — conflicts now handled per-entry)
- ✅ Security/PCI review — `docs/security-review-v1.0.md`: no PAN/CVV anywhere (migration scan + payment-type key pinning tests), no hardcoded secrets, no f32/f64 in non-test Rust, all SQL parameterized, Argon2id auth; fixed F5 (DB credentials printed in logs — `redact_db_url` at all three sites); OPEN and recorded honestly: F1 df-api has no auth, F2 edge HTTP has no auth, F3 RBAC never enforced, F4 portal scope is the customer UUID, F8 no dead-letter for conflicts, F9 no TLS/mTLS on sync, F10 no CORS/rate limiting; `cargo audit` could not run (no binary, no network) — CI must run it before any release tag
- ✅ Docker/Helm self-hosting docs — `Dockerfile` (multi-stage rust:1.99 → slim, non-root), `docker-compose.yml` (edge :8080 + api :3000 + provisioned Postgres), Helm chart `deploy/helm/decor-pos/`, `docs/self-hosting.md`; release binaries smoke-tested live; `docker build` / `docker compose up` / `helm template` NOT run (no Docker daemon or Helm binary in VM) — stated in the docs
- ✅ Version bumped to 1.0.0 (workspace + health-version asserts)
### Exit tests
- ✅ v1.0 — all prior phases' exit tests re-run green by the coordinator: offline checkout parity, tint/formula end-to-end, AR statement batch, inventory back-office, Phase 4 EDI round-trip / POD close-out / CRM lapsed, Phase 5 portal exit, Phase 6 scheduling — plus 9 df-sync chaos tests and the migrator fixture test
- ✅ Migrator imports the sample legacy dataset with checksums matching — verified live by the coordinator: 24 rows read, 14 imported, 5 failed (all intentional bad rows with named reasons), re-import is a no-op for imported rows; inventory rows fail with "unknown store code MAIN" on an unseeded DB (documented behavior)
- ✅ Chaos sync tests pass — dropped 4 of 12 then redelivered to exact totals (inv −7, AR 200.0000); same batch 3× is a no-op (12 duplicates); two-edge same-aggregate merge inv −5, AR 150.0000; conflicting resend at 999.0000 rejected, stored TX stays 216.1088
### Verification
- ✅ `cargo test --workspace` — 402/402 passing (71 suites, 0 failed; Phase 5 was 328, Phase 6 added 74)
- ✅ clippy clean (`-D warnings`), fmt clean, web-backoffice `npm run build` clean (297.79 kB JS)
- ✅ Live checks by the coordinator: `df-server migrate import` end-to-end on real SQLite files; release `df-server`/`df-api` binaries smoke-tested (`/health` ok)
### GitHub
- ⬜ NOT pushed yet (local only; main still at `e4ae658`) — push via connector in one commit once Kiel asks; tag v1.0
### Known limits
- PG round-trip still untested (SQLite only in this VM); Postgres migrations compile-checked only.
- No native `.xlsx` — scheduled "Excel" artifact is CSV + PDF.
- Scheduling times are UTC in v1.0; no staff directory (staff_id is a UUID text field); overnight shift templates not supported.
- Skipped sync-conflict entries stay pending forever — no dead-letter path (open F8).
- `df-api` and edge HTTP have no auth (F1/F2); RBAC defined but never enforced (F3); portal scope is the customer UUID (F4).
- Docker image layers / compose runtime / Helm templates unverified until run on a Docker host (no daemon in VM).
- `df-server hq` / `migrate` (standalone mode) / `standalone` are unimplemented — Postgres is provisioned only in compose.
