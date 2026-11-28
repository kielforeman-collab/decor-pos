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
- ✅ All four Phase 1 counter limits (provider refunds on returns, DB-backed holds, card auto-void on failed commit, and real signed-in cashier attribution) were closed on Nov 28, 2026 (see bottom section).

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
- ✅ Machine-tinting code (`df-tinter`, `df-plugin-sdk/src/tinter.rs`, canister `colorant_levels`/`tint_jobs` repos, `shortage-alerts`, `/pos/tinting/dispense`) removed Nov 28, 2026 (see bottom section). Legacy `0003_phase2` SQL migrations stay untouched for existing DB checksums.
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

## CI installer downloads (v1.2.2) — ✅ DONE Nov 28, 2026
### Scope
- ✅ Release workflow `.github/workflows/release.yml` — on tags `v*`, two build jobs: Windows (`windows-latest`, free for public repos with MSVC/Windows SDK/NSIS/WiX) builds the Decor POS desktop installer (NSIS `.exe` + WiX `.msi`) + `df-server` Windows binary; Linux (`groundbox-4vcpu`) builds the desktop installer (`.rpm` + `.AppImage`, switched from `.deb` in `7fd897b`) + `df-server` Linux binary. Each build job uploads run artifacts; a `publish` job (`groundbox-4vcpu`, tag runs only) attaches both sets to the GitHub release for the tag, so two jobs never race creating the release. `workflow_dispatch` supports a `platform` choice (`all` / `linux` / `windows`) and stops at artifacts — no release — which is how the pipeline is tested before any tag.
- ✅ Existing CI check job moved `ubuntu-latest` → `groundbox-4vcpu` (one-line change per Groundbox docs). Groundbox free plan: 3,000 CI minutes/month; the GitHub App must be installed for this repo or Groundbox jobs queue forever.
- ✅ Version bump 1.2.1 → 1.2.2 for real — workspace Cargo.toml, df-api health version, all version-asserting tests, docs/self-hosting.md examples; tauri.conf.json + pos-desktop package.json were still at 0.2.0 (Phase 1) — both now 1.2.2, package-lock.json root version too.
- ✅ App icon as text SVG `apps/pos-desktop/src-tauri/icons/icon.svg` — same mark as the PWA icon-512.png (navy `#1a1a2e` ticket: white rounded rect 64..448, rx 48, cut by a 65px navy band at y 224..288, measured from the PNG). CI generates the binary icon set with `cargo tauri icon` before building; `tauri.conf.json` lists `32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.icns`, `icon.ico`, `icon.png`; no binary icon files are committed.
- ✅ Tauri builds use `cargo tauri build --features tauri-shell` (plain build exits 2 without the shell feature) with explicit bundles (`nsis,msi` on Windows, `rpm,appimage` on Linux); Linux job installs the webkit/appindicator/librsvg/patchelf deps first.
### Verification
- ✅ Both workflow files parse as YAML and pass on GitHub Actions
- ✅ CI on `82a2967` passed (run `46120986250`, 2m44s — Rust `check` + all 4 frontend builds)
- ✅ Version-asserting tests re-run green after the 1.2.2 bump: df-api lib 11/11, df-edge lib 22/22, df-server hq_mode 1/1, df-api phase7_http_auth 3/3, df-server postgres_hq_round_trip 4/4 (live leg skipped — no live Postgres in this VM)
- ✅ `workflow_dispatch` dry-run of `release.yml` on `82a2967` **PASSED** (run `46120995977`: `build-linux` 7m6s, `build-windows` 23m39s; both `decor-pos-linux` and `decor-pos-windows` artifacts uploaded cleanly)
- ✅ Tagged release `v1.2.2` **PUBLISHED** (re-tagged on `6b6bb73`, run `46130155428`: `build-linux` 7m42s, `build-windows` 22m12s, `publish` 13s; earlier `.deb` run `46122843029` superseded) — all 6 assets verified live on GitHub Release `v1.2.2`:
  - `Decor.POS_1.2.2_x64-setup.exe` (5.8 MB) — Windows NSIS installer
  - `Decor.POS_1.2.2_x64_en-US.msi` (8.3 MB) — Windows MSI installer
  - `df-server-v1.2.2-windows-x86_64.exe` (22.6 MB) — Windows server binary
  - `Decor.POS-1.2.2-1.x86_64.rpm` (9.4 MB) — Linux RPM package
  - `Decor.POS_1.2.2_amd64.AppImage` (87.3 MB) — Linux AppImage
  - `df-server-v1.2.2-linux-x86_64` (27.8 MB) — Linux server binary
