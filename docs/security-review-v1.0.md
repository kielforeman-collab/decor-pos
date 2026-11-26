# Security / PCI Review — decor-pos v1.0 (Phase 6)

- **Date:** 2026-11-26
- **Scope:** `crates/` (Rust workspace, v0.6.0 → v1.0), `apps/` frontends
  (read-only review), `Dockerfile`, `docker-compose.yml`,
  `deploy/helm/decor-pos/`
- **Method:** source grep + targeted tests (not a penetration test, not
  a live-environment audit). Every claim below cites a file:line or a
  test name. Findings marked **FIXED** were fixed in this pass with a
  test; **OPEN** items are recorded honestly for a later phase.
- **Out of scope / not verified:** see §8.

## Verdict in one paragraph

No card data is stored anywhere, no secrets are hardcoded, and money
is `rust_decimal` throughout. The real exposure in v1.0 is
**authentication**: neither the HQ API nor the edge HTTP API
authenticates callers, and the RBAC model exists but is never
enforced — the system is safe only on a trusted LAN / loopback, which
is how it is shipped by default. Do not expose ports 8080/3000 beyond
a trusted network until F1–F3 are closed. Two real defects found in
this pass (credentials in logs; sync marking unapplied entries
synced) are fixed with tests.

**Phase 7 addendum (2026-11-26):** F1–F4 are now FIXED — the
`df-auth-http` gate enforces `Bearer dfp_...` sessions with
per-route RBAC on both surfaces, portal login replaces the
customer-UUID scope, and the login endpoints are implemented
(see §2 and `docs/auth.md` §6). Ports 8080/3000 are no longer
open to any LAN host, but the loopback/trusted-network posture
still applies as defence in depth: F9 (no TLS/mTLS) and F10 (no
CORS, no rate limiting — including no login throttling) remain
OPEN, so do not expose either listener publicly yet.

## Findings

| # | Severity | Status | Finding |
|---|----------|--------|---------|
| F1 | High | **FIXED** (Phase 7, 2026-11-26) | `df-api` has no authentication — every route is open |
| F2 | High | **FIXED** (Phase 7, 2026-11-26) | Edge HTTP API has no authentication — any LAN host can transact as any cashier |
| F3 | Medium | **FIXED** (Phase 7, 2026-11-26) | RBAC model defined but never enforced |
| F4 | Medium | **FIXED** (Phase 7, 2026-11-26) | Portal scope is the customer id — no separate portal login (known Phase 5 limit) |
| F5 | Medium | **FIXED** | Database URLs (incl. Postgres password) printed to logs |
| F6 | Medium | **FIXED** | `sync_now` marked a whole batch synced even if the sink applied only part of it |
| F7 | Medium | **FIXED** | One conflicting transaction entry blocked a whole store's sync forever |
| F8 | Low | OPEN | Conflicting sync entries stay pending with no dead-letter/quarantine path |
| F9 | Low | OPEN | No TLS/mTLS on the edge↔HQ path (spec §2 promises mTLS) |
| F10 | Low | OPEN | No CORS configuration; no rate limiting on any HTTP API |

## 1. PCI — no PAN/CVV anywhere (PASS)

Spec §8 guardrail 3 requires zero PAN/CVV columns, enforced by a
schema test. Verified by grep **and** by tests (run them with
`cargo test -p df-db --test pci_no_pan_columns` and
`cargo test -p df-payments`):

- **Migrations:** `crates/df-db/tests/pci_no_pan_columns.rs` scans
  all 18 migration files (both dialects) for forbidden identifiers
  (`pan`, `card_number`, `cardnumber`, `cvv`, `cvc`, `card_cvv`,
  `card_expiry`, `cardholder`, `track_data`, `magstripe`,
  `card_security_code`). None present. It also pins the exact column
  sets of the two payment tables:
  - `payments` (`migrations/*/0002_phase1.sql:101`): only `id`,
    `transaction_id`, `tender`, `amount`, `reference`,
    `authorization`, `created_at` — a tender label, a provider
    reference string, an approval code.
  - `portal_payments` (`migrations/*/0010_phase5.sql:10`): only
    `idempotency_key`, `customer_id`, `payment_entry_id`, `amount`,
    `method`, `provider_reference`, `created_at`.
