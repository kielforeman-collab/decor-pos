# Self-Hosting decor-pos (v1.2)

One-command stack, single-binary mode, and Kubernetes. Read
`docs/security-review-v1.0.md` first, and `docs/auth.md` for the
Phase 7 auth gate: since v1.1 every `df-api` / `df-server hq` route
except `/health`, `/api-docs/openapi.json` and the auth login
endpoints requires a `Bearer dfp_...` session token (no default
password exists; create the first admin in the POS GUI on first run
— open the POS at `http://localhost:1420` and fill in the
"Create your admin account" form — or, as the alternative,
`df-server users create-admin`). This stack is for a trusted local
network or a VPN, never the open internet without a reverse proxy
in front.

## What runs in v1.2

| Component | Status in v1.2 |
|---|---|
| `edge` — `df-server edge` (SQLite) | Working: counter POS, portal, BOPIS, sync outbox |
| `api` — `df-api` (HQ read API) | Working: health, OpenAPI, admin reads against the edge SQLite file |
| `hq` — `df-server hq` (PostgreSQL or SQLite) | Working in v1.2: connects to the database named by `--db` / `DATABASE_URL`, applies pending migrations, then serves the same `df-api` router — with the same Phase 7 auth gate — on `--listen` (default `127.0.0.1:3000`). `df-server migrate schema` applies migrations standalone (idempotent; a second run reports `already_up_to_date`). PostgreSQL round-trip: see Validation status below. |
| `postgres` — PostgreSQL 16 | Working in v1.2 as the HQ database (see `hq` above). The compose stack still provisions it with the same credentials/volume as in v1.0. |
| `standalone` / `worker` | Not implemented (`df-server <mode>` exits 2) |

## 1-command Docker Compose

Requirements: Docker Engine + Compose v2, 2 GB RAM, ports 8080 and
3000 free.

```sh
# Windows PowerShell (run from the repo root):
.\setup.ps1
# Linux / macOS:
./setup.sh
# Both generate .env from .env.example with fresh STORE_ID / ORG_ID
# UUIDs and a random POSTGRES_PASSWORD, and print the password once —
# save it. They refuse to overwrite an existing .env. Copying
# .env.example by hand and editing the UUIDs yourself also works.
# Never commit a real .env.
docker compose up --build -d
curl -s http://localhost:8080/health   # edge
curl -s http://localhost:3000/health   # api
curl -s http://localhost:3000/api-docs/openapi.json | head -c 400
```

### Create the first admin (GUI, first run)

With the stack up, start the counter POS and open it in a browser:

```sh
cd apps/pos-desktop
npm install   # first time only
npm run dev   # serves the POS on http://localhost:1420
```

Open `http://localhost:1420`. On a fresh install (no users yet) the
sign-in screen is replaced by a **"Create your admin account"**
form — email, display name, password — and the new admin is signed
straight in, no second step. Once any user exists the normal
sign-in screen appears instead.

Alternative (CLI, e.g. headless or for HQ): create the admin with
`df-server users create-admin` — see the HQ section below for the
exact command. `create-admin` refuses when an admin already exists
for the org (unless `--force` is passed and confirmed).

Stop with `docker compose down` (volumes kept) or
`docker compose down -v` (deletes the SQLite database and Postgres
data).

### What you get

- Edge POS API on `http://localhost:8080` — checkout, tills, holds,
  portal, BOPIS, sync. Try a price preview:
  `POST /pos/carts/price-preview` (see the OpenAPI doc on the API
  service for the full route list).
- HQ read API on `http://localhost:3000` — `/health`,
  `/api-docs/openapi.json`, and the Phase 2–4 admin reads
  (`/api/v1/crm/activities`, `/api/v1/deliveries/dispatch`,
  `/api/v1/edi/documents`, …).
- The edge database is a SQLite file in the `edgedata` volume
  (`/data/edge.db` in the container). Back it up by copying the
  volume or running `docker compose cp edge:/data/edge.db ./`.

## Environment variables