### GitHub
- ✅ Historical troubleshooting: Release #1 (`46107924150`) failed on Windows (no bash for `dtolnay/rust-toolchain`) and Linux (missing `bundle.icon` for AppImage); Release #2 (`46109942823`) & #3 (`46111316618`) passed Linux, failed Windows on `groundbox-windows-4vcpu` because `rustup` defaulted to `x86_64-pc-windows-gnu` without `dlltool.exe`.
- ✅ Fixes committed & pushed: `82a2967` switched `build-windows` to GitHub-hosted `windows-latest` + `dtolnay/rust-toolchain@stable` and added `icons/icon.ico`/`icons/icon.icns` to `tauri.conf.json`; `7fd897b` switched Linux bundle from `.deb` to `.rpm` (verified in run `46125808511`).
- ✅ Release `v1.2.2` re-tagged at `6b6bb73` (CI run `46126461287` passed) and published live with `.rpm`: https://github.com/kielforeman-collab/decor-pos/releases/tag/v1.2.2
### Known limits
- The release jobs do not re-run the test suite: a tag points at a main commit, and the check job runs on every push to main, so tagged code has already passed tests + clippy + fmt. A tag cut on a commit whose check run has not finished yet would ship untested code.
- `df-server` is the only server binary in the release (edge + HQ modes in one binary); the mobile PWA / Tauri Mobile wrapper is not built here.
- Windows installer needs WebView2 on the target machine (standard on Windows 10/11); compiling `tauri-cli` from source makes the Windows job ~24 min (switching to prebuilt `@tauri-apps/cli` or `taiki-e/install-action` is an easy future speedup).

## pos-desktop polish & default theme — ✅ DONE Nov 27, 2026
### Scope
- ✅ Multi-line same-SKU ticket preview fix (`apps/pos-desktop/src/components/Ticket.tsx`): `lineAmount` matches server preview lines by row index (verified against `product_id`) instead of `.find()`, and uses exact decimal helpers (`mulDecimals` + `discountedPrice`) instead of `Number()` for provisional totals.
- ✅ Real tint color swatches (`types.ts`, `TintModal.tsx`, `Ticket.tsx`): added optional `colorHex?: string | null` to `TintInfo`, persisted `rgb_hex` through `TintModal`, and rendered the actual swatch hex on ticket lines instead of hardcoded `#3d4a57`.
- ✅ Sidebar cleanup (`Sidebar.tsx`): removed hardcoded `badge: '2'` on `Deliveries`.
- ✅ Custom theme contrast (`styles.css`): `:root[data-theme="custom"]` now derives `--panel`, `--panel-2`, `--field`, `--ink-2`, `--ink-3`, `--line`, `--line-strong`, and `--sel` via `color-mix(in srgb, ...)` for readable hierarchy.
- ✅ Lazy-loaded SheetJS (`productImport.ts`): converted `xlsx` to a type-only import + dynamic `await import('xlsx')` inside `parseImportFile`, splitting SheetJS into its own on-demand chunk.
- ✅ Default theme (`themes.ts`, `styles.css`): switched `DEFAULT_THEME_ID` and `:root` fallback tokens from `tos` to `mono` (`Document · B/W`) so initial paint matches without FOUC; all themes (`tos`, `lcars`, `expanse`, `dark`, custom) stay selectable.
- ✅ `.gitignore`: added `node_modules/`, `dist/`, and `*.tsbuildinfo`.
### Verification
- ✅ `apps/pos-desktop` `npm run build` (`tsc -b && vite build`) clean with zero errors and zero chunk-size warnings: main JS bundle `384.13 kB` (down from ~720 kB), lazy `xlsx` chunk `429.52 kB`.
### GitHub
- ✅ Pushed to `main`: `0f40ba8` (polish fixes + `.gitignore`) and `ff901ab` (default theme `mono`).

## Backend monolith modularization — ✅ DONE Nov 27, 2026
### Scope
- ✅ `crates/df-edge/src/http.rs` (3,771 lines) → `crates/df-edge/src/http/`: split into `mod.rs` (router, WS, shared error/query helpers, public re-exports), `pos.rs`, `bi.rs`, `inventory.rs`, `crm.rs`, `delivery.rs`, `edi.rs`, `portal.rs`, `scheduling.rs`, `auth.rs`, and `tests.rs` (0 behavior change, all `df_edge::http::*` exports preserved).
- ✅ `crates/df-edge/src/edge.rs` (1,965 lines) → `crates/df-edge/src/edge/`: split into `mod.rs` (`Edge` struct, init/config/pricing/serve/`test_support`), `checkout.rs`, `tills.rs`, `returns.rs`, `holds_quotes.rs`, and `sync.rs`.
- ✅ `crates/df-api/src/lib.rs` (2,070 lines) → split into `lib.rs` (state, OpenAPI `ApiDoc`, router, re-exports), `schemas.rs`, `auth.rs`, `tinting.rs`, `reports.rs`, `reads.rs`, and `tests.rs` (all public exports and OpenAPI 3.1 paths preserved).
- ✅ `crates/df-server/src/main.rs` (2,197 lines) → split into `main.rs` (dispatch + unit tests), `cli.rs`, `db.rs`, `hq.rs`, `edge.rs`, `ops.rs`, `edi.rs`, and `users.rs`.
- ✅ `crates/df-edge/src/inventory.rs` (1,554 lines) → `crates/df-edge/src/inventory/`: split into `mod.rs` (balances, reorder points, product import, shelf tags, shared outbox helper, public re-exports), `purchasing.rs` (replenishment + POs), `transfers.rs`, `counts.rs`, `pricing.rs`, and `ar_reads.rs`.
- ✅ Bonus Rust 1.100 clippy fix in `crates/df-db/src/vendors.rs`: replaced redundant closure with `crate::purchase_orders::po_from_row`.
### Verification
- ✅ `cargo fmt --all -- --check` — clean.
- ✅ `cargo clippy --workspace --all-targets -- -D warnings` — clean across all workspace crates.
- ✅ `cargo test -p df-api -p df-server -p df-edge` — **105 passed, 0 failed** across 27 test suites (after `1ce55f9`), plus `cargo test -p df-edge --lib --test inventory_backoffice --test product_import --test ar_statement_batch` — **38 passed, 0 failed** after the `inventory/` split.
### GitHub
- ✅ Pushed to `main`: `1ce55f9` (`http/`, `edge/`, `df-api`, `df-server`, `df-db` clippy fix) and `b4e2e8e` (`inventory/`).

