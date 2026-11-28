# Authentication — decor-pos v1.1 (Phase 7, auth core)

- **Status:** auth **core** (primitives + login services + admin
  bootstrap) plus **HTTP enforcement** (Phase 7, landed
  2026-11-26): the `df-auth-http` gate middleware enforces a
  `Bearer dfp_...` session on every `df-api` / `df-edge` route
  except the public list in §6, with per-route RBAC from
  `Role::has_permission`. F1–F4 are FIXED at the HTTP layer — see
  `docs/security-review-v1.0.md`. The frontend login screens are
  the third Phase 7 worker's job and are not in this document.
- **Spec:** architectural specification §1 (local auth), §5.1
  (`users`), §6.1 (auth endpoints), §9 (COLORx deferred — no COLORx
  code anywhere in Phase 7).
- **Findings addressed:** F1/F2 get the primitives (sessions,
  per-request validation); F3 (RBAC model, now with a real role-id
  mapping) gets enforced by the HTTP worker; F4 (portal scope is the
  customer UUID) is closed at the login layer — portal access now
  requires a `PortalContractor` login scoped to one linked customer.

## 1. Token design

Sessions are **opaque bearer tokens**, not JWTs: no signature to
verify, no claims to parse, nothing to revoke except a database row.

| Item | Value |
|---|---|
| Presented token | `dfp_` + base64url (no padding) of 32 random bytes (256 bits, `rand`) |
| Stored | SHA-256 hex of the token only (`sessions.token_hash`, `UNIQUE`) |
| Raw token | Returned to the caller **once**, at login; never persisted, never logged, not recoverable from the hash |
| Hash comparison | Constant-time XOR fold (`df_auth::hashes_equal`) |
| Why SHA-256, not Argon2id | A 256-bit random token needs no stretching, and validation runs on **every request** — Argon2id there would be a self-inflicted DoS. Passwords/PINs/badges stay Argon2id (see §4) |

`Session` (`df-auth/src/session.rs`) carries the token hash only —
there is no field that could hold a raw token, so a session row can
be logged and serialized safely. The row also denormalizes the
user's `role` (as text) and `customer_id` (portal scope) from login
time, so the HTTP layer can authorize from the session row alone.
If a user's role changes, existing sessions keep the old role until
they expire or are revoked — revoke on role change
(`df_db::revoke_sessions_for_user`).

## 2. TTLs

| Session | TTL | Rule |
|---|---|---|
| Staff (any role, incl. PIN/badge counter-switch) | **8 hours** | One till shift. No silent refresh — log in again after 8 h. |
| Portal, no remember-me | **8 hours** | Same as staff. |
| Portal, remember-me | **30 days** | Only when the caller **explicitly** requests remember-me at login (`df_auth::session_ttl`). |

Constants: `df_auth::{STAFF_SESSION_TTL, PORTAL_SESSION_TTL,
PORTAL_REMEMBER_ME_TTL}`.

## 3. Roles and `role_id`

`users.role_id` (spec §5.1) is a UUID, but Phases 0–6 shipped no
`roles` table and no code ever wrote a `role_id`. Phase 7 defines
the mapping in `df-auth/src/rbac.rs` (`role_id(Role)`,
`role_from_id(Uuid)`) with fixed well-known UUIDs, and migration
`0012_phase7` seeds a `roles` table with exactly those ids:

| Role | `role_id` |
|---|---|
| Cashier | `10000000-0000-4000-8000-000000000001` |
| SalesAssociate | `10000000-0000-4000-8000-000000000002` |
| Warehouse | `10000000-0000-4000-8000-000000000003` |
| Driver | `10000000-0000-4000-8000-000000000004` |
| Manager | `10000000-0000-4000-8000-000000000005` |
| Admin | `10000000-0000-4000-8000-000000000006` |
| PortalContractor | `10000000-0000-4000-8000-000000000007` |

Deliberately **no FK** on `users.role_id`: a deployment that
hand-inserted a user with an arbitrary `role_id` must keep
migrating, and the app rejects an unmapped id at read time
(`DbError::InvalidData`) instead of defaulting to a role. Role
names (`Role::as_str` / `FromStr`) are the text cached in
`sessions.role` and accepted by `df-server users create --role`.