- **Payment types:** `crates/df-payments/src/provider.rs` test
  `payment_types_carry_no_card_data` pins the exact JSON key sets of
  `PaymentRequest` (`amount`, `idempotency_key`, `surcharge`,
  `tender`) and `PaymentResult` (`amount`, `authorization`,
  `reference`, `status`, `surcharge`, `tender`). There is no field
  that could carry card data, so no serialization can leak one.
- **Mock provider** (`crates/df-payments/src/mock.rs`): references
  look like `mock_1`, approvals like `MOCK000001`. No card fields.
- **Stripe Terminal** (`crates/df-payments/src/stripe.rs:14`): the
  skeleton makes no HTTP call; credentials come only from
  `STRIPE_API_KEY` / `STRIPE_TERMINAL_LOCATION_ID` at construction
  (`from_env`, line 43) and the struct deliberately has **no `Debug`
  impl** so the key can never be printed by a derived formatter.
- **Portal bill pay** (`crates/df-portal/src/lib.rs:12`): a bill
  payment carries an amount, a method label (`card`/`ach`) and an
  idempotency key only; the card itself is authorized by the
  configured provider, never stored.
- **BOPIS / storefront:** no card fields in `df-portal` or the
  storefront/portal frontends (grep `card|pan|cvv` finds only the
  method label and UI copy).

## 2. Authentication & authorization

### F1 — FIXED in Phase 7 (High): `df-api` has no authentication

`crates/df-api/src/lib.rs` builds its router with no auth middleware
or layer (grep `middleware|layer` finds only state extraction). All
five auth routes return **501** (`lib.rs:918-922`, test
`auth_routes_are_501_in_phase_0`), and the read endpoints added in
Phases 2–4 (`GET /api/v1/crm/activities`, `/crm/appointments`,
`/deliveries/dispatch`, `/edi/documents`) are plain JSON reads with no
caller check. Anyone who can reach the listener can read CRM,
delivery and EDI data. **Mitigation in v1.0:** `df-api` binds
`127.0.0.1:3000` by default (`crates/df-api/src/main.rs`); the new
`--listen` flag (added in this pass for Docker) must not be pointed
at a public interface until auth lands. The compose file and Helm
chart keep the API on ClusterIP / host-mapped ports for local use
only, and both say so in comments.

**Fixed 2026-11-26 (Phase 7, v1.1):** `create_router` applies the
`df-auth-http` gate (`crates/df-auth-http/src/lib.rs`) per route
group via `route_layer`. Every route except `GET /health`, `GET
/api-docs/openapi.json` and the auth login endpoints requires a
`Bearer dfp_...` session validated against the `sessions` table
(unknown/expired/revoked/inactive → 401). The login endpoints are
implemented over `df-db::auth` (`POST /api/v1/auth/login`,
`/auth/counter-switch`, `/auth/logout`, `/auth/override-verify`;
passkey/OIDC stay honest 501 stubs). The OpenAPI document stays
public but is verified by test to contain no data rows
(`hq_unauthenticated_is_401_health_and_openapi_public`,
`openapi_document_is_3_1`). Full matrix: `docs/auth.md` §6.

### F2 — FIXED in Phase 7 (High): edge HTTP API has no authentication

`crates/df-edge/src/http.rs` has no auth check on any route:
`POST /pos/transactions` (checkout), `/pos/tills/open`,
`/pos/transactions/{id}/void`, `/portal/payments`, `/pos/sync/run`,
etc. `cashier_id` is caller-supplied (`http.rs:186`) with no session
behind it — any host on the store LAN can sell, void, open tills and
trigger sync as any cashier. This matches the known Phase 1 limit
("POS cashier sign-in not wired") but was never written down as a
security boundary: **the edge API trusts the LAN**. Deployments must
keep the edge on an isolated store network/VLAN, never on a guest or
public network.

