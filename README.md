# decor-pos — Open-Source Rust Paint & Decorating POS

Open-source, self-hostable POS & retail management for independent paint
& decorating retailers. Full functional parity with Epicor Decor Fusion
is the goal; see the canonical spec at
`../your_files/decor-fusion-pos-plan/architectural-specification.md`
(spec §1–§9).

> **Phase 1 (v0.2) is what this repo contains right now.** Offline
> counter POS: `df-edge` + `df-sync` + `df-hw` + `df-payments` are
> real, `df-server edge` mode serves the local POS API, and the
> `pos-desktop` counter UI is built. See "What's real vs stub" below.

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

Or run the API skeleton:

```sh
cargo run -p df-api        # http://127.0.0.1:3000/health
cargo run -p df-server -- --help
```

## What's real vs stub (Phase 1)

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
  outbox). SQLite round-trip tests pass, including file-backed
  databases (`sqlite:edge.db` creates the file on open); the
  PostgreSQL path is migration-embedded and compile-checked — a
  live-Postgres round-trip test is still open (needs a running
  PG 16+).
- `df-auth` — Argon2id password hashing/verification, counter PIN and
  badge-token hashing, RBAC roles/permissions. WebAuthn/Passkeys and
  OIDC are trait + stub only (return `NotImplemented`).
- `df-api` — Axum skeleton, OpenAPI 3.1 via utoipa
  (`/api-docs/openapi.json`), working `/health`. Auth routes exist
  and return 501.
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
  sync works out of the box in the dev path.
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
- `df-server` — `edge` mode real (local SQLite + POS HTTP/WS API,
  mock terminal + in-process `MemoryHq`); `standalone`, `hq`,
  `worker`, `migrate` still exit 2.
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

Exit test: `crates/df-edge/tests/offline_checkout_parity.rs`
(`offline_checkout_print_reconnect_parity`,
`offline_return_reverses_ledger`) — offline cash sale → print →
reconnect → HQ inventory/transaction/AR parity, replay is a no-op,
return reverses the ledgers on the next sync.

Stubs (crate exists, `phase0_stub()`, no implementation):

- `df-tinter`, `df-edi`, `df-plugin-sdk`, `df-jobs`, `df-reports` —
  implemented in Phases 2–6.
- `df-server` — `standalone`, `hq`, `worker`, `migrate` modes
  report "not implemented".

## Deferred

Benjamin Moore COLORx tint machine integration is **deferred to
post-v1.0** (spec §9). There is no COLORx code, plugin, or protocol
work anywhere in this repo. The generic `df-tinter` bridge (Phase 2)
and the `TinterPlugin` trait are the integration point for later.

## What's next (Phase 2, v0.3)

`color_catalog`, `tint_formulas`, colorant COGS costing, searchable
customer/job/room formula history, `df-tinter` generic
watch-folder/TCP/serial bridge + virtual simulator + ZPL can-label
templates, and the feature-flagged decorating verticals
(`vertical_flooring`, `vertical_window_treatments`,
`vertical_wallpaper`, `vertical_sample_loans`). Benjamin Moore
COLORx stays deferred (spec §9) — Phase 2 ships the generic bridge
and simulator only.

## License

Dual-licensed Apache-2.0 OR MIT — see `LICENSE-APACHE` and
`LICENSE-MIT`.
