# Contributing to LinkedVault

Thanks for helping improve LinkedVault. This guide covers local setup, verification,
and pull request expectations. Architecture and agent/dev invariants live in the
linked docs below — do not invent APIs or duplicate those contracts here.

## Prerequisites

- **Node.js** with npm (Vite 8 expects a current Node; Node 20.19+ or 22.12+ is a
  safe baseline)
- **Rust** toolchain with MSRV **1.77.2** (see `apps/desktop/src-tauri/Cargo.toml`)
- **Microsoft C++ Build Tools** or Visual Studio Build Tools (Windows)
- **Microsoft WebView2 Runtime**

## Run locally

From the repository root:

```powershell
npm --prefix apps\desktop install
npm run dev
```

`npm run dev` launches the Tauri desktop app. For a frontend-only Vite preview
(no native desktop commands):

```powershell
npm run web:dev
```

See the root [README.md](README.md) for PowerShell `npm.ps1` workarounds and
production build / release notes.

## Verify before you open a PR

Run what matches the surface you changed:

| Area | Commands |
| --- | --- |
| TypeScript / UI | `npm run verify:no-any`, `npm run build` |
| Newspaper / responsive UI | `npm run verify:ui` |
| Rust | `npm run cargo:clippy`, `npm run cargo:test` |
| Module layout / providers | `npm run verify:architecture` |
| Writer, migrations, clipping durability | `npm run verify:persistence` |

A failure in `verify:no-any` or `build` blocks completion for TypeScript work.

## Architecture and invariants

- **Architecture source of truth:** [docs/architecture/README.md](docs/architecture/README.md)
- **Frontend / Rust ownership:** [docs/architecture/frontend-rust-ownership-boundary.md](docs/architecture/frontend-rust-ownership-boundary.md)
- **Agent and developer invariants:** [AGENTS.md](AGENTS.md)

Do not add a fourth provider scheduler, cross-import providers, or move ADRs out
of `docs/architecture/`.

## Pull request expectations

- Do not commit secrets, cookies, tokens, or local `LinkVaultData` databases.
- Keep provider isolation: `linkedin`, `coursera`, and `newspaper` must not
  import each other’s internals.
- When you change a Tauri command name, argument, or payload, update the typed
  TypeScript adapter in the same change. Do not weaken types with `any`.
- Prefer focused diffs; leave unrelated dirty work and generated build trees alone.
- Follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## Security reports

Do not open public issues for vulnerabilities. See [SECURITY.md](SECURITY.md).
