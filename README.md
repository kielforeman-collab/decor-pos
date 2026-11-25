# decor-pos — Open-Source Rust Paint & Decorating POS

Open-source, self-hostable POS & retail management for independent paint
& decorating retailers. Full functional parity with Epicor Decor Fusion
is the goal; see the canonical spec at
`../your_files/decor-fusion-pos-plan/architectural-specification.md`
(spec §1–§9).

> **Phase 0 (v0.1) is what this repo contains right now.** Core engine
> and blueprint only — no counter POS, no tinter bridge, no payments,
> no portal. See "What's real vs stub" below.

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

Run the API skeleton:

```sh
cargo run -p df-api        # http://127.0.0.1:3000/health
cargo run -p df-server -- --help
```

## What's real vs stub (Phase 0)

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
  ledger). SQLite round-trip tests pass; the PostgreSQL path is
  migration-embedded and compile-checked — a live-Postgres round-trip
  test is a Phase 1 item.
- `df-auth` — Argon2id password hashing/verification, counter PIN and
  badge-token hashing, RBAC roles/permissions. WebAuthn/Passkeys and
  OIDC are trait + stub only (return `NotImplemented`).
- `df-api` — Axum skeleton, OpenAPI 3.1 via utoipa
  (`/api-docs/openapi.json`), working `/health`. Auth routes exist
  and return 501.

Stubs (crate exists, `phase0_stub()`, no implementation):

- `df-sync`, `df-edge`, `df-hw`, `df-tinter`, `df-payments`, `df-edi`,
  `df-plugin-sdk`, `df-jobs`, `df-reports` — implemented in Phases 1–6.
- `df-server` — CLI subcommands exist; every mode reports
  "not implemented in Phase 0".

## Deferred

Benjamin Moore COLORx tint machine integration is **deferred to
post-v1.0** (spec §9). There is no COLORx code, plugin, or protocol
work anywhere in this repo. The generic `df-tinter` bridge (Phase 2)
and the `TinterPlugin` trait are the integration point for later.

## What's next (Phase 1, v0.2)

`df-edge` local daemon + `df-sync` UUIDv7 outbox replication, `df-hw`
(ESC/POS, ZPL, HID scanner, cash drawer), `df-payments` (Stripe
Terminal + standalone mode + surcharge), and the Tauri 2 counter POS
with offline checkout → print → reconnect → ledger parity test.

## License

Dual-licensed Apache-2.0 OR MIT — see `LICENSE-APACHE` and
`LICENSE-MIT`.