## Frontend consolidation (`pos-desktop` absorbs `web-backoffice`) — ✅ DONE Nov 27, 2026
### Scope
- ✅ Absorbed every remaining `apps/web-backoffice` workflow into `apps/pos-desktop` using `pos-desktop`'s CSS token/theme system (no hardcoded colors, works across `mono`, `dark`, `expanse`, `lcars`, `tos`, and custom themes):
  - `backofficeTypes.ts` + `backofficeApi.ts` — modular types and authenticated `/api` client helpers (401 clears session).
  - `DeliveriesScreen.tsx` — dispatch board (status/date filters), create delivery, assign driver/vehicle, status transitions, 42-col packing slip preview, and POD close-out (signature + photo ref + SHA-256).
  - `CountsScreen.tsx` — start physical count, scan counted quantities via `ProductPicker`, reconcile variances and inspect posted adjustments.
  - `CrmScreen.tsx` — activities CRUD, due/overdue reminder tickler (complete/+24h snooze), appointments CRUD/cancel, saved views (activity + customer dialects), and lapsed-contractor report.
  - `SchedulingScreen.tsx` — shift templates, daily staff shifts & status, timeclock clock-in/out, labor forecast & coverage check.
  - `ReportsScreen.tsx` — daily usage & low-stock reports, PaintCare/Eco remittance with CSV download, ad-hoc transaction inquiry, and scheduled email outbox.
  - `PricingRulesPanel.tsx` + ZPL shelf tags integrated into `ProductsScreen.tsx` (`Catalog & stock` / `Pricing rules` tab + `Generate shelf tag (ZPL)` on product detail).
  - `EdiIntakePanel.tsx` integrated into `OrdersScreen.tsx` (collapsible `EDI intake & 832` panel for 855/856/810/832 intake, 832 review `mark-applied`, and vendor catalog CSV import).
- ✅ Updated `Sidebar.tsx` and `App.tsx` so all 12 sidebar items (`Transactions`, `Deliveries`, `Orders`, `Transfers`, `Counts`, `Products`, `Customers`, `CRM`, `Vendors`, `Scheduling`, `Dashboards`, `Reports`) are wired live.
- ✅ Retired and removed `apps/web-backoffice/` (~4.8k LOC duplicate React app deleted) and updated `README.md`, `docs/index.html`, and `docs/auth.md`.
- ✅ CI (`.github/workflows/ci.yml`): added `frontend` build matrix (`npm ci && npm run build` for `pos-desktop`, `mobile-pwa`, `web-portal`, `web-storefront`) and expanded the `f32`/`f64` guard across all 8 financial/transactional crates (`df-domain`, `df-db`, `df-auth`, `df-ar`, `df-accounting`, `df-edge`, `df-edi`, `df-payments`).
### Verification
- ✅ `apps/pos-desktop` `npm run build` (`tsc -b && vite build`) clean — main bundle `445.40 kB` (`122.08 kB` gzipped), lazy `xlsx` chunk `429.52 kB`, zero warnings.
- ✅ `apps/mobile-pwa`, `apps/web-portal`, `apps/web-storefront` `npm ci && npm run build` — all 3 clean.
- ✅ Float-money grep across all 8 financial crates — clean.
### GitHub
- ✅ Pushed to `main`: `099e1d9` (CI run `46119908512` passed).

## Machine-tinting cleanup & release speedup — ✅ DONE Nov 28, 2026
### Scope
- ✅ Removed dropped machine-tinting code (formula tracking only per Nov 26 rescope):
  - Deleted entire `crates/df-tinter/` crate (`VirtualTinter`, `TinterPlugin`, `TintJob`) and removed it from workspace `Cargo.toml`.
  - Removed `crates/df-plugin-sdk/src/tinter.rs` (`HttpTinterPlugin`) and replaced `examples/fake_tinter_plugin.rs` with generic `examples/fake_http_plugin.rs` for `PluginHost` integration tests (also eliminates the parallel port-race flake in `host_integration.rs`).
  - Removed `ColorantLevelRow`, `TintJobRow`, `DbError::InsufficientColorant`, and all canister-level / tint-job repo methods from `crates/df-db` (legacy `0003_phase2` SQL migrations preserved untouched for existing DB checksums); simplified `product_import.rs` to upsert catalog `colorants` only.
  - Deleted `crates/df-reports/src/shortage.rs`, removed colorant-shortage alerts from `low_stock_report`, and updated `daily_usage_report` to compute daily formula usage directly from tracked `tint_formulas`.
  - Removed `SHORTAGE_ALERTS_JOB` from `crates/df-jobs`, `POST /pos/tinting/dispense` and `Edge::tinter` from `crates/df-edge`, `POST /api/v1/tinting/colorants/levels` and `GET /api/v1/reports/shortage-alerts` from `crates/df-api`, and `--kind shortage` from `crates/df-server`.
  - Updated `GET /pos/tinting/colorants`, `GET /api/v1/tinting/colorants`, and `apps/pos-desktop` (`src-tauri` + `src/api.ts` + `src/types.ts`) to serve the org's `colorants` catalog directly (`Vec<ColorantView>`) for the formula builder without canister levels; replaced `tint_dispense_end_to_end.rs` with `tint_formula_end_to_end.rs`.
  - Updated `docs/auth.md` and `README.md`.
