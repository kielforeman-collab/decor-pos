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
- ✅ Pushed Nov 26 via connector in ONE commit `a553a19` (329 files, 52 new; 2 binary PWA icons excluded — connector cannot carry binaries)
- ✅ Tag v1.0 created Nov 26 via browser release page (connector has no tag tool), commit `a553a19`, marked Latest
### Known limits
- PG round-trip still untested (SQLite only in this VM); Postgres migrations compile-checked only.
- No native `.xlsx` — scheduled "Excel" artifact is CSV + PDF.
- Scheduling times are UTC in v1.0; no staff directory (staff_id is a UUID text field); overnight shift templates not supported.
- Skipped sync-conflict entries stay pending forever — no dead-letter path (open F8).
- `df-api` and edge HTTP have no auth (F1/F2); RBAC defined but never enforced (F3); portal scope is the customer UUID (F4).
- Docker image layers / compose runtime / Helm templates unverified until run on a Docker host (no daemon in VM).
- `df-server hq` / `migrate` (standalone mode) / `standalone` are unimplemented — Postgres is provisioned only in compose.

## Phase 7 — Authentication (v1.1) — ✅ DONE Nov 26, 2026
### Scope
- ✅ Auth core — opaque session tokens (`dfp_` + 32 random bytes, SHA-256 hash stored server-side, never the raw token), 8h staff TTL / 30d portal remember-me (explicit only), migration `0012_phase7` (`roles` seed with fixed UUIDs, `users.portal_customer_id`, `sessions`), user + session repos, `df-server users create-admin` / `create` / `list` / `revoke-sessions` CLI (no hardcoded default password; second admin needs `--force` + typed `YES`)
- ✅ HTTP enforcement — valid token required on ALL df-api + df-edge routes except GET /health (+ public OpenAPI doc, verified data-free); 401 unauthenticated, 403 wrong role; RBAC enforced per route from df-auth roles (role→route matrix in docs/auth.md §6); login/counter-switch/override-verify implemented on both surfaces (were 501); logout revokes; actor identity comes from the session (`cashier_id` in request bodies ignored)
- ✅ Portal login (F4) — real contractor login (`POST /pos/auth/portal-login`) replaces customer-UUID-as-scope; session carries the linked `customer_id`; another contractor's id → 403; `portal_enabled` remains a second gate; BOPIS catalog/order placement/status stay public by design, pickup completion requires staff auth
- ✅ Frontend login screens — pos-desktop (password + PIN/badge counter-switch, manager override prompts for void/price-below-cost), web-backoffice (role-gated tabs, UI-only), web-portal (`?customer=` identity removed — customer id comes only from the login response), mobile-pwa (outbox kept on logout, replay refuses while logged out); token key `df-pos-token`, `Authorization: Bearer`, 401 → login; public BOPIS storefront untouched
- ✅ Security review update — F1–F4 marked FIXED with mechanism in docs/security-review-v1.0.md; F8/F9/F10 stay OPEN (not in this phase)
- ✅ Coordinator fixes during verification — df-jobs wall-clock time-bomb fixed (`run_now_at` stamps the simulated instant; `run_due` uses it); `run_users_create` 10-arg clippy error fixed by bundling into `CreateUserArgs`; two RBAC holes found by the HTTP worker and closed: scheduling now has real permissions (`SchedulingView` every staff role, `SchedulingManage` Manager/Admin) and `PortalContractor` holds no staff permissions (its `ArView` removed — it had leaked other customers' AR summaries through the unscoped staff AR routes); docs/auth.md §6/§9 + security review updated to match
### Exit tests
- ✅ (a) Unauthenticated → 401 on protected edge + HQ routes; GET /health → 200 without a token on both; OpenAPI → 200 with no data rows
- ✅ (b) Cashier token → 403 on manager-only routes (edge reorder-points, HQ colorant levels); manager token → 200
- ✅ (c) Portal login as contractor A → 200 + token with `customer_id` = A; B's statements → 403; A's own → 200; reorder for A → 200, for B → 403
- ✅ (d) Expired token (minted −1s TTL) → 401 on both surfaces; logout → 200 `{revoked: true}`, token then → 401
- ✅ (e) All prior phases' exit tests re-run green by the coordinator: 26 df-edge exit tests + 9 df-sync chaos + 1 migrator fixture, 0 failed
### Verification
- ✅ `cargo test --workspace` — 439/439 passing, 0 failed (Phase 6 baseline: 402; Phase 7 added 37)
- ✅ clippy clean (`-D warnings`, workspace-wide), fmt clean, `npm run build` clean in pos-desktop (253 kB), web-backoffice (304 kB), web-portal (241 kB), mobile-pwa (262 kB)
- ✅ Exit-test files re-run by the coordinator: `df-edge/tests/phase7_http_auth.rs` 4/4, `df-api/tests/phase7_http_auth.rs` 3/3
### GitHub
- ✅ Pushed Nov 26 via connector in ONE commit `1d7cf78` (352 files, 25 new; 2 binary PWA icons excluded — connector cannot carry binaries)
- ✅ Tagged v1.1 Nov 26 via browser release page (commit `1d7cf78`, marked Latest): https://github.com/kielforeman-collab/decor-pos/releases/tag/v1.1
### Known limits
- Tokens live in localStorage (`df-pos-token`), readable by any script on the origin — httpOnly cookies are the follow-up before exposing the POS beyond the store network.
- No login rate limiting/lockout; no refresh/sliding sessions (8h is hard); no expired-session janitor (expired rows rejected on read, never deleted).
- PIN/badge login identifies by email — a bare PIN/badge cannot name its user (hashes can't be searched); badge-only tap-to-switch needs a non-secret badge identifier column.
- Audit actor columns: void/return record no actor; override `reason_code` accepted but not persisted.
- EDI vendor config has no separate permission (all `/edi/*` is `ReportView`-gated).
- Tauri `invoke()` calls cannot carry a Bearer token (local IPC) — login gates the UI in Tauri mode, but in-process command enforcement belongs to the Tauri layer.
- PG round-trip still untested; F8/F9/F10 stay OPEN (dead-letter, TLS/mTLS, CORS/rate limiting).

## Phase 8 — PostgreSQL HQ (v1.2) — ✅ DONE Nov 26, 2026
### Scope
- ✅ `df-server hq` — `df-server hq --db <url> --org-id <uuid> [--store-id <uuid>] [--listen <addr>]` (default 127.0.0.1:3000); `--db` falls back to `DATABASE_URL` env (`--db` wins; exit 2 when neither is set); connects via `df-db::connect`, applies pending migrations, serves the `df-api` router with the pool in `ApiState` — Phase 7 auth gate reused verbatim; logs the redacted URL, migration counts, listen address; never logs the password
- ✅ `df-server migrate schema` — applies pending migrations, prints JSON `{database (redacted), dialect, status, newly_applied, applied}`; second run `"already_up_to_date"`, exit 0. Reporting reads sqlx's own `_sqlx_migrations` table before/after (sqlx 0.8.6 does not expose applied migrations on the migrator API)
- ✅ df-db Postgres parity — the AnyPool layer could NOT run on Postgres at all (found live, not by audit): sqlx 0.8.6 `Any` does not rewrite `?` → `$N` (reaches PG literally, where `?` is the JSONB operator), cannot bind TEXT params to uuid/numeric/timestamptz columns, and cannot decode native PG types on read. Fix: all queries rewritten `?` → `$N` (803 placeholders, 218 literals, 19 files) and Postgres migrations use TEXT columns with the same encoding SQLite already uses (ids hyphenated, money exact decimal strings, timestamps RFC 3339, JSON verbatim). Native PG types remain a future data-layer phase (logical schema identical, reversible from here). Also fixed: `payments.authorization` is a reserved word in PG — quoted `"authorization"` in the Postgres migration + repo INSERT. New migration `0013_phase8` (both dialects): `sync_applied_entries` + `sync_conflicts` tables for idempotent ingest
- ✅ HTTP sync ingest — `POST /api/v1/sync/ingest` on df-api, gated by `Permission::SyncIngest` (Manager/Admin staff session token only — no separate service token; recorded in docs/auth.md §6: no new credential type to build/rotate, 8h TTL is the cost). Accepts a `DeltaBatch`, applies through a DB-backed `DbHq` sink (one transaction per batch); idempotency at the DB layer via `sync_applied_entries` (`ON CONFLICT(entry_id) DO NOTHING` — replay counted duplicate, nothing else changes); same-id/different-content → `sync_conflicts`, stored row never overwritten. Edge push client: `RemoteHq` (`POST {base}/api/v1/sync/ingest`, Bearer, 30s timeout) wired via `DF_HQ_URL` + `DF_HQ_TOKEN` env (read in `Edge::open`); `sync_now_with_configured_hq` prefers remote, then in-process `MemoryHq`, else `NoHq`. Payloads enriched additively (job_id/po_number on transactions, all four deltas + unit_cost/lot on inventory, reference_type/due_date on AR) — also fixes the transfer "close" leg's `delta_in_transit` being lost in sync
- ✅ Version bump to 1.2.0 for real — workspace Cargo.toml, df-api health version, df-edge health (via `CARGO_PKG_VERSION`), all version-asserting tests; `grep -rn '1\.1\.0'` clean; `df-server --version` prints `1.2.0`
- ✅ Docs — docs/self-hosting.md (HQ against PostgreSQL section, local apt Postgres provisioning, v1.2 validation notes) + docs/auth.md (§6 SyncIngest matrix row + decision note, §7 HQ bootstrap variant)
### Exit tests
- ✅ (a) `df-server migrate schema` against fresh live Postgres applies 12 migrations `[1,2,3,4,5,6,7,9,10,11,12,13]` (no 0008), second run `already_up_to_date` — re-verified by the coordinator manually against the live DB
- ✅ (b) `df-server hq` against live Postgres: GET /health → 200 `{"status":"ok","version":"1.2.0"}`; 3 protected routes without a token → 401
- ✅ (c) Live round-trip (re-run by the coordinator): offline SQLite edge cash sale 2 gal (grand 173.2088, TX-000001) + charge sale 1 gal (grand 86.6044, TX-000002) → HTTP ingest `applied=5 duplicates=0`; HQ inventory −3 (edge −3), HQ AR 86.6044 (edge 86.6044), rows inv=2 ar=1 tx=2; replay `applied=0 duplicates=5`, figures unchanged
- ✅ (d) Portal + AR reads against Postgres HQ return the same figures as the SQLite HQ path for the same fixture (repo-level + HTTP-level via two spawned `hq` instances; inventory on_hand −3, AR 86.6044, portal payment 150.0000)
- ✅ (e) All prior phases' exit tests re-run green: full workspace suite 458/0 (see Verification); Worker 3's per-file table: 57/57 across df-edge/df-sync/df-api/df-migrator/df-server exit-test files
### Verification
- ✅ `cargo test --workspace --no-fail-fast` re-run by the coordinator — **458 passed, 0 failed** (Phase 7 baseline: 439; Phase 8 added 19)
- ✅ `cargo test -p df-server --test postgres_hq_round_trip` re-run by the coordinator — **4/4 pass live** (5.9s); skips cleanly with a printed reason when no live Postgres is reachable
- ✅ clippy clean (`-D warnings`, workspace-wide), fmt clean — both re-run by the coordinator. No frontend touched, no `npm run build` needed
### GitHub
- ✅ Pushed Nov 26 via connector in ONE commit `a7fc201` (361 files, 11 new; 2 binary PWA icons excluded — connector cannot carry binaries)
- ✅ Tagged v1.2 Nov 26 via browser release page (commit `a7fc201`, marked Latest): https://github.com/kielforeman-collab/decor-pos/releases/tag/v1.2
### Known limits
- Postgres columns are TEXT, not native UUID/NUMERIC/TIMESTAMPTZ — deliberate Phase 8 trade (see Scope); native types are a future data-layer phase. Spec §5's native-type language is not yet met.
- HTTP ingest auth uses a staff Manager/Admin session token (8h TTL, re-login on expiry) — no long-lived service token yet.
- `RemoteHq` edge→HQ push is exercised through the ingest endpoint test, not as a live edge-daemon push; `Edge::open` always runs SQLite migrations (edge is SQLite-only).
- `df-server standalone` / `worker` remain Phase 0 stubs (exit 2). `docker-compose.yml` still runs the `df-api` service (not switched to `df-server hq`).
- The VM's Postgres is ephemeral by design: a VM restart wipes `/tmp` (including the URL/password files) and the installed server — re-provision with the apt steps in docs/self-hosting.md. Kiel's Linux box (100.94.29.18) already has Postgres 18.3 + decor-pos at v1.1 as a fallback host.
- PG round-trip test uses per-test fresh DBs (`decorpos_hq_rt_a/b/c/d`); `df-db pg_repos` shares the `decorpos_hq` DB and its rows accumulate (fresh UUIDv7 ids per run, no cleanup).
- F8/F9/F10 stay OPEN (dead-letter, TLS/mTLS, CORS/rate limiting). No COLORx code. No f32/f64 money code.

## GUI first-run setup (v1.2.1) — ✅ DONE Nov 26, 2026
### Scope
- ✅ First-admin setup in the GUI — both surfaces (HQ was a small reuse): public `GET /pos/auth/setup-status` + `POST /pos/auth/setup` on the edge, `GET /api/v1/auth/setup-status` + `POST /api/v1/auth/setup` on HQ. Setup creates the first Admin ONLY when the users table is empty (409 otherwise, nothing created); always Admin, no role parameter accepted (unknown `role` field → 422); Argon2id via existing df-auth/df-db; returns the same flat `LoginResponse` as login (fresh `dfp_` token). Shared service `df-db::setup_first_admin` (one transaction: count → ensure org row → insert user → re-count → insert session → commit); org_id from the edge/HQ configured org, org row created in-transaction when missing (PG FK needs it); `count_users` helper + executor variants in df-db/df-auth-http. pos-desktop login screen calls setup-status on load: `needs_setup` true → "Create your admin account" form (email, display name, password + confirm) instead of login, success logs the new admin straight in (same localStorage keys); false or status-call failure → the existing login form unchanged, login never blocked
- ✅ Coordinator fix during verification — the Phase 7 frontend `normalizeSession` (all four apps) could NOT parse the real backend response: the backend `LoginResponse` is FLAT (`{token, user_id, email, ...}`), the normalizer only read a nested `user` object, so `login()` and the new `setupAdmin()` would have thrown "Login response did not include a user id" on a successful response. Fixed in the normalizer (the one place auth.ts names for shape fixes): fall back to the top-level object when no nested user exists. Proven with a real esbuild-bundled node test importing the actual `auth.ts`: flat shape parses (token, user_id, email, display_name, role Admin, customer_id null), nested shape still accepted, missing token still throws. Same one-line fix applied to web-backoffice, web-portal, mobile-pwa (identical defect in all four)
- ✅ One-command `.env` setup: `setup.ps1` (Windows) + `setup.sh` (repo root, chmod +x) — create `.env` from `.env.example` ONLY if missing, fresh UUIDs for STORE_ID/ORG_ID, random 32-hex-char POSTGRES_PASSWORD from a crypto RNG; never overwrite (refusal message, exit 1); password printed ONCE with a "SAVE THIS PASSWORD NOW" note
- ✅ Docs: docs/self-hosting.md Docker admin step is now the GUI flow (open the POS at :1420, "Create your admin account" form); CLI `df-server users create-admin` stays as the alternative; setup.ps1/setup.sh in the Windows instructions
- ✅ Version bump to 1.2.1 for real — workspace Cargo.toml, df-api health version, df-edge health (via `CARGO_PKG_VERSION`), all version-asserting tests + docs examples; `grep -rn '"1.2.0"'` clean; `df-server --version` prints `1.2.1`
### Exit tests
- ✅ (a) Fresh DB: setup-status → needs_setup true; POST setup → 201 + `dfp_` token + role Admin + display_name defaulted to email local part; login with the new credentials → 200; setup-status after → needs_setup false (edge + HQ test files, 5 tests each)
- ✅ (b) Second POST setup (different email) → 409, user count unchanged at 1; concurrent setups → exactly one 201 + one 409, count 1
- ✅ (c) Blank email / blank password / password < 8 chars → 400, user count 0; `role` field in the body → 422, count 0
- ✅ (d) pos-desktop login screen shows the setup form when needs_setup is true and the normal login form otherwise — verified by the coordinator by tracing the code path (App.tsx:552 renders Login only; Login useEffect calls getSetupStatus once; null → "Checking first-run status…", true → setup form, false/failure → login form); no component test (pos-desktop has no test setup, none added)
- ✅ (e) setup.sh executed by the coordinator in a scratch dir: run 1 exit 0, .env with fresh UUIDs + 32-hex password; run 2 exit 1, refusal message, .env byte-identical. setup.ps1 read-verified only (no PowerShell in this VM) — its guard/exit-1/RNG/UTF-8-no-BOM logic checked by reading; needs one real Windows run before it is trusted
- ✅ (f) All prior phases' exit tests still green: full workspace suite 468/0 (see Verification); PG round-trip test re-run LIVE by the coordinator against a re-provisioned Postgres 16 (the VM restarted again and wiped /tmp; package survived, cluster re-started, role + round-trip DB re-created) — 4/4 in 5.57s
### Verification
- ✅ `cargo test --workspace --no-fail-fast` re-run by the coordinator — **468 passed, 0 failed** (Phase 8 baseline: 458; +10 new setup tests, 5 edge + 5 HQ). Full suite ran BEFORE the 1.2.1 version bump; the bump touched version strings only and every version-asserting test re-ran green after it (df-api lib 11/11, df-edge lib 12/12, hq_mode 1/1, phase7 api auth 3/3)
- ✅ `cargo test -p df-server --test postgres_hq_round_trip` re-run LIVE by the coordinator — **4/4 pass** (5.57s)
- ✅ clippy clean (`-D warnings`, workspace-wide), fmt clean — both re-run by the coordinator. `npm run build` clean in all four touched apps (pos-desktop 255.84 kB, others build clean)
- ✅ Normalizer fix proven by the coordinator with a real esbuild-bundled node test against the actual `auth.ts` (see Scope) — flat + nested + missing-token cases, all pass
### GitHub
- ✅ Pushed Nov 26 via connector in ONE commit `2d5d389` (365 files, 6 new; 2 binary PWA icons excluded — connector cannot carry binaries)
### Known limits
- setup.ps1 is read-verified only (no PowerShell in this VM) — needs one real Windows run before it is trusted.
- Scripts generate UUIDv4, not the UUIDv7 the `.env.example` comment suggests — `Uuid::parse_str` accepts any version, so v4 works, but if v7 is ever enforced the scripts need updating.
- Residual race window on PostgreSQL only: two concurrent setups with different emails could both commit in the sliver between the second's post-insert re-count and its commit (needs a table lock or serializable isolation); on SQLite (the edge's real DB) the single-writer lock closes it, and the concurrent test passes. HQ setup is a one-time operator action, left as a documented limit.
- No email-format validation beyond non-blank (mirrors the CLI). The GUI flow needs the pos-desktop dev server (`npm run dev`, :1420) running against an edge on :8080 — the Docker Compose stack alone does not serve the POS UI.
- The VM's Postgres is ephemeral: a VM restart wipes /tmp (URL file) — the 16 package survived the last restart, only the cluster needed re-starting and the role/DB re-creating.
- F8/F9/F10 stay OPEN. No COLORx code. No f32/f64 money code.

## GUI product import — ✅ DONE Nov 26, 2026 (pushed `240f386`, no version bump)

**Follow-up fix Nov 26 (pushed `6b74934`):** the import failed live with `row not found: store <id>` because a fresh edge database has an empty `stores` table — `Edge::open` seeds nothing and no production path ever created the configured store. Fix: `Edge::ensure_configured_store()` (org + store from config, code `MAIN`, no-op when rows exist, never overwrites), called once in `df-server edge` startup. Country/region/postal/timezone are placeholders (no store-edit UI yet). Test `ensure_configured_store.rs` (seeds fresh DB, idempotent). Full `cargo test -p df-edge`: 62 passed, 0 failed; clippy/fmt clean.
### Scope
- ✅ "Import products" button on the back-office Inventory tab (apps/web-backoffice, :1421): file picker accepts .xlsx and .csv; the spreadsheet is parsed IN THE BROWSER with SheetJS (`xlsx` 0.18.5, new dep) — the file itself is never uploaded, only parsed JSON rows are POSTed. Reads sheet "EXCEL" when present, else the first sheet; first row is the header.
- ✅ Preview before anything is sent: row count, first ~5 ready rows (SKU, name, list price, net price), and every row that will be skipped with its reason (skipped list capped at 20 shown). Confirm button posts only the problem-free rows; result shows created / updated / failed plus a per-row failure table. Inventory refreshes after a successful import.
- ✅ Backend: `POST /pos/inventory/import` on df-edge (new `Edge::import_products` in crates/df-edge/src/inventory.rs + handler/route in http.rs), gated on `PricingManage` — Manager/Admin only. Warehouse holds `InventoryAdjust` but NOT `PricingManage`, so it gets 403 (proven by test). Route + matrix row added to docs/auth.md and the http.rs route docs.
- ✅ Upsert by `(org_id, sku)`: new SKUs created, existing SKUs repriced (name + list/standard/avg cost + manufacturer code only — department, tax class, type, UOM untouched), so re-importing the same file is safe. One bad row never fails the batch; failures carry the 1-based spreadsheet row number and a named reason. 10,000-row per-request cap; unknown store_id → 404; empty rows → 400.
- ✅ Row mapping / validation in unit-testable `df-db::product_import` (`parse_money`, `validate_import_row`, `import_products`) + `update_imported_product` repo fn. Prices are `Decimal`, "$"/commas stripped, negatives rejected. Column mapping: SKU → sku, Product Name → name, List Price → list_price, NET Price → standard_cost (and avg_cost on create), Brand Code → manufacturer_code. NOT stored (no columns): Category, French Description, French Category Name, Line, Color, Size, Product UPC, MSRP, MAP, Brand Discount Y/N, Brand Discount, Attention flags — the preview and this doc say so; MSRP/MAP are still validated when present (garbage → bad row). Defaults for columns the file cannot supply follow the existing seed pattern: product_type "paint_base", base_uom "gal", fresh `Uuid::now_v7()` department_id / tax_class_id (no departments or tax-class tables exist — free-form UUIDs, no FK). The malformed shifted row (sheet row 1012: every cell "K7931X-001") is rejected by the price check with reason `net_price "K7931X-001" is not a valid price` (backend) / `List Price price "K7931X-001" is not a valid price` (preview).
- ✅ Role-gating in the UI is Manager/Admin only (mirrors the BI-tab pattern); the backend enforces it — the button is hidden for other roles and the endpoint 403s them.
### Verification
- ✅ df-db: 7 unit tests (`product_import::tests`: money parsing, missing SKU/name, bad/missing prices, malformed shifted row, optional MSRP/MAP) + 4 SQLite integration tests (`sqlite_product_import.rs`: create-then-update on re-import, duplicate SKU in one file, bad rows don't fail the batch, malformed shifted row) — 11/11
- ✅ df-edge: 4 HTTP integration tests (`product_import.rs` over the real router): unauthenticated → 401; Cashier + Warehouse → 403, Manager + Admin → 200; mixed batch (1 create + 1 update + 3 failures incl. row 1012) with DB repricing verified; unknown store → 404 — 4/4
- ✅ Dry-run of the REAL frontend parser against the REAL file (esbuild-bundled `productImport.ts` run in node, read-only, nothing written to any database): 1,228 rows parsed, 1,227 ready, 1 skipped (sheet row 1012, malformed row, named reason), 0 duplicate SKUs
- ✅ Frontend builds: `npm run build` (tsc + vite) clean in apps/web-backoffice (bundle 646.96 kB — SheetJS; chunk-size warning only, no error)
- ✅ `cargo fmt --check` clean and `cargo clippy -p df-db -p df-edge --all-targets` clean for the touched crates (clippy run before the final fmt pass; fmt touches formatting only)
- ✅ Full `cargo test -p df-db -p df-edge` suite: **115 passed, 0 failed** (exit 0; ~10.5 min, dominated by the slow Argon2-seeded auth tests — no failures anywhere, including all pre-existing suites)
### GitHub
- ✅ Pushed Nov 26 via connector in ONE commit `240f386` (369 files, 6 new; 2 binary PWA icons excluded — connector cannot carry binaries)
### Known limits
- Imported products carry the seed defaults (product_type "paint_base", base_uom "gal", fresh department/tax-class UUIDs) — the price list has no UOM/type/department columns. A later pass could map BM Size codes to UOMs or real departments; deliberately not invented here.
- Category / UPC / MSRP / MAP are not stored — if Kiel wants them searchable later, `products` needs new columns (a migration, not a silent drop).
- The 10,000-row cap and the per-row (non-transactional) upsert are deliberate: a failure mid-batch leaves earlier rows imported, reported in the result.
- The frontend bundle grew ~430 kB (SheetJS); no code-splitting done — back-office only, acceptable.