| Variable | Service | Default | Notes |
|---|---|---|---|
| `STORE_ID` | edge, api | *(required)* | UUID of the store the edge serves |
| `ORG_ID` | edge, api | *(required)* | UUID of the organization |
| `STORE_NAME` | edge | `Decor POS Store` | Shown on receipts |
| `CURRENCY` | edge | `CAD` | `USD` or `CAD` — must be `CAD` for this store: the counter UI sends CAD amounts |
| `COUNTRY` | edge | `CA` | `US` or `CA` — the store's country, seeded into the store row on first boot; tax prices from it |
| `REGION` | edge | `BC` | Province/state code, e.g. `BC` — seeded with `COUNTRY`; `CA`/`BC` = GST 5% + PST 7% |
| `DATABASE_URL` | hq, migrate schema | *(none — `--db` or this is required)* | sqlx URL for the HQ database: `postgres://<user>:<password>@<host>:<port>/<database>` or `sqlite:<path>`. `--db` wins when both are set. Never commit a real URL; never log one (startup logs show the redacted form `postgres://***@host:5432/db`). |
| `POSTGRES_USER` / `POSTGRES_PASSWORD` / `POSTGRES_DB` | postgres | `decor` / *(required)* / `decorpos` | The HQ database credentials; `DATABASE_URL` is built from these. |
| `STRIPE_API_KEY`, `STRIPE_TERMINAL_LOCATION_ID` | edge | *(empty)* | Env-only; empty = mock terminal. Live Stripe Terminal calls are not implemented in v1.0 |
| `DF_TYPST_BIN` | edge | *(empty)* | Path to a `typst` binary for monthly-statement PDFs. The image does not bundle Typst — mount one or run `statements` on the host |
| `DF_HQ_URL` / `DF_HQ_TOKEN` | edge | *(empty)* | Remote HQ base URL (e.g. `https://hq.example.com`) and store-scoped `dfs_` service token (minted via `df-server tokens create`). When set, `df-server edge` pushes deltas to HQ instead of in-memory `MemoryHq`. |
| `DF_SYNC_INTERVAL_SECS` | edge | `30` | Background HQ sync interval in seconds when `DF_HQ_URL` is configured (`0` disables timer). |
| `DF_CORS_ORIGINS` | edge, hq, api | *(empty — fail closed)* | Comma-separated exact allowed browser origins (e.g. `https://pos.example.com`). Wildcard `*` is rejected. |
| `DF_AUTH_RATE_LIMIT_MAX` / `DF_AUTH_RATE_LIMIT_WINDOW_SECS` | edge, hq, api | `20` / `60` | Per-client-IP auth endpoint rate limit (`0` disables). Returns `429` + `Retry-After` before Argon2id runs. |
| `DF_MAX_BODY_BYTES` | edge, hq, api | `4194304` (4 MiB) | Maximum HTTP request body size. |
| `DF_TRUST_PROXY_HEADERS` | edge, hq, api | `false` | Set `true` only behind a trusted TLS/mTLS reverse proxy that overwrites `X-Forwarded-For`. |

## Volumes & ports

| Volume / port | Purpose |
|---|---|
| `edgedata` → `/data` | Edge SQLite database (`edge.db`) — the only durable state in v1.0. Shared by the `api` service (read/write pool, no migrations). |
| `pgdata` → `/var/lib/postgresql/data` | Postgres data — the HQ database in v1.2. |
| `8080` (edge), `3000` (api) | Host-mapped HTTP. Postgres is bound to `127.0.0.1:5432` only. |

## Single binary (no Docker)

```sh
cargo build --release --locked -p df-server -p df-api
./target/release/df-server edge --db sqlite:edge.db \
  --store-id <uuid> --org-id <uuid> --listen 127.0.0.1:8080 \
  --currency CAD --country CA --region BC
./target/release/df-api --db sqlite:edge.db \
  --org-id <uuid> --store-id <uuid> --listen 127.0.0.1:3000
```

`df-server edge` creates and migrates the SQLite database on open.
`df-api` runs no migrations — the database must already exist.