- ✅ Release workflow speedup (`.github/workflows/release.yml`):
  - Replaced `cargo install tauri-cli --version "^2" --locked` (compiling from source) with prebuilt `npm install -g @tauri-apps/cli@2` (`~5s`) + `tauri icon` (`~4s`) / `tauri build` on both Windows and Linux (quoting `--bundles "nsis,msi"` for `pwsh`).
  - Added `Swatinem/rust-cache@v2` (`shared-key: release-windows`, native Node 24) to `build-windows` so compiled release dependencies (`~715 MB` compressed) are cached across Windows releases.
  - Upgraded `actions/upload-artifact@v4` → `@v6` and `actions/download-artifact@v4` → `@v7` (native Node 24 runtime — zero Node 20 deprecation warnings on both Linux and Windows).
### Verification
- ✅ `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `apps/pos-desktop` `npm run build` (`445.36 kB` main bundle) — all clean.
- ✅ GitHub `CI` green on both `cf8e603` (run `46134785390`, 2m56s Rust `check` incl. nextest + clippy + fmt + doc tests + Typst + all 4 frontend builds) and `7229161` (run `46135286815`).
- ✅ GitHub `Release` dry-run timings (zero warnings/annotations):
  - **`build-linux`** (run `46134789543`, `cf8e603`): **4m39s** (down from ~7m42s).
  - **`build-windows`** (run `46135296077`, `7229161`): **15m11s** cold cache (down from **22m12s**, saving ~7m on cold builds; `Install Tauri CLI` dropped to **5s**, and `Swatinem/rust-cache@v2` saved the 715 MB compiled dependency cache for even faster warm runs).
### GitHub
- ✅ Pushed to `main`: `cf8e603` (`-3,720` net LOC machine-tinting cleanup + Tauri CLI / Node 24 workflow upgrades) and `7229161` (Windows `pwsh` `--bundles "nsis,msi"` fix + `Swatinem/rust-cache@v2`).

## Security & sync hardening (F8, F10, store service tokens) — ✅ DONE Nov 28, 2026
### Scope
- ✅ Migration `0016_security_sync_hardening.sql` (both SQLite & Postgres):
  - `sync_outbox.quarantined_at` + `quarantine_reason` (+ `idx_outbox_quarantined`) for edge dead-lettering.
  - `sync_conflicts.resolved_at` + `resolution_note` for HQ conflict triage.
  - `service_tokens` table (SHA-256 hash only, store-scoped `dfs_` bearer tokens for unattended edge → HQ sync).
- ✅ **F8 — Sync dead-letter queue & HQ conflict triage:**
  - `ApplyReport::conflict_ids` populated by `MemoryHq` and `DbHq`; `Edge::sync_now` automatically quarantines rejected conflicts out of `pending()` so poisoned entries stop re-sending forever.
  - `Outbox::mark_failed` / `mark_failed_with_limit` enforces `DEFAULT_MAX_RETRIES = 10` (closing F10's unbounded retry loop), auto-quarantining entries that exceed the cap.
  - Edge dead-letter triage endpoints (`SyncIngest` permission): `GET /pos/sync/dead-letters`, `POST /pos/sync/dead-letters/{id}/requeue`, `POST /pos/sync/dead-letters/{id}/dismiss`, plus `SyncState::dead_letter_count`.
  - HQ conflict triage endpoints (`SyncIngest` staff permission): `GET /api/v1/sync/conflicts` (`?store_id=&include_resolved=`), `POST /api/v1/sync/conflicts/{entry_id}/resolve`.
- ✅ **Store / Edge Service Tokens (`dfs_...`) + background Edge → HQ sync:**
  - `df-auth::service_token` + `df-db::auth` (`create_service_token`, `validate_service_token`, `list_service_tokens`, `revoke_service_token`), 256-bit entropy, SHA-256 hex at rest, constant-time compare.
  - `df-auth-http::gate` adds `AuthRule::AnyOfOrServiceScope` + `SyncIdentity`: `dfs_` tokens are accepted ONLY on `POST /api/v1/sync/ingest` (403 on all other routes) and strictly bound to their single `store_id` (403 on cross-store batch).
  - Admin HTTP API (`UserManage`): `POST /api/v1/auth/service-tokens`, `GET /api/v1/auth/service-tokens`, `POST /api/v1/auth/service-tokens/{id}/revoke` (never exposes `token_hash`).
  - CLI (`df-server tokens create|list|revoke`) and live remote HQ wiring in `df-server edge` (`--hq-url` / `DF_HQ_URL`, `--hq-token-env` / `DF_HQ_TOKEN`, `--sync-interval-secs` / `DF_SYNC_INTERVAL_SECS` background sync timer).
- ✅ **F10 — HTTP hardening (`df-auth-http::hardening` applied to `df-edge`, `df-api`, `df-server hq/edge`):**
  - Exact-origin CORS allowlist (`--cors-origin` / `DF_CORS_ORIGINS`, fail-closed empty default, wildcard `*` rejected at startup).
  - Per-IP auth rate limiting (`20` attempts / `60s` default on `/login`, `/portal-login`, `/counter-switch`, `/override-verify`, `/setup`; returns `429` + `Retry-After` before Argon2id runs; `ConnectInfo<SocketAddr>` wired on all HTTP servers + optional `--trust-proxy-headers`).
  - 4 MiB request body limit (`DefaultBodyLimit`, configurable via `DF_MAX_BODY_BYTES`) and defensive response headers (`X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, `Cache-Control: no-store` on `/auth/*`).