**Fixed 2026-11-26 (Phase 7, v1.1):** the same `df-auth-http`
gate is applied to every edge route except `GET /health`, the
`POST /pos/auth/*` login endpoints, and the public BOPIS
storefront (`GET /catalog/products`, `POST /bopis/orders`, `GET
/bopis/orders/{id}`). `cashier_id` in the open-till body is now
ignored — the till's cashier is the session's user. Login/logout
live on the edge itself (`POST /pos/auth/login`,
`/counter-switch`, `/portal-login`, `/logout`,
`/override-verify`) so the POS frontend needs no HQ API.
Tests: `crates/df-edge/tests/phase7_http_auth.rs`
(`unauthenticated_protected_edge_routes_are_401`,
`expired_token_is_401_and_logout_revokes`). The LAN-trust note
above still applies as defence in depth (F9/F10 remain open),
but an unauthenticated LAN host can no longer transact.

### F3 — FIXED in Phase 7 (Medium): RBAC defined, never enforced

`crates/df-auth/src/rbac.rs` defines roles (`Cashier`,
`SalesAssociate`, `Warehouse`, `Driver`, `Manager`, `Admin`,
`PortalContractor`) and permissions with tests — but no call site in
`df-edge`, `df-api` or `df-server` invokes `has_permission` or any
authorize function (grep across all three crates: no matches). The
model is ready; the enforcement wiring is a post-v1.0 item. Do not
treat "role exists in the code" as "access is controlled".

**Fixed 2026-11-26 (Phase 7, v1.1):** every route group on both
surfaces carries an `AuthRule::AnyOf(...)` gate built from
`Role::has_permission` (`crates/df-auth-http/src/lib.rs`, groups
in `df-edge/src/http.rs` `router()` and
`df-api/src/lib.rs` `create_router`). A valid token with the
wrong role gets 403; no token gets 401, never 403. Full
role→route matrix: `docs/auth.md` §6. Tests:
`cashier_forbidden_manager_allowed_on_reorder_points` (edge),
`hq_cashier_forbidden_manager_allowed_on_colorant_levels` (HQ).
Two follow-on gaps found during Phase 7 verification are also
fixed in Phase 7: scheduling routes now carry real permissions
(`SchedulingView` / `SchedulingManage` in `rbac.rs`, read vs
manage groups in `df-edge/src/http.rs`), and `PortalContractor`
holds no staff permissions at all (its `ArView` was removed, so
a contractor session cannot read staff AR routes). Tests:
`portal_contractor_holds_no_staff_permissions`,
`scheduling_permissions` (`df-auth/src/rbac.rs`).

### F4 — FIXED in Phase 7 (Medium): portal scope is the customer id

`crates/df-edge/src/portal.rs:109-116` gates every portal call on
`portal_enabled`, and `portal_reorder` additionally checks the
formula belongs to the caller (`portal.rs:161+`). But there is no
portal login: anyone who knows (or guesses) a contractor's customer
UUID can view their invoices, statements, formulas and pay their
bills (known Phase 5 limit, restated here). UUIDs are unguessable in
practice but they are bearer tokens, not credentials — do not put
portal URLs in logs or shared links.

**Fixed 2026-11-26 (Phase 7, v1.1):** portal routes now require
a `PortalContractor` session (`POST /pos/auth/portal-login`,
`df_db::login_portal`, session carries the linked
`customer_id`), and every portal handler additionally checks the
session's customer equals the requested customer — another
contractor's id returns 403, never 404. `portal_enabled`
remains a second gate. The public BOPIS storefront stays
unauthenticated by design (catalog + order placement/status
only); counter pickup completion requires staff auth
(`PosCheckout`). Test:
`portal_login_scopes_contractor_to_own_customer`
(`crates/df-edge/tests/phase7_http_auth.rs`). The remaining gap
noted here during the build — `PortalContractor` holding `ArView`
in `rbac.rs`, which let a contractor read unscoped staff AR
routes — is fixed in Phase 7 verification: the role now holds no
staff permissions (`docs/auth.md` §9, test
`portal_contractor_holds_no_staff_permissions`).

### What is good here

- Passwords: Argon2id, PHC strings, default parameters
  (m=19 MiB, t=2, p=1), minimum 8 characters
  (`crates/df-auth/src/password.rs:23`).
- PINs: 4–6 ASCII digits, stored only as Argon2id hashes
  (`crates/df-auth/src/pin.rs:20`); badge tokens are random
  32-character strings, stored only as hashes (`pin.rs:54`).
