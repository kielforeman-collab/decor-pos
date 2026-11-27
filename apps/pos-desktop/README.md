# pos-desktop — Tauri 2 Counter POS (Phase 1, v0.2)

Keyboard-first counter checkout for decor-pos: product scan/entry,
matrix-priced totals, quotes, returns, holds (park/resume), layaways
(deposit on a quote/order), split tender, till open/close, Z-report,
mock/standalone payment provider selection, and receipt preview.
The UI talks to `df-edge` — in-process in the native app, over local
HTTP in the dev path. No back-office/admin screens (Phase 3).

## Status — read this first

**The React frontend is complete and its production build is
verified** (`npm run build`: `tsc` + Vite, clean).

**The native Tauri shell build is UNVERIFIED in this VM and is not
claimed to work here.** What is missing:

- System libraries: `libwebkit2gtk-4.1-dev` + `libsoup-3.0-dev`
  (and GTK) are not installed, and `apt-get install` fails in this
  VM with held broken packages (`libzstd-dev` depends on
  `libzstd1 (= 1.5.5+dfsg2-2build1.1)` but
  `1.5.5+dfsg2-2build1.1+hatch1` is installed; `libwebkit2gtk-6.0-dev`
  is not packaged for this release at all). The `src-tauri` code
  therefore cannot link here.
- The Tauri CLI is not installed in this VM either
  (`cargo tauri` / `cargo install tauri-cli` was not run).

What *is* verified without those libraries:

- `cargo test -p pos-desktop` — the `src-tauri` crate compiles as a
  plain Rust crate (default features, no Tauri shell) and its
  `PosApp` test passes: edge opens, provider switches
  mock ↔ standalone, and barcode scanning runs the real `df-hw`
  parser.
- `npm run build` — the frontend builds clean.
- The dev path below, end to end (edge HTTP + Vite dev server +
  a real checkout through the UI's API).

To build the native shell on a machine with the system libraries:

```sh
cargo install tauri-cli --version 2
cargo run -p pos-desktop --features tauri-shell
# or: cd apps/pos-desktop && cargo tauri dev / cargo tauri build
```

The native app reads its store config from the environment:
`DF_CURRENCY` (`USD` or `CAD`, default `USD`), `DF_COUNTRY` and
`DF_REGION` (e.g. `CA` / `BC`). The counter UI sends CAD amounts,
so this store runs with `DF_CURRENCY=CAD DF_COUNTRY=CA DF_REGION=BC`
— `DF_COUNTRY`/`DF_REGION` seed the store row on first boot, which
is what tax prices from (`CA`/`BC` = GST 5% + PST 7%); without them
the seed leaves the jurisdiction blank and tax prices as 0.

## Dev path (verified in this VM)

Terminal 1 — the edge daemon (mock terminal, in-process `MemoryHq`):

```sh
export PATH="$HOME/.cargo/bin:$PATH"
cargo run -p df-server -- edge \
  --db sqlite:edge.db \
  --store-id 00000000-0000-0000-0000-00000000a000 \
  --org-id 00000000-0000-0000-0000-00000000b000 \
  --listen 127.0.0.1:8080 \
  --currency CAD --country CA --region BC
```

`--currency CAD --country CA --region BC` are this store's values
(the counter UI sends CAD; `CA`/`BC` = GST 5% + PST 7%): they seed
the store row on first boot, which is what tax prices from. Run
the edge before `seed-demo.sql` — its store insert is
`INSERT OR IGNORE`, so a seeded row is left alone.

Terminal 2 — the UI (Vite proxies `/api/*` to the edge on :8080):

```sh
cd apps/pos-desktop
npm install
npm run dev   # http://localhost:1420
```

Counter flow in the UI: open a till → scan / quick-pick / enter
lines → totals preview live → add tenders (blank amount = remaining
balance; split tender = multiple rows) → **Charge** (Ctrl+Enter).
Then: receipt panel, park/resume holds, quote, layaway (deposit),
return, close till for the Z-report, **Sync now**.

Shortcuts: `/` focuses the scanner · `Ctrl+Enter` charges.

### Seed the demo catalog

The quick-pick products and barcodes live in `src/catalog.ts`.
For eco fees to apply, seed the same products into the edge DB
(unseeded products still sell at list price, with tax from the
store's country/region and no eco fee):

```sh
sqlite3 edge.db < apps/pos-desktop/seed-demo.sql
```

Matrix pricing applies automatically when pricing rules exist for
the org (see `crates/df-edge/tests/offline_checkout_parity.rs` for
an example rule).

## How the two modes differ

| Feature | Native Tauri app | Dev path (Vite + df-server edge) |
|---|---|---|
| Checkout, preview, tills, Z-report, returns, holds, quotes, layaways, sync | yes (in-process `Edge`) | yes (HTTP API) |
| Payment provider switch (mock ↔ standalone) | yes | no — the server's provider is fixed (mock by default) |
| Receipt preview | captured ESC/POS text from the mock printer | rendered from the checkout response |
| Barcode scanning | real `df-hw` parser in Rust (`scan_barcode` command) | identical TS mirror in `src/scan.ts` |

## Layout

- `src-tauri/` — Rust backend. `PosApp` owns the `Edge`, a
  capturing `MockPrinter`, and the two providers. With
  `--features tauri-shell` it additionally exposes Tauri commands
  and runs the window; without the feature it is a plain library
  (the binary prints these instructions and exits 2). Env:
  `DF_STORE_ID`, `DF_ORG_ID`, `DF_DB` (default
  `sqlite:pos-desktop.db`), `DF_STORE_NAME`.
- `src/` — React 19 + TypeScript + Vite. `api.ts` picks the Tauri
  or HTTP backend; `scan.ts` mirrors the `df-hw` scanner parser;
  `catalog.ts` holds the demo catalog; `App.tsx` is the whole
  counter UI (checkout first; no admin screens).

## Still stub / not in v0.2

- Stripe Terminal: the provider exists in `df-payments` but its
  live calls are not implemented (Phase 1), so it is not selectable
  in the UI.
- Cashier sign-in (PIN/badge) is not wired: till open uses a fixed
  demo cashier id; real auth lands with `df-auth` integration.
- Layaway balance collection / layaway close-out is a Phase 3
  (AR/back-office) item; v0.2 records the layaway + deposit only.
- App icons are not bundled yet (`tauri.conf.json` has no icon
  list); packaging (`cargo tauri build`) needs them added.