- ✅ Updated `docs/security-review-v1.0.md` (F8 & F10 marked FIXED; F9 reverse-proxy TLS/mTLS guidance), `docs/auth.md`, and `docs/self-hosting.md`.
### Verification
- ✅ `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, float-money guard across all 8 financial crates, and `pci_no_pan_columns` (including migration `0016`) — all clean.
- ✅ GitHub `CI` green on `f2ed8cc` (run `46151548739`, `check` in **2m14s**: **481/481** workspace nextest tests passed, 0 failed, 0 skipped + doc tests + Typst + all 4 frontend builds).
### GitHub
- ✅ Pushed to `main`: `f2ed8cc` (`feat(security,sync): close F8/F10 with dead-letter queue, store service tokens, CORS & auth rate limiting`).

## POS counter completeness (Phase 1 caveats closed) — ✅ DONE Nov 28, 2026
### Scope
- ✅ **DB-backed holds (`0017_pos_holds.sql`, SQLite & Postgres):**
  - Added `pos_holds` table (`id`, `store_id`, `cashier_id`, `payload_json`, `created_at`) and `df-db::transactions` repos (`PosHoldRow`, `insert_pos_hold`, `list_pos_holds`, atomic `take_pos_hold`).
  - Removed the in-memory `holds` map from `df-edge::Edge`; `park_hold` / `park_hold_as` / `list_holds` / `resume_hold` now persist across edge restarts.
- ✅ **Real signed-in cashier attribution:**
  - Added `checkout_as`, `checkout_with_default_provider_as`, `record_return_as`, `record_return_with_provider`, `park_hold_as`, `record_quote_as`, `record_layaway_as`, and `record_layaway_with_default_provider_as` on `Edge`.
  - Wired `Extension<AuthSession>` (`auth.user_id`) in `crates/df-edge/src/http/pos.rs` across checkout, return, hold, quote, and layaway handlers.
  - Removed hardcoded demo cashier UUID fallback in `apps/pos-desktop/src/App.tsx::doOpenTill`.
- ✅ **Card auto-void on failed split-tender / failed DB commit:**
  - Added `PaymentProvider::void` (`df-payments`, implemented on `MockTerminal`, `StandaloneTerminal`, `StripeTerminal`, plus `MockTerminal::voided_refs()` / `refunds()` tracking and `TenderKind::from_str_opt`).
  - `Edge::checkout_as` and `Edge::record_layaway_as` automatically void all previously authorized card references via `PaymentProvider::void` if a later split-tender declines/errors or if the local SQLite write/commit fails.
- ✅ **Card provider refunds on returns & voids:**
  - `Edge::record_return_inner` refunds original card payment references via `PaymentProvider::refund` (capped to each card payment's amount using exact `Decimal` math), persists negative refund `PaymentRow` rows on the return transaction, and returns them in `CompletedSale.payments` + return receipts.
  - `Edge::void_transaction` / `void_transaction_with_provider` voids original card payment references through the configured payment provider before flipping status to `"voided"`.
### Verification
- ✅ `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, float-money guard across all 8 financial crates, and `pci_no_pan_columns` (including migration `0017`) — all clean.
- ✅ GitHub `CI` green on `03e8417` (run `46153292778`, `check` in **2m18s**: **485/485** workspace nextest tests passed, 0 failed, 0 skipped + doc tests + Typst + all 4 frontend builds).
### GitHub
- ✅ Pushed to `main`: `03e8417` (`feat(pos): DB-backed holds, real cashier attribution, and card auto-void/refunds`).

## Admin & Sync UI (`pos-desktop` + edge admin API) — ✅ DONE Nov 28, 2026
### Scope
- ✅ **Backend (`df-db` + `df-edge/src/http/admin.rs`):**
  - Added `df_db::update_store` and `df_db::update_user_profile` (no new migration needed; uses existing `stores`, `users`, `sessions`, and `service_tokens` tables).
  - Added RBAC-gated edge admin routes:
    - `GET /admin/store` (`SyncIngest` — Manager/Admin) & `POST /admin/store` (`SystemAdmin` — Admin only): edit store name, code, country/region/postal, currency, timezone, and warehouse flag, with live active tax/eco-fee preset resolution (`df_domain::tax`) that immediately takes effect on register price previews and checkouts.
    - `GET/POST /admin/users`, `POST /admin/users/{id}`, `POST /admin/users/{id}/revoke-sessions` (`UserManage` — Admin only): create/edit staff & portal accounts, rotate password/PIN/badge hashes (hashes never exposed), auto-revoke active sessions on role change/deactivation, and block self-deactivation/self-demotion (`400`).
    - `GET/POST /admin/service-tokens`, `POST /admin/service-tokens/{id}/revoke` (`UserManage` — Admin only): issue store-scoped `dfs_` sync tokens (raw secret returned once on `201`), list metadata only, and revoke.
  - Added end-to-end integration test suite `crates/df-edge/tests/admin_sync_http.rs` (4 tests covering RBAC, live tax recalculation after store jurisdiction update, user CRUD/guardrails/session revocation, and service token lifecycle) and updated `docs/auth.md`.