- WebAuthn/OIDC are honest stubs returning `NotImplemented`
  (`df-auth/src/webauthn.rs:28`, `oidc.rs`) — no fake auth path.
- Portal gating itself is correct and complete: every `portal_*`
  method funnels through `portal_customer`
  (`df-edge/src/portal.rs:111-160`), so disabling
  `portal_enabled` locks the contractor out immediately.

## 3. Secrets handling

### F5 — FIXED (Medium): database credentials printed to logs

`df-api` printed the raw `--db` URL on connect and on connect failure
(`crates/df-api/src/main.rs:51,54`), `df-server` did the same in
`open_pool` (`crates/df-server/src/main.rs:390`), and `Edge`'s
manual `Debug` impl printed the raw `db_url`
(`crates/df-edge/src/edge.rs`). A PostgreSQL URL is
`postgres://user:password@host/db` — the password went to
stdout/stderr (and therefore to Docker/K8s logs) on every start.

**Fix:** `df_db::redact_db_url` (`crates/df-db/src/lib.rs`), which
replaces the userinfo with `***` (`postgres://***@host/db`),
preserving scheme/host/path. All three print sites now use it, and
`df-db` has a unit test (`redact_db_url_hides_postgres_password`)
covering passwords containing `@`, password-less URLs, and SQLite
URLs (unchanged). SQLite URLs never carried credentials; the fix
matters for the Postgres HQ path.

### Rest of secrets handling (PASS)

- Stripe credentials: env-only (see §1); never logged, never written
  to disk (`stripe.rs:20`).
- No hardcoded secrets/keys/tokens in the tree (grep for
  `sk_live|pk_live|password = "|secret = "|api_key = "|AKIA…`:
  no matches outside tests/examples).
- No `.env` files in the repo; `.gitignore` excludes `.env`.
  `.env.example` ships only placeholders.
- Docker image: no secrets baked in; credentials arrive as
  environment variables at runtime; the runtime user is unprivileged
  (uid 10001).
- No `Debug` derive on any struct holding a credential
  (`StripeTerminal` is the only credential holder; see §1).

## 4. Sync integrity (chaos findings)

Two defects were found by the Phase 6 chaos tests and fixed in this
pass; the tests that prove them live in
`crates/df-sync/tests/chaos_sync.rs` and
`crates/df-edge/tests/chaos_sync_edge.rs`.

### F6 — FIXED (Medium): partial applies were silently marked synced

`Edge::sync_now` (`crates/df-edge/src/edge.rs`, before this pass)
marked **every** id in the batch synced whenever the sink returned
`Ok` — even if the sink had applied only a subset (e.g. cut off
mid-batch). The unapplied entries would never be redelivered: silent
data loss. **Fix:** `ApplyReport` now carries `applied_ids`
(`crates/df-sync/src/sink.rs`) and `sync_now` marks only those ids
synced. Proven by `partial_apply_leaves_unapplied_entries_pending`
(6 deltas, sink applies 3 → 3 stay pending → second sync completes
to exactly −7).

### F7 — FIXED (Medium): one conflicting entry blocked a store's sync

`MemoryHq` pre-validated the whole batch and returned `Err` on the
first conflicting transaction re-send, so the edge marked the entire
batch failed — and the poisoned entry was in every subsequent drain,
so **no entry from that store ever synced again**. **Fix:** conflicts
are now handled per entry (`sink.rs`): the conflicting entry is
skipped (counted in `ApplyReport::conflicts`, recorded once in
`MemoryHq::conflicts`), the rest of the batch applies, and the
stored transaction is never overwritten. Proven by
`conflicting_edits_from_two_edges_do_not_block_sync` and
`outbox_conflicting_entry_stays_pending_without_blocking`.

### F8 — OPEN (Low): no dead-letter path for conflicting entries

A skipped conflicting entry stays pending in the outbox forever and
is re-skipped on every sync (proven by the test above). It cannot
block anything anymore, but it never resolves itself either, and
`MemoryHq`'s conflict list is in-memory (lost on restart). A real
HQ sink needs a persisted conflicts table and an operator workflow
(void + re-issue as a new transaction); that is post-v1.0 work.

### F9 — OPEN (Low): no TLS/mTLS on the sync path

