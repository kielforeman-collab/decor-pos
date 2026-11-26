# Self-Hosting decor-pos (v1.0)

One-command stack, single-binary mode, and Kubernetes. Read
`docs/security-review-v1.0.md` first: **v1.0 has no authentication on
the API or the edge HTTP API** — this stack is for a trusted local
network or a VPN, never the open internet.

## What runs in v1.0

| Component | Status in v1.0 |
|---|---|
| `edge` — `df-server edge` (SQLite) | Working: counter POS, portal, BOPIS, sync outbox |
| `api` — `df-api` (HQ read API) | Working: health, OpenAPI, admin reads against the edge SQLite file |
| `postgres` — PostgreSQL 16 | **Provisioned only.** `df-server hq` / `migrate` are not implemented, so nothing connects to it yet. It is in the stack so the database, volume and credentials exist when HQ lands. PostgreSQL round-trip is untested in v1.0. |
| `standalone` / `hq` / `worker` / `migrate` | Not implemented (`df-server <mode>` exits 2) |

## 1-command Docker Compose

Requirements: Docker Engine + Compose v2, 2 GB RAM, ports 8080 and
3000 free.

```sh
cp .env.example .env
# Edit .env: replace STORE_ID / ORG_ID (fresh UUIDv7s) and
# POSTGRES_PASSWORD. Never commit a real .env.
docker compose up --build -d
curl -s http://localhost:8080/health   # edge
curl -s http://localhost:3000/health   # api
curl -s http://localhost:3000/api-docs/openapi.json | head -c 400
```

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
| `CURRENCY` | edge | `USD` | `USD` or `CAD` |
| `POSTGRES_USER` / `POSTGRES_PASSWORD` / `POSTGRES_DB` | postgres | `decor` / *(required)* / `decorpos` | Provisioned for future HQ; unused in v1.0 |
| `STRIPE_API_KEY`, `STRIPE_TERMINAL_LOCATION_ID` | edge | *(empty)* | Env-only; empty = mock terminal. Live Stripe Terminal calls are not implemented in v1.0 |
| `DF_TYPST_BIN` | edge | *(empty)* | Path to a `typst` binary for monthly-statement PDFs. The image does not bundle Typst — mount one or run `statements` on the host |

## Volumes & ports

| Volume / port | Purpose |
|---|---|
| `edgedata` → `/data` | Edge SQLite database (`edge.db`) — the only durable state in v1.0. Shared by the `api` service (read/write pool, no migrations). |
| `pgdata` → `/var/lib/postgresql/data` | Postgres data (unused in v1.0) |
| `8080` (edge), `3000` (api) | Host-mapped HTTP. Postgres is bound to `127.0.0.1:5432` only. |

## Single binary (no Docker)

```sh
cargo build --release --locked -p df-server -p df-api
./target/release/df-server edge --db sqlite:edge.db \
  --store-id <uuid> --org-id <uuid> --listen 127.0.0.1:8080
./target/release/df-api --db sqlite:edge.db \
  --org-id <uuid> --store-id <uuid> --listen 127.0.0.1:3000
```

`df-server edge` creates and migrates the SQLite database on open.
`df-api` runs no migrations — the database must already exist.

## Helm (Kubernetes)

A skeleton chart is in `deploy/helm/decor-pos/` (edge + API
Deployments/Services, shared PVC, optional PostgreSQL
StatefulSet). It is a skeleton: values are placeholders and the
Services are `ClusterIP` only — no Ingress is defined, deliberately,
because v1.0 has no authentication.

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
- Set `postgres.enabled=false` to skip the (unused in v1.0)
  PostgreSQL StatefulSet.

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