- ✅ **Frontend (`apps/pos-desktop`):**
  - Added `System` sidebar group (`Sync` with live dead-letter count badge + `Admin` for Manager/Admin roles) using the existing CSS token/theme system (works across `mono`, `dark`, `expanse`, `lcars`, `tos`, and custom themes).
  - Added `SyncScreen.tsx`: live edge sync health (`pending_count`, `dead_letter_count`, `last_synced_at`, `last_batch_id`), on-demand `Sync now`, and F8 dead-letter queue inspector with `Requeue for retry` and operator-reason `Dismiss`.
  - Added `AdminScreen.tsx` (3 tabs): `Store & tax` (editable store profile + clickable US/CA tax & PaintCare/eco-fee preset preview + immediate TopBar store-name update), `Staff & users` (account table, create/edit forms, password/PIN/badge rotation, session revocation), and `Service tokens` (issue `dfs_` tokens with one-time copyable reveal banner, list, revoke).
### Verification
- ✅ `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, float-money guard across all 8 financial crates, and `apps/pos-desktop` `npm run build` (`476.06 kB` main bundle, lazy `xlsx` chunk `429.52 kB`, zero warnings) — all clean.
- ✅ GitHub `CI` green on `bf88d41` (run `46155448491`, `check` in **2m21s**: **489/489** workspace nextest tests passed, 0 failed, 0 skipped + doc tests + Typst + all 4 frontend builds).
### GitHub
- ✅ Pushed to `main`: `bf88d41` (`feat(admin,sync): add edge admin APIs and pos-desktop Sync & Admin screens`).

## Release v1.2.3 — ✅ PUBLISHED Nov 28, 2026
### Scope
- ✅ Version bump `1.2.2 → 1.2.3` across workspace `Cargo.toml` (+ `Cargo.lock`), `apps/pos-desktop` (`package.json`, `package-lock.json`, `src-tauri/tauri.conf.json`), `df-api` health response, version-asserting tests, and `docs/self-hosting.md` (`f78d9ac`).
- ✅ Includes all work since `v1.2.2`: machine-tinting cleanup (`-3,720` LOC), F8/F10 security & sync hardening (`dfs_` store service tokens, dead-letter queue, HQ conflict triage, CORS allowlist, per-IP auth rate limiting), POS counter completeness (DB-backed `pos_holds`, real cashier IDs, card auto-void & provider refunds), Admin & Sync UI in `pos-desktop` (`SyncScreen`, `AdminScreen`, edge `/admin/*` APIs), and `groundbox-8vcpu` CI + warm-cache Windows release builds.
### Verification
- ✅ GitHub `CI` green on `f78d9ac` (run `46157655870`, `check` in **1m34s** on `groundbox-8vcpu` + all 4 frontend builds).
- ✅ Tagged release `v1.2.3` **PUBLISHED** (run `46157847579`: `build-linux` **3m54s**, `build-windows` **7m35s** warm cache — down from 22m12s, `publish` **13s**) — all 6 assets verified live at https://github.com/kielforeman-collab/decor-pos/releases/tag/v1.2.3:
  - `Decor.POS_1.2.3_x64-setup.exe` (5.9 MB) — Windows NSIS installer
  - `Decor.POS_1.2.3_x64_en-US.msi` (8.3 MB) — Windows MSI installer
  - `df-server-v1.2.3-windows-x86_64.exe` (23.2 MB) — Windows server binary
  - `Decor.POS-1.2.3-1.x86_64.rpm` (9.6 MB) — Linux RPM package
  - `Decor.POS_1.2.3_amd64.AppImage` (87.3 MB) — Linux AppImage
  - `df-server-v1.2.3-linux-x86_64` (28.4 MB) — Linux server binary

## Native Desktop App Parity (embedded loopback `df-edge` HTTP server) — ✅ DONE Nov 28, 2026
### Scope
- ✅ **Backend (`df-auth-http`, `df-edge`, `apps/pos-desktop/src-tauri`):**
  - Updated `df_auth_http::HttpHardeningConfig::validate` to accept `tauri://` exact origins (`tauri://localhost` on Linux/macOS WebKit alongside `http://tauri.localhost` / `https://tauri.localhost` on Windows WebView2 and Vite dev origins; wildcards remain rejected).
  - Added `df_edge::http::router_from_arc(Arc<Edge>, HttpHardeningConfig)` so `PosApp` shares a single `Arc<Edge>` across the embedded HTTP server and native hardware/provider commands (`set_provider`, `last_receipt_text`, `scan_barcode`, `get_status`).
  - Added `PosApp::start_loopback_server()` (binds strictly `127.0.0.1:0`, never exposed on LAN, wires in-process `MemoryHq` fallback when `DF_HQ_URL` is unset), called on native Tauri startup in `src-tauri/src/main.rs`, and exposed via `#[tauri::command] get_api_base_url`.
  - Updated `apps/pos-desktop/src-tauri/tauri.conf.json` CSP with `connect-src 'self' http://127.0.0.1:* ws://127.0.0.1:* http://localhost:* ws://localhost:*`.
- ✅ **Frontend (`apps/pos-desktop/src/`):**
  - Added `apiBase.ts` (`getApiBase()`, `apiUrl(path)`): targets `/api<path>` in Vite browser mode and caches `invoke('get_api_base_url')` (`http://127.0.0.1:<port>`) in the installed Tauri app.
  - Unified `api.ts`, `backofficeApi.ts`, and `auth.ts` onto `apiUrl()` so **all 14 desktop sections** (Transactions, Deliveries, Orders, Transfers, Counts, Products, Customers, CRM, Vendors, Scheduling, Dashboards, Reports, Sync, Admin) run identically inside the Windows & Linux installers with full `Bearer dfp_...` RBAC enforcement, real cashier `user_id` attribution, and auth rate limiting (closing the Phase 7 Tauri IPC RBAC gap). Native hardware/provider commands (`getStatus`, `setProvider`, `lastReceiptText`, `scanBarcode`) stay on `invoke()` sharing the same `Arc<Edge>`.
### Verification
- ✅ `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, float-money guard across all 8 financial crates, `apps/pos-desktop` `npm run build` (`471.80 kB` main bundle, `-4.3 kB` net reduction, lazy `xlsx` chunk `429.52 kB`), and `pos_app_embedded_loopback_http_parity` — all clean.
- ✅ GitHub `CI` green on `f90b585` (run `46160190576`, `check` in **1m30s**: **490/490** workspace nextest tests passed in 31.9s, 0 failed, 0 skipped + doc tests + Typst + all 4 frontend builds).
- ✅ Native Tauri `--features tauri-shell` dry-run (`Release`, `platform=linux`, run `46160201614`): **`build-linux` passed in 2m52s** on `groundbox-8vcpu` (`.rpm` + `.AppImage` + `df-server` built and staged cleanly).
### GitHub
- ✅ Pushed to `main`: `f90b585` (`feat(desktop): embedded loopback df-edge HTTP server for full native Tauri parity`).

## Release v1.2.4 & GitHub Pages refresh — ✅ PUBLISHED Nov 28, 2026
### Scope
- ✅ Refreshed [`docs/index.html`](docs/index.html) (GitHub Pages at https://kielforeman-collab.github.io/decor-pos/) from stale `v1.0` docs to current `v1.2.4` installers, PostgreSQL HQ setup, Argon2id/RBAC/F8/F10 security summary, full env-var reference, and native desktop parity (`3b0c5fa`, `be63050`).
- ✅ Version bump `1.2.3 → 1.2.4` across workspace `Cargo.toml` (+ `Cargo.lock`), `apps/pos-desktop`, `tauri.conf.json`, `df-api`, test assertions, `docs/self-hosting.md`, and `docs/index.html` (`be63050`), shipping **Native Desktop App Parity** (embedded loopback `df-edge` HTTP server + full 14-section RBAC parity) inside the downloadable Windows & Linux installers.
### Verification
- ✅ GitHub `CI` green on `be63050` (run `46161387382`, `check` in **1m30s** + all 4 frontends) and GitHub Pages deployed (`46161386844`, 25s).
- ✅ Tagged release `v1.2.4` **PUBLISHED** (run `46161557459`: `build-linux` **2m54s**, `build-windows` **6m47s** warm cache, `publish` **14s**) — all 6 assets verified live at https://github.com/kielforeman-collab/decor-pos/releases/tag/v1.2.4:
  - `Decor.POS_1.2.4_x64-setup.exe` (7.4 MB) — Windows NSIS installer
  - `Decor.POS_1.2.4_x64_en-US.msi` (10.7 MB) — Windows MSI installer
  - `df-server-v1.2.4-windows-x86_64.exe` (23.2 MB) — Windows server binary
  - `Decor.POS-1.2.4-1.x86_64.rpm` (11.9 MB) — Linux RPM package
  - `Decor.POS_1.2.4_amd64.AppImage` (89.4 MB) — Linux AppImage
  - `df-server-v1.2.4-linux-x86_64` (28.4 MB) — Linux server binary

## Post-v1.2.4 UX & Catalog Completeness — ✅ DONE Nov 28, 2026
### Scope
- ✅ **Products screen split-pane scrolling fix (`c7b619c`):**
  - Bound `.screen-split` detail panes to viewport height with independent `.screen-scroll-pane` list scroll and sticky right-hand detail pane (auto-resetting detail scroll on selection change) so Product detail never scrolls off-screen.
  - GitHub `CI` green on `c7b619c` (run `46163641763`).
- ✅ **Lossless `.xls`/`.xlsx`/`.csv` product catalog import & Product UPC persistence:**
  - Migration `0018_product_catalog_fields.sql` (SQLite & PostgreSQL; expected migration list updated in `df-server`): adds `upc`, `category`, `line_code`, `color_name`, `size_code`, and `source_metadata_json` to `products` plus `idx_products_org_upc (org_id, upc)`.
  - `df-db`: `Product`, `InventoryBalance`, `ImportProductRow`, `insert_product`, and `update_imported_product` (with `COALESCE` so partial re-imports preserve existing catalog/source fields) now persist Product UPC, category, line, color, raw size code, and the full raw spreadsheet row JSON (`source_metadata`). Every imported UPC is also upserted into `product_barcodes` via `upsert_product_barcode`; added `get_product_by_barcode` (`product_barcodes` → `products.upc` → fallback `products.sku`).
  - `df-edge`: `GET /pos/products/search` resolves `?barcode=` via `get_product_by_barcode`, searches `?q=` across SKU/name/UPC/category/line/color/source metadata, and returns all catalog fields + `source_metadata`; `GET /inventory/shelf-tags` now encodes `product.upc` on ZPL labels (falling back to `sku` when absent).
  - `apps/pos-desktop`: `productImport.ts` now captures every non-empty raw column keyed by its original header into `source_metadata`, accepts `Product UPC`/`UPC`/`Barcode` aliases, and avoids scientific-notation truncation on 12-digit numeric `.xls` UPCs; `ProductsScreen.tsx` accepts `.xls,.xlsx,.csv`, filters across UPC/catalog/source columns, displays Product UPC in the inventory & preview tables and detail pane, and renders a collapsible **Source spreadsheet fields** table showing every preserved column verbatim.
### Verification
- ✅ `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, float-money guard across all 8 financial crates, `apps/pos-desktop` `npm run build` (`474.85 kB` main bundle, lazy `xlsx` chunk `429.52 kB`), and targeted suites (`pci_no_pan_columns` 3/3, `sqlite_product_import` 7/7, `df-edge` `product_import` 5/5, `df-server` unit + `migrate_schema` 5/5) — all clean.
- ✅ GitHub `CI` green on `82084da` (run `46167668250`, `check` in **2m35s** — full workspace nextest suite, doc tests, Typst, clippy, fmt, float-money guard + all 4 frontend builds).
### GitHub
- ✅ Pushed to `main`: `c7b619c` (`fix(ui): keep Products detail pane visible with independent scrolling`), `82084da` (`feat(products): persist Product UPC and lossless source spreadsheet fields on import`).

## Release v1.2.5 — ✅ PUBLISHED Nov 28, 2026
### Scope
- ✅ Version bump `1.2.4 → 1.2.5` across workspace `Cargo.toml` (+ `Cargo.lock`), `apps/pos-desktop`, `tauri.conf.json`, `df-api`, test assertions, `docs/self-hosting.md`, and `docs/index.html` (`5d16053`), shipping the Products screen split-pane scroll fix (`c7b619c`) and lossless `.xls`/`.xlsx`/`.csv` import + Product UPC persistence/barcode lookup/ZPL shelf-tag UPC (`82084da`).
### Verification
- ✅ GitHub `CI` green on `5d16053` (run `46168285156`, `check` in **1m39s** + all 4 frontends).
- ✅ Tagged release `v1.2.5` **PUBLISHED** (run `46168470269`: `build-linux` **3m30s**, `build-windows` **8m13s**, `publish` **15s**) — all 6 assets verified live at https://github.com/kielforeman-collab/decor-pos/releases/tag/v1.2.5:
  - `Decor.POS_1.2.5_x64-setup.exe` (7.4 MB) — Windows NSIS installer
  - `Decor.POS_1.2.5_x64_en-US.msi` (10.7 MB) — Windows MSI installer
  - `df-server-v1.2.5-windows-x86_64.exe` (23.4 MB) — Windows server binary
  - `Decor.POS-1.2.5-1.x86_64.rpm` (11.9 MB) — Linux RPM package
  - `Decor.POS_1.2.5_amd64.AppImage` (89.4 MB) — Linux AppImage
  - `df-server-v1.2.5-linux-x86_64` (28.5 MB) — Linux server binary


## Customer Setup (Create & Edit) — ✅ DONE Nov 28, 2026
### Scope
- ✅ **Schema (`0019_customer_setup.sql`, SQLite & PostgreSQL):** adds nullable `address_line1`, `address_line2`, `city`, `province`, `postal_code`, and `notes` columns to `customers`; expected migration lists updated in `df-server` unit & `migrate_schema` tests.
- ✅ **Database (`df-db`):** extended `Customer`, `customer_from_row`, and `insert_customer`, and added full-row `update_customer` in `crates/df-db/src/repos.rs`.
- ✅ **Edge API (`df-edge`):** added `Edge::create_customer` and `Edge::update_customer` (`crates/df-edge/src/customers.rs`) wired to `POST /pos/customers` and `POST /pos/customers/{id}` (`CustomerManage` route gate). Auto-assigns `C-1001`+ when account number is blank, validates uniqueness/email/type/credit limit/terms/price level, extends `GET /pos/customers/search` to match phone numbers, and gates credit-control changes (`charge_enabled`, `credit_limit`, `payment_terms`, `terms_days`, `finance_rate_pct_monthly`, `credit_hold`, `portal_enabled`) to roles holding `ArPayment` (Manager / Admin; returns `403` otherwise).
- ✅ **POS Desktop UI (`apps/pos-desktop`):** added `CustomerFormModal.tsx` and wired **+ New customer** and **Edit customer** into `AccountsScreen.tsx`, displaying full contact, billing address, tax-exempt cert/expiry, PO/job rules, portal status, and account notes, with immediate customer directory listing.
### Verification
- ✅ `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, float-money guard across all 8 financial crates, `df-server` migration tests (5/5), `df-edge` `customer_setup` integration tests + `customers` unit test, `pci_no_pan_columns` (3/3), and `apps/pos-desktop` `npm run build` — all clean.
- ✅ GitHub `CI` green on `5d018b6` (run `46177962622`, `check` in **1m44s** + all 4 frontends).
### GitHub
- ✅ Pushed to `main`: `5d018b6` (`feat(customers): add customer setup create/edit endpoints and POS UI`).