`--currency` / `--country` / `--region` are the values seeded into
the store row on first boot (`--country CA --region BC` = GST 5% +
PST 7% for this store); the seed never overwrites an existing row,
so changing them later means fixing the store row, not re-running
the edge. Omit `--country` / `--region` and the seed leaves the
jurisdiction blank — tax then prices as 0 (the edge prints a
warning on first boot in that case).

### Local PostgreSQL (dev)

On Ubuntu, the dev VM provisions PostgreSQL 16 from apt (verified
Nov 26, 2026 — no workaround was needed despite an earlier apt
issue):

```sh
apt-get update
DEBIAN_FRONTEND=noninteractive apt-get install -y postgresql-16
pg_ctlcluster 16 main start
# as the postgres superuser:
CREATE ROLE decorpos LOGIN PASSWORD '<password>' CREATEDB;
CREATE DATABASE decorpos_hq OWNER decorpos;
```

Connection URL shape: `postgres://decorpos:<password>@127.0.0.1:5432/decorpos_hq`
(never commit a real password; the round-trip tests read the URL
from an env/file outside the repo and skip cleanly when no live
Postgres is reachable).

### HQ against PostgreSQL (v1.2)

```sh
# 1. Apply the schema (first run only; a second run is a no-op).
export DATABASE_URL='postgres://decor:<password>@127.0.0.1:5432/decorpos'
./target/release/df-server migrate schema
# → {"database": "postgres://***@127.0.0.1:5432/decorpos", "dialect": "postgres",
#    "status": "migrated", "newly_applied": [1, 2, ...], "applied": [1, 2, ...]}

# 2. Create the first admin (migrations run first, so this also works
#    on a fresh database; password from DF_USERS_PASSWORD or stdin).
export DF_USERS_PASSWORD='correct horse battery staple'
./target/release/df-server users create-admin --db "$DATABASE_URL" \
  --org-id <uuid> --email admin@example.invalid --display-name "Admin"

# 3. Start HQ. --db wins over DATABASE_URL when both are set.
./target/release/df-server hq --db "$DATABASE_URL" \
  --org-id <uuid> --listen 127.0.0.1:3000
curl -s http://127.0.0.1:3000/health   # {"status":"ok","version":"1.2.5"}
```

`df-server hq` logs the redacted database URL, how many migrations
are applied (and how many this start applied), then the listen
address. Admin routes require a `Bearer dfp_...` token from
`POST /api/v1/auth/login` — see `docs/auth.md` §7 for the full
bootstrap and the role → route matrix.

## Helm (Kubernetes)

A skeleton chart is in `deploy/helm/decor-pos/` (edge + API
Deployments/Services, shared PVC, optional PostgreSQL
StatefulSet). It is a skeleton: values are placeholders and the
Services are `ClusterIP` only — no Ingress is defined, deliberately,
because v1.0 had no authentication — since v1.1 the Phase 7 gate
(see `docs/auth.md`) applies, so an Ingress still needs TLS plus
the Bearer-token flow, not open access.

```sh
# Build and push the image first (the chart references it by
# image.repository / image.tag in values.yaml):
docker build -t my-registry/decor-pos:1.0.0 .
docker push my-registry/decor-pos:1.0.0

helm install decor-pos deploy/helm/decor-pos \
  --set image.repository=my-registry/decor-pos \
  --set image.tag=1.0.0 \
  --set store.id=<store-uuid> \
  --set store.orgId=<org-uuid> \
  --set postgres.password=<long-random-password>

kubectl -n default port-forward svc/decor-pos-edge 8080:8080
curl -s http://localhost:8080/health
```

Caveats:

- Edge and API share one `ReadWriteOnce` PVC. Both pods must land
  on the same node; for multi-node clusters use a `ReadWriteMany`
  storage class or pin the pods with affinity.
- One edge replica only — the SQLite file is local to the pod.
- Set `postgres.enabled=false` to skip the PostgreSQL StatefulSet
  (only when HQ is not used).