Spec §2 shows edge↔HQ as "mTLS + Delta Sync". In v1.0 there is no
network sync transport at all — the only sink is the in-process
`MemoryHq`. When a real HQ sink lands, it must authenticate batches
(F1's API has none) and use mTLS; until then the exposure is nil
because nothing crosses a network.

### F10 — OPEN (Low): no CORS, no rate limiting

No `CorsLayer` is configured anywhere (browsers calling the edge
from another origin are blocked by default — safe but incidental),
and no endpoint has rate limiting or request-size limits. The
`mark_failed` retry counter is unbounded (a permanently failing entry
retries forever). Acceptable on a trusted LAN; revisit before any
public exposure.

## 5. Input handling & money

- **SQL injection:** all queries are parameterized (`sqlx` `?`
  binds). The single `format!` that builds SQL
  (`crates/df-sync/src/outbox.rs:231`, `mark_synced`) interpolates
  only a run of `?` placeholders whose count matches the bound ids —
  no user input reaches the SQL string.
- **Floating-point money:** grep for `f32|f64` across
  `crates/` and `apps/pos-desktop/src-tauri/src` (excluding test
  code) finds **nothing**. Money is `rust_decimal::Decimal` /
  `NUMERIC(14,4)` throughout (spec §8 guardrail 1).
- **Outbox atomicity:** mutations and their outbox rows commit in
  one transaction (`Outbox::enqueue_exec`, tested by
  `enqueue_inside_a_transaction_is_atomic`).

## 6. Dependencies

- `cargo audit` **could not be run**: no `cargo-audit` binary exists
  in this VM and there is no network access to install it (the
  registry cache contains no audit crate). This must run in CI with
  network access before any release tag — it is the one check in
  this review that is not actually done.
- `cargo tree` (offline, from the pinned `Cargo.lock`) records the
  versions in play: `argon2 0.5.3`, `axum 0.8.9`, `sqlx 0.8.6`,
  `tokio 1.54.1`, `rust_decimal 1.43.0`, `uuid 1.29.3`,
  `serde 1.0.229`, `serde_json 1.0.154`, `chrono 0.4.45`,
  `clap 4.6.10`, `utoipa 5.5.0`, `tower-http 0.6.11`,
  `base64 0.21.7`, `sha2 0.10.9`, `rand 0.9.5` (plus `rand 0.8.8`
  pulled in transitively — two major versions, both current
  patch lines), `proptest 1.12.0`. No dependency is obviously
  abandoned or pre-1.0 in a risky spot; nothing here substitutes for
  `cargo audit`.
- The workspace builds with `SQLX_OFFLINE=true` and no live
  database (`.cargo/config.toml`).

## 7. Deployment posture (Docker/Helm)

- Runtime image runs as uid 10001, no shell tools beyond `curl`
  (healthchecks) and `ca-certificates`.
- Compose binds Postgres to `127.0.0.1:5432` only; edge/API ports
  8080/3000 are host-mapped for local use. **Do not map them on a
  public host** (F1/F2).
- Helm: both Services are `ClusterIP`; no Ingress is defined. The
  chart's `NOTES.txt` repeats the no-auth warning.
- Postgres in compose/Helm is provisioned but unused in v1.0; its
  password must be changed from the example value before any use.

## 8. What was NOT verified

- **PostgreSQL round-trip** — no PG server in this VM (standing
  caveat since Phase 0). The Postgres migrations are embedded and
  compile-checked; a live round-trip is still untested.
- **Live Stripe Terminal** — the provider is a skeleton; no live
  payment call was made or observed.
- **TLS/mTLS, OIDC, WebAuthn** — not implemented, not verified.
- **Plugin HTTP transport trust boundary** — `df-plugin-sdk`
  spawns local plugin processes with no auth headers; the trust
  boundary is "local machine". A malicious local plugin is out of
  scope for this review.
- **Frontends** — the React apps were grep-reviewed for card fields
  only. The mobile PWA keeps its offline outbox in `localStorage`,
  which includes sale/customer data on the device; that storage was
  not audited in depth (shared-device risk is noted, not verified).
- **Dependency vulnerabilities** — `cargo audit` not run (§6).