## 4. Credentials

- **Passwords:** Argon2id PHC strings, default parameters
  (m=19 MiB, t=2, p=1), minimum 8 characters
  (`df_auth::{hash_password, verify_password}`). Nullable in the
  schema (OIDC-only users have none).
- **Counter PINs:** 4–6 ASCII digits, Argon2id
  (`df_auth::{hash_pin, verify_pin}`).
- **Badges:** random 32-char tokens, Argon2id
  (`df_auth::{generate_badge_token, hash_badge_token,
  verify_badge_token}`).
- **WebAuthn / OIDC:** honest `NotImplemented` stubs (unchanged
  from Phase 0).

PIN/badge logins identify the user **by email**: Argon2id hashes
cannot be searched, so a bare PIN or badge token cannot name its
user. The counter UI selects the cashier (or re-uses the previous
session's user); the PIN/badge proves it is them. A badge-only
lookup would need an extra non-secret badge identifier column —
open follow-up, not faked by scanning every user's hash.

## 5. Login services — exact signatures

Pure verification lives in `df-auth/src/login.rs` (takes an
`AuthUser` value, touches no database). DB-backed orchestration
(load → verify → issue session → persist) lives in
`df-db/src/auth.rs`. The HTTP workers call the `df-db` functions.

```rust
// df-auth — pure, no database
df_auth::login::verify_password_login(user: &AuthUser, password: &str) -> Result<(), AuthError>
df_auth::login::verify_pin_login(user: &AuthUser, pin: &str) -> Result<(), AuthError>
df_auth::login::verify_badge_login(user: &AuthUser, badge_token: &str) -> Result<(), AuthError>
df_auth::login::verify_portal_login(user: &AuthUser, password: &str) -> Result<(), AuthError>
df_auth::login::verify_manager_override(
    approver: &AuthUser,
    credential: &OverrideCredential,   // Password(String) | Pin(String)
    now: DateTime<Utc>,
) -> Result<OverrideApproval, AuthError>  // approver_user_id, approver_role, method, approved_at

// df-auth — sessions (pure)
df_auth::generate_token() -> String                    // "dfp_<base64url>"
df_auth::hash_token(token: &str) -> String             // SHA-256 hex
df_auth::hashes_equal(a: &str, b: &str) -> bool        // constant-time
df_auth::new_session(
    user_id: Uuid, role: Role, customer_id: Option<Uuid>,
    ttl: Duration, now: DateTime<Utc>,
) -> (String, Session)                                // (presented token, row)
df_auth::validate_session_token(
    token: &str, session: &Session, user_active: bool, now: DateTime<Utc>,
) -> Result<(), AuthError>
df_auth::session_ttl(role: Role, remember_me: bool) -> Duration

// df-db — repositories (DbError)
df_db::create_user(pool: &AnyPool, new: &NewUser) -> Result<UserRow, DbError>
df_db::get_user_by_email(pool: &AnyPool, email: &str) -> Result<UserRow, DbError>  // in repos.rs
df_db::get_user_by_id(pool: &AnyPool, id: Uuid) -> Result<UserRow, DbError>
df_db::list_users_for_org(pool: &AnyPool, org_id: Uuid) -> Result<Vec<UserRow>, DbError>
df_db::count_users_with_role(pool: &AnyPool, org_id: Uuid, role: Role) -> Result<i64, DbError>
df_db::set_password_hash(pool: &AnyPool, user_id: Uuid, hash: Option<&str>) -> Result<(), DbError>
df_db::set_pin_hash(pool: &AnyPool, user_id: Uuid, hash: Option<&str>) -> Result<(), DbError>
df_db::set_badge_hash(pool: &AnyPool, user_id: Uuid, hash: Option<&str>) -> Result<(), DbError>
df_db::set_portal_customer(pool: &AnyPool, user_id: Uuid, customer_id: Option<Uuid>) -> Result<(), DbError>
df_db::set_user_active(pool: &AnyPool, user_id: Uuid, active: bool) -> Result<(), DbError>
df_db::insert_session(pool: &AnyPool, session: &Session) -> Result<(), DbError>
df_db::get_session_by_token_hash(pool: &AnyPool, token_hash: &str) -> Result<Session, DbError>
df_db::revoke_session(pool: &AnyPool, session_id: Uuid) -> Result<(), DbError>
df_db::revoke_session_by_token_hash(pool: &AnyPool, token_hash: &str) -> Result<(), DbError>
df_db::revoke_sessions_for_user(pool: &AnyPool, user_id: Uuid) -> Result<u64, DbError>
df_db::list_sessions_for_user(pool: &AnyPool, user_id: Uuid) -> Result<Vec<Session>, DbError>

// df-db — login services (AuthError)
df_db::login_with_password(pool, email: &str, password: &str, remember_me: bool) -> Result<LoginOutcome, AuthError>
df_db::login_with_pin(pool, email: &str, pin: &str) -> Result<LoginOutcome, AuthError>
df_db::login_with_badge(pool, email: &str, badge_token: &str) -> Result<LoginOutcome, AuthError>
df_db::login_portal(pool, email: &str, password: &str, remember_me: bool) -> Result<LoginOutcome, AuthError>
df_db::verify_manager_override(pool, email: &str, credential: &OverrideCredential) -> Result<OverrideApproval, AuthError>
df_db::create_session(pool, user: &UserRow, ttl: Duration, customer_id: Option<Uuid>) -> Result<(String, Session), AuthError>
df_db::validate_session_token(pool, token: &str) -> Result<Session, AuthError>
```

`LoginOutcome { user: UserRow, session: Session, token: String }`
— the token is returned once; the HTTP layer puts it in an
`Authorization: Bearer` response / secure cookie and never logs it.

### Error discipline

An unknown email, a wrong password, a missing password hash, and a
wrong-role portal attempt all return `AuthError::InvalidCredentials`
("invalid credentials") — the login boundary cannot be used to
enumerate emails. An inactive account is `AuthError::AccountInactive`
and an unknown/expired/revoked session is
`AuthError::InvalidSession`; the HTTP layer maps both to a generic
401. A `PortalContractor` user with no linked customer is the
distinct `AuthError::PortalNotLinked` (a config error — fix the
link, don't retry the login). A non-manager attempting an override
is `AuthError::PermissionDenied`.

### Portal login and the customer link (F4)

The portal link lives on the user: `users.portal_customer_id`
(migration `0012_phase7`, `NULL` for staff; PostgreSQL enforces the
reference to `customers(id)`, SQLite deliberately omits the FK per
the edge convention). One portal user is scoped to exactly one
contractor customer; a customer row predates any portal user and
must not name one. `login_portal` requires role
`PortalContractor` **and** a linked customer, and the issued
session carries `customer_id` = that link — the portal handlers
authorize against the session's customer, never a caller-supplied
customer id. (`customers.portal_enabled` still gates the customer
side, as in Phase 5.)

## 6. HTTP enforcement — the gate, the matrix, the exceptions

Enforcement lives in the shared crate `df-auth-http`
(`crates/df-auth-http/`), so both HTTP surfaces run the same
checks. The gate is Axum middleware applied per route group with
`Router::route_layer` + `middleware::from_fn_with_state`, carrying
a `Gate { pool, rule }` state:

```rust
// df-auth-http/src/lib.rs
pub enum AuthRule {
    Authenticated,            // any live session, any role
    AnyOf(Vec<Permission>),   // session role must hold one of these
    Portal,                   // PortalContractor role only
}
pub struct AuthSession {      // inserted into request extensions
    pub user_id: Uuid,        // on success; handlers read it with
    pub session_id: Uuid,     // Extension<AuthSession>
    pub role: Role,
    pub customer_id: Option<Uuid>,  // portal scope
}
```

Per request, in order:

1. **No database configured** (`df-api` without `--db`) → 503
   `service unavailable` — the gate cannot validate without a
   pool, so it refuses before looking at the token. (The edge
   always has a pool, so this never fires there.)
2. **No/invalid/expired/revoked token, or the user is inactive**
   → 401 `{"error": "invalid or expired session — log in again"}`.
   The message is deliberately generic: it does not say which of
   those fired. A database failure during validation → 500
   `{"error": "internal error"}`, never the DB text.
3. **Valid token, wrong role** → 403 `{"error": "role <Role> may
   not ..."}` naming the role.
4. On success the handler receives `Extension<AuthSession>` and
   authorizes/acts as that session.

Bearer parsing is strict: `Authorization: Bearer <token>` only.
The one exception is WebSocket paths (`/ws/...`): browsers cannot
set headers on the WS handshake, so `?token=<token>` is accepted
there and only there.

### Login endpoints (both surfaces)

| Endpoint | Body | Result |
|---|---|---|
| `POST /api/v1/auth/login` (HQ) / `POST /pos/auth/login` (edge) | `{email, password, remember_me?}` | `LoginResponse {token, user_id, email, display_name, role, customer_id?, expires_at}` — token is `dfp_...` |
| `POST /api/v1/auth/counter-switch` / `POST /pos/auth/counter-switch` | `{email, pin?, badge_token?}` | Same shape; `email` is **required** — Argon2id hashes cannot be searched, so a bare PIN/badge cannot name a user (see §4) |
| `POST /pos/auth/portal-login` (edge only) | `{email, password, remember_me?}` | Same shape; requires `PortalContractor` + a linked customer (`PortalNotLinked` → 403); `customer_id` is the link |
| `POST /api/v1/auth/logout` / `POST /pos/auth/logout` | `{}` | `{revoked: true}`; gated `Authenticated` — an expired token cannot log out (the gate 401s first) |
| `POST /api/v1/auth/override-verify` / `POST /pos/auth/override-verify` | `{user_id, pin, reason_code}` | `{approved, approver_user_id, approver_role, method, approved_at}`; approver must be Manager/Admin; empty `reason_code` → 400; unknown approver id → generic 401 |
| `POST /api/v1/auth/passkey/verify`, `GET /api/v1/auth/oidc/callback` | — | Honest 501 stubs (unchanged from Phase 0) |

Error mapping at the login boundary: unknown email / wrong
password / missing hash / inactive account → 401 `invalid
credentials` (no enumeration); portal user with no linked
customer → 403 `portal user has no linked customer`;
non-manager override attempt → 403. A wrong PIN on
counter-switch is a 401, not a lockout — there is no rate
limiting yet (see §9).

### Role → route matrix (edge, `df-edge/src/http.rs`)

Permissions are `df_auth::Permission`; a role holds a permission
iff `Role::has_permission` says so (`df-auth/src/rbac.rs` is the
source of truth — the table below is a copy of it, not a second
definition). The route tables list the **permission** a group
requires; which roles that admits follows from this table.

| Role | Permissions held |
|---|---|
| Cashier | `PosCheckout`, `PosTillManage`, `CustomerManage`, `SchedulingView` |
| SalesAssociate | `PosCheckout`, `PosTillManage`, `CustomerManage`, `InventoryView`, `ArView`, `SchedulingView` |
| Warehouse | `InventoryView`, `InventoryAdjust`, `PoReceive`, `SchedulingView` |
| Driver | `InventoryView`, `SchedulingView` |
| Manager | `PosCheckout`, `PosVoid`, `PosPriceOverride`, `PosTillManage`, `InventoryView`, `InventoryAdjust`, `PoCreate`, `PoReceive`, `ArView`, `ArPayment`, `ArStatementRun`, `CustomerManage`, `PricingManage`, `ReportView`, `SchedulingView`, `SchedulingManage` |
| Admin | Manager's set + `UserManage`, `SystemAdmin` |
| PortalContractor | *(none)* — portal access is by role, not permission |

Note the consequences: Cashier holds `CustomerManage`, so
cashiers may use `/crm/*`; Driver holds only `InventoryView`
(plus `SchedulingView`), which admits it to the delivery group
(all `/deliveries*` routes share one `PosCheckout`-or-`InventoryView`
gate, so a driver can create, assign, and POD a delivery) and to
inventory views — but not to checkout, tills, voids, CRM, reports,
or pricing; and PortalContractor holds no staff permission at all,
so a contractor session cannot reach any staff route (Phase 7
coordinator fix — it previously held `ArView`, which leaked other
customers' AR summaries through the unscoped staff AR routes).

| Route group (edge) | Requires | Notes |
|---|---|---|
| `GET /health` | public | Liveness only |
| `POST /pos/auth/login`, `/counter-switch`, `/portal-login`, `/override-verify` | public | How a session is obtained |
| `POST /pos/auth/logout` | any live session | Revokes the caller's session |
| `POST /pos/carts/price-preview`, `/pos/transactions`, `/pos/transactions/{id}/return`, `/pos/holds*`, `/pos/quotes`, `/pos/layaways`, `/pos/sync/*`, `GET/POST /pos/tinting/formulas`, `GET /pos/tinting/colors`, `POST /bopis/orders/{id}/complete-pickup` | `PosCheckout` | Checkout surface; BOPIS pickup completion needs staff auth even though order placement is public |
| `POST /pos/tills/open`, `/pos/tills/{id}/close` | `PosTillManage` | |
| `POST /pos/transactions/{id}/void` | `PosVoid` | Manager/Admin only |
| `GET /pos/tinting/colorants` | `PosCheckout` **or** `InventoryView` | Org colorant catalog (formula tracking) |
| `GET /pos/reports/daily-usage`, `/pos/reports/low-stock`, `/bi/kpi`, `/bi/dashboard`, `/bi/eco`, `/bi/inquiry`, `/bi/email-outbox` | `ReportView` | |
| `POST /pos/reports/low-stock/thresholds`, `/pricing/rules` (GET+POST) | `PricingManage` | |
| `POST /pos/inventory/import` | `PricingManage` | GUI product import (Manager/Admin only — Warehouse holds `InventoryAdjust` but not this); rows are parsed in the browser, posted as JSON, upserted by SKU |
| `GET /inventory/balances`, `/inventory/reorder-points`, `/inventory/transfers`, `/inventory/counts`, `/inventory/shelf-tags`, `/purchasing/orders`, `/vendors`, `/vendors/{id}` | `InventoryView` | |
| `POST /inventory/reorder-points`, `/inventory/transfers*`, `/inventory/counts*` | `InventoryAdjust` | Includes transfer ship/receive, count scan/reconcile |
| `POST /purchasing/orders`, `/vendors`, `/vendors/{id}` | `PoCreate` | Vendor create/update share the purchasing gate; `name` is the PO key and is never editable |
| `POST /purchasing/orders/{id}/receive` | `PoReceive` | |
| `POST /purchasing/replenishment/suggest` | `PoCreate` **or** `InventoryView` | |
| `GET /ar/customers/{id}/summary`, `/ar/aging`, `/ar/collections` | `ArView` | |
| `/crm/*` | `CustomerManage` | Activities, appointments, reminders, saved views, lapsed report |
| `/deliveries*` | `PosCheckout` **or** `InventoryView` | Create/dispatch/assign/status/packing-slip/POD |
| `/edi/*` | `ReportView` | Send, intake, documents, acks, three-way match, price applications, transport, catalog import — all read/write under one permission; no separate vendor-config permission exists yet |
| `/portal/*` | `PortalContractor` + customer scope (see below) | |
| `GET /mobile/kpis` | `PosCheckout` **or** `InventoryView` **or** `ArView` | |
| `GET /scheduling/templates`, `/scheduling/shifts`, `/scheduling/timeclock`, `/scheduling/forecast`, `/scheduling/coverage` | `SchedulingView` | Reads + timeclock clock-in/out (every staff role) |
| `POST /scheduling/templates`, `/scheduling/shifts`, `/scheduling/shifts/{id}/status` | `SchedulingManage` | Manager/Admin only |
| `GET /ws/v1/store/{store_id}` | any live session | `?token=` accepted (browsers cannot set WS headers) |

### Role → route matrix (HQ API, `df-api/src/lib.rs`)

| Route group (HQ) | Requires | Notes |
|---|---|---|
| `GET /health`, `GET /api-docs/openapi.json` | public | OpenAPI doc is shapes only — verified by test to contain no data rows (no SKU, email, token, account number) |
| `POST /api/v1/auth/login`, `/counter-switch`, `/passkey/verify`, `GET /api/v1/auth/oidc/callback`, `POST /api/v1/auth/override-verify` | public | Passkey/OIDC are 501 stubs |
| `POST /api/v1/auth/logout` | any live session | |
| `GET /api/v1/colors/search`, `/api/v1/tinting/formulas`, `/api/v1/tinting/colorants`, `/api/v1/inventory/balances`, `/api/v1/deliveries/dispatch` | `InventoryView` | |
| `POST /api/v1/tinting/formulas` | `InventoryAdjust` | |
| `GET /api/v1/reports/daily-usage`, `/low-stock`, `/api/v1/edi/documents` | `ReportView` | |
| `GET /api/v1/ar/customers/{id}/summary`, `/api/v1/ar/aging` | `ArView` | |
| `GET /api/v1/crm/activities`, `/api/v1/crm/appointments` | `CustomerManage` | HQ CRM is read-only; writes go through the edge |
| `POST /api/v1/sync/ingest` | `SyncIngest` **or** `dfs_` service token (`sync:ingest`) | Edge→HQ sync ingest. Service tokens are strictly bound to their single `store_id` (403 on cross-store batch). |
| `GET /api/v1/sync/conflicts`, `POST /api/v1/sync/conflicts/{entry_id}/resolve` | `SyncIngest` (staff only) | HQ conflict triage (F8). Service tokens get 403. |
| `POST/GET /api/v1/auth/service-tokens`, `POST /api/v1/auth/service-tokens/{id}/revoke` | `UserManage` (Admin only) | Issue, list (metadata only — never hashes), and revoke store `dfs_` sync tokens. |

### Sync ingest auth (Phase 8 + Security Hardening)

`POST /api/v1/sync/ingest` accepts either:

- a staff session `Bearer dfp_…` token with permission `SyncIngest` (Manager/Admin), or
- a long-lived, revocable **store service token** `Bearer dfs_…` carrying scope
  `sync:ingest` (migration `0016_security_sync_hardening.sql`), minted via
  `df-server tokens create` or `POST /api/v1/auth/service-tokens`.

Service tokens use the same 256-bit entropy + SHA-256 hash-at-rest discipline as
`dfp_` sessions (raw secret printed once, never persisted). They are least-privilege:
`df-auth-http::gate` rejects `dfs_` tokens with **403** on every non-ingest route,
and `sync_ingest` rejects any batch whose `store_id` does not match the token's bound
`store_id` with **403**. The edge daemon reads the token from `DF_HQ_TOKEN` (with
`--hq-url` / `DF_HQ_URL` and `--sync-interval-secs` / `DF_SYNC_INTERVAL_SECS`).

### Portal scoping (F4, HTTP layer)

The gate's `Portal` rule admits only `PortalContractor`
sessions. Every portal handler additionally checks
`AuthSession::may_access_customer(id)` — the session's
`customer_id` must equal the requested customer — and returns
**403, never 404**, so a contractor cannot probe whether another
customer id exists. `customers.portal_enabled` remains a second
gate, as in Phase 5. Staff sessions (any role, including Admin)
get 403 on portal routes: the portal is for contractors, not a
staff back door.

### Public-endpoint exceptions (intentional)

- `GET /health` (both surfaces) and `GET /api-docs/openapi.json`
  (HQ) — no data, no session.
- Auth login endpoints (listed above) — a session cannot be
  required to obtain a session.
- BOPIS storefront (edge): `GET /catalog/products`,
  `POST /bopis/orders`, `GET /bopis/orders/{id}` — the public
  catalog and order placement/status. Order **completion**
  (counter pickup) is deliberately NOT public: it requires
  `PosCheckout`.
- WebSocket `?token=` query param — browsers cannot set the
  `Authorization` header on the handshake.

### Actor wiring

Handlers use the session, not the caller, as the actor:

- `POST /pos/tills/open`: the till's cashier is the session's
  `user_id`. The request body's `cashier_id` is still accepted
  for wire compatibility but is **ignored** — a caller cannot
  open a till as someone else. Checkout inherits the till's
  cashier, so sales are attributed to the logged-in cashier.
- Void/return have no actor column in their tables and the edge
  writes no `audit_logs` row for them — the `reason_code` on an
  override is accepted but not persisted. Recorded as a
  follow-up in §9, not silently fixed by inventing a column.

### Tests

- `crates/df-edge/tests/phase7_http_auth.rs` (4 tests) and
  `crates/df-api/tests/phase7_http_auth.rs` (3 tests): exit
  tests (a)–(d) over real routers — 401 without a token,
  403 cashier-on-manager-route, portal A-vs-B scoping with a
  real reorder, expired token, logout revocation.
- `crates/df-edge/src/http.rs` unit tests (12) and
  `crates/df-api/src/lib.rs` unit tests (7, incl. the OpenAPI
  no-data assertion) cover the same flows at handler level.
- Shared seeding helper:
  `df_auth_http::test_support::seed_test_users` — seeds an
  admin/manager/cashier/sales/warehouse/driver + two portal
  contractors (A/B) with live `dfp_` tokens. Note: Argon2id
  hashing makes seeded tests slow (~35 s for the 4-test edge
  integration file in debug).

## 7. Bootstrap procedure

No default password exists anywhere. The first admin is created
with the CLI; migrations run first, so it works on a fresh database.

```sh
# 1. Create the first admin. Password from DF_USERS_PASSWORD when set,
#    otherwise read from stdin at a prompt (never logged, never printed).
export DF_USERS_PASSWORD='correct horse battery staple'   # or omit and type it
df-server users create-admin --db sqlite:edge.db --org-id <uuid> \
  --email admin@example.invalid --display-name "Admin"
# → {"id": "...", "email": "admin@example.invalid", "display_name": "Admin",
#    "role": "Admin", "org_id": "..."}

# 2. A second admin is refused unless --force is passed AND the operator
#    types YES at the confirmation prompt:
df-server users create-admin --db sqlite:edge.db --org-id <uuid> \
  --email admin2@example.invalid --display-name "Admin 2" --force
# (prompt) org <uuid> already has 1 admin(s); type YES to create another admin:

# 3. Create staff / portal users. --role is one of Cashier | SalesAssociate |
#    Warehouse | Driver | Manager | Admin | PortalContractor.
#    PortalContractor requires --customer-id; other roles reject it.
df-server users create --db sqlite:edge.db --org-id <uuid> \
  --email cashier@example.invalid --display-name "Cashier" \
  --role Cashier --pin 4821
df-server users create --db sqlite:edge.db --org-id <uuid> \
  --email contractor@example.invalid --display-name "Contractor" \
  --role PortalContractor --customer-id <customer-uuid>

# HQ (PostgreSQL) variant — same commands, a Postgres URL instead of
# a SQLite file. `df-server hq` (Phase 8, v1.2) serves the same
# `df-api` router — and the same auth gate — against Postgres.
export DATABASE_URL='postgres://decor:<password>@127.0.0.1:5432/decorpos'
df-server migrate schema            # first run only; a second run is a no-op
df-server users create-admin --db "$DATABASE_URL" --org-id <uuid> \
  --email admin@example.invalid --display-name "Admin"
df-server hq --db "$DATABASE_URL" --org-id <uuid> --listen 127.0.0.1:3000
# --db wins over DATABASE_URL when both are set; with neither, the
# command exits 2. The URL (and its password) is never logged —
# startup logs show postgres://***@host:5432/decorpos.

# 4. Verify and manage.
df-server users list --db sqlite:edge.db --org-id <uuid>
df-server users revoke-sessions --db sqlite:edge.db --email cashier@example.invalid
```

Notes:

- `--db` / `DATABASE_URL` precedence (`hq`, `migrate schema`):
  `--db <url>` wins when given; otherwise `DATABASE_URL` is used;
  with neither, the command exits 2. The URL format is a sqlx URL:
  `postgres://<user>:<password>@<host>:<port>/<database>` or
  `sqlite:<path>` / `sqlite::memory:`. Never commit a real URL or
  password; pass it via env or a secrets manager.
- `--password-env NAME` (default `DF_USERS_PASSWORD`) names the env
  var the password is read from. An empty/unset var falls through
  to the stdin prompt.
- `--no-password` creates a PIN-only user (no password hash); without
  it, a user needs a password or a `--pin`.
- When the org row does not exist yet it is created with
  `--org-name` (default `Decor POS`).
- Passwords are validated (≥ 8 characters) by Argon2id hashing;
  an invalid password or PIN exits 2 with the reason on stderr.
- Deactivating a user (`df_db::set_user_active`) blocks their next
  login **and** their next session validation — no separate session
  sweep is needed, though `revoke-sessions` removes the rows from
  the live list.

## 8. Schema (migration `0012_phase7`, both dialects)

```sql
CREATE TABLE roles (id UUID/TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE);
-- seeded with the seven well-known ids from §3.

ALTER TABLE users ADD COLUMN portal_customer_id UUID/TEXT
    [REFERENCES customers(id) -- PostgreSQL only];

CREATE TABLE sessions (
    id              UUID/TEXT PRIMARY KEY,
    user_id         UUID/TEXT NOT NULL REFERENCES users(id),
    token_hash      TEXT NOT NULL UNIQUE,   -- SHA-256 hex; the UNIQUE constraint is the token_hash index
    role            TEXT NOT NULL,          -- cached Role name at login
    customer_id     UUID/TEXT,              -- portal scope, NULL for staff
    created_at      TIMESTAMPTZ/TEXT NOT NULL,
    expires_at      TIMESTAMPTZ/TEXT NOT NULL,
    revoked_at      TIMESTAMPTZ/TEXT,       -- NULL while live
    last_used_at    TIMESTAMPTZ/TEXT NOT NULL
);
CREATE INDEX idx_sessions_user_id ON sessions (user_id, created_at);
```

## 9. Deliberately left open

- **Fixed in Phase 7 verification (coordinator):** the two gaps
  recorded here during the build are closed — `PortalContractor`
  now holds no staff permissions (its `ArView` is removed in
  `rbac.rs`, so a contractor session cannot reach the unscoped
  staff AR routes at all; tests
  `portal_contractor_holds_no_staff_permissions`), and scheduling
  has real permissions (`SchedulingView` for every staff role,
  `SchedulingManage` for Manager/Admin; tests
  `scheduling_permissions`, routes split into read/manage groups
  in `df-edge/src/http.rs`).
- **Audit actor columns:** void/return record no actor, and an
  override's `reason_code` is accepted but not persisted. Needs a
  schema change (an actor column / audit row), not an HTTP-layer
  invention.
- **EDI vendor config** has no separate permission: all `/edi/*`
  routes (including send/intake/import) are `ReportView`-gated.
- **Frontend login screens:** landed in Phase 7 (login screens in
  pos-desktop, web-portal, mobile-pwa — `web-backoffice` was absorbed
  into `pos-desktop`; token `df-pos-token`, `Authorization: Bearer`;
  401 returns to login). Known limit: tokens live in localStorage, not
  httpOnly cookies.
- **Badge-only lookup:** a badge scan with no selected user cannot
  name its user (hashes can't be searched). Needs a non-secret badge
  identifier column if the counter UX wants tap-to-switch without
  selecting a cashier first.
- **Auth rate limiting (F10):** implemented in `df-auth-http::hardening`
  (20 attempts / 60 s per client IP by default across login, counter-switch,
  override-verify, portal-login, and first-run setup; returns `429` +
  `Retry-After` before Argon2id runs). Account-level lockout across distributed
  IPs remains a future enhancement if exposed to the public internet.
- **No refresh tokens / sliding sessions:** 8 h is hard; a still-
  working cashier logs in again. A refresh flow is a later phase if
  the counter UX asks for it.
- **PostgreSQL round-trip** still untested live (standing caveat
  since Phase 0 — no PG server in this VM). The PG migration is
  embedded and compile-checked; SQLite is tested end-to-end
  (`crates/df-db/tests/sqlite_phase7_auth.rs`, 10 tests).
- **Session expiry cleanup:** expired rows are rejected on read but
  never deleted. A janitor job (or `DELETE ... WHERE expires_at <
  now()`) is a later ops item.