## Validation status (honest)

What was actually run in the v1.0 VM, and what was not:

- ✅ `cargo build --release --locked -p df-server -p df-api`
  succeeds; both binaries run and answer `GET /health` (the dev
  path `df-server edge` + HTTP checks are exercised by the workspace
  test suite).
- ✅ `docker-compose.yml`, Helm `Chart.yaml`/`values.yaml` parse as
  YAML (checked with Python `yaml.safe_load`). Helm templates were
  reviewed by hand; `helm template` was **not** run (no Helm binary
  in the VM) and `helm lint` was **not** run.
- ❌ `docker build` was **not** run — no Docker daemon exists in
  this VM (`docker: command not found`). The Dockerfile's build
  commands are exactly the `cargo build --release --locked` command
  validated above, but the image itself (base-image tags
  `rust:1.99-bookworm`, `debian:bookworm-slim`, the `curl` install,
  the non-root user, the healthcheck) is **unverified until run on
  a machine with Docker**.
- ❌ `docker compose up` was **not** run (no daemon). The stack is
  validated by review + YAML parse only.
- ⚠️ Base-image tag `rust:1.99-bookworm` assumes that tag exists on
  Docker Hub; if it does not, use the closest newer
  `rust:<version>-bookworm` tag (the code needs ≥ 1.88 for
  let-chains).

### v1.2 additions (HQ mode)

- ✅ `df-server migrate schema` verified against SQLite: first run
  applies migrations 1–12 (0008 does not exist) and reports them;
  second run exits 0 with `"status": "already_up_to_date"` and no
  changes. Covered by `crates/df-server/tests/migrate_schema.rs`
  (also covers the `DATABASE_URL` fallback).
- ✅ `df-server hq` verified against SQLite: starts, migrates,
  answers `/health` with `{"status":"ok","version":"1.2.5"}`,
  returns 401 on an admin route without a token (the Phase 7 gate
  is live on HQ), and 200 with a real admin session
  (`POST /api/v1/auth/login` → `Bearer dfp_...`). Covered by
  `crates/df-server/tests/hq_mode.rs`.
- ✅ Live-Postgres round-trip (PostgreSQL 16.15, local apt install —
  see "Local PostgreSQL (dev)" below): `df-server migrate schema`
  applies 12 migrations `[1,2,3,4,5,6,7,9,10,11,12,13]` (0008 does
  not exist), second run `already_up_to_date`; `df-server hq`
  answers `/health` 200 `{"status":"ok","version":"1.2.5"}` and
  401s protected routes without a token; edge (SQLite) cash sale
  2 gal (grand 173.2088) + charge sale 1 gal (grand 86.6044) pushed
  via HTTP ingest `POST /api/v1/sync/ingest` (`applied=5
  duplicates=0`) → HQ inventory −3 (edge −3), HQ AR 86.6044 (edge
  86.6044); replay `applied=0 duplicates=5`, figures unchanged.
  Covered by `crates/df-server/tests/postgres_hq_round_trip.rs`
  (4/4, skips cleanly with a printed reason when no live Postgres
  is reachable). The blocker found during this phase is fixed:
  `payments.authorization` is a reserved word in PostgreSQL, so
  the column is quoted (`"authorization"`) in the Postgres
  migration and the repo INSERT.
- ✅ Postgres parity (Phase 8): sqlx 0.8.6's `Any` driver does NOT
  rewrite `?` → `$N` (it reaches Postgres literally, where `?` is
  the JSONB operator) and cannot bind/decode native PG types — so
  all queries now use `$N` placeholders and the Postgres
  migrations use TEXT columns with the same encoding SQLite
  already uses (ids hyphenated, money exact decimal strings,
  timestamps RFC 3339, JSON verbatim). Native PG types
  (UUID/NUMERIC/TIMESTAMPTZ) remain a future data-layer phase, not
  a silent regression: the logical schema is identical.
- ✅ Full workspace suite: 458 passed, 0 failed (Phase 7 baseline
  439); fmt + clippy clean.
