# Sahl — working conventions

Offline-first, multi-vertical POS. Rust core shared by terminal and server; SvelteKit UI.

## The one rule that matters

**Every calculation touching money, tax, or invoice identity lives in `sahl-core` and nowhere else.**
TypeScript never computes a total — it *displays* one. The terminal and the server run the same compiled
code, so they cannot drift. If you find yourself adding arithmetic to a `.ts` file, stop.

`sahl-core` stays I/O-free and async-free. No `tokio`, no `sqlx`, no filesystem. That is what keeps it
testable, portable, and shareable between the Tauri binary and the Axum server.

## Layout

```
crates/sahl-core/      pure domain: money, tax, event, ledger, projection, policy
crates/sahl-fiscal/    Fiscalization trait + bd_mushak / zatca / noop
crates/sahl-terminal/  Tauri v2 app — SQLite, hardware, sync client
crates/sahl-server/    Axum — sync ingest, API, jobs
apps/terminal/         SvelteKit SPA (adapter-static) → Tauri webview
apps/dashboard/        SvelteKit SSR → owner dashboard
packages/ui/           shared Svelte components, design tokens, icons
docs/                  gitignored — plans and research, never committed
```

## Money

- Integer minor units only (`i64`). **Never `f64`.** `clippy::float_arithmetic` is `deny` at the
  workspace root, so this is enforced, not aspirational.
- Arithmetic is checked. `arithmetic_side_effects` is `deny` — a silent wrap in this codebase is a
  financial defect, not a lint nit.
- Splitting money must not lose or invent a cent. Use the allocation helpers; never divide and round.

## Rust

- `unwrap`/`expect`/`panic`/`todo` are `deny` outside tests. Return typed errors (`thiserror`).
- `unsafe_code` is `forbid`.
- Every domain invariant gets a property test, not just an example test.

## Frontend

- **Formatting is `Intl` only** — `NumberFormat`, `DateTimeFormat`, `RelativeTimeFormat`. Never a custom
  formatter. Rust owns the arithmetic; JS only formats.
- **Always pass `numberingSystem: 'latn'`.** `bn-BD` renders Bengali digits and `ar-SA` Arabic-Indic by
  default. Geist Mono has neither glyph set, so numerals would silently fall back and break tabular
  alignment — and ZATCA expects Western digits. Use the shared helpers in `packages/ui`.
- Design tokens only. No raw hex in components; no hardcoded px outside the token file.
- Icons: Lucide, per-icon imports, bundled locally.
- Fonts: Mona Sans (UI), Geist Mono (all numerics, tabular), Anek Bangla, IBM Plex Sans Arabic.
  **Self-hosted and bundled — never a CDN.** The terminal is offline-first; a failed webfont request is a
  register rendering in Times New Roman mid-rush.
- Locales `en` / `bn-BD` / `ar-SA`, with RTL designed in rather than bolted on.
- Two density modes over one token set: `compact` (dashboard, 13px body) and `touch` (terminal, 15px body,
  44px minimum targets). Mis-taps at a counter cost money.

## Config

Fail fast at startup on missing or malformed env. **No hardcoded defaults, no silent fallbacks.** A config
mistake must be loud and immediate, never a wrong number discovered in a monthly report.

## Errors

Typed errors in Rust. In SvelteKit: `+error.svelte` for 404/500, the `handleError` hook for reporting, and
a *designed* offline/degraded screen on the terminal — never a stack trace facing a cashier.

## Git

- Author: `ebnsina <ebnsina.me@gmail.com>` (set per-repo).
- **No `Co-Authored-By` trailers.**
- Remote uses the `github-es` SSH host alias.
- `docs/` and `data/` are gitignored — no plans, roadmap, or secrets in the public repo.
- Push at each completed feature.

## Docs

Verify every library and API detail against current documentation (`ctx7`) before use. Do not write an API
call from memory — this stack moves faster than any training data.
