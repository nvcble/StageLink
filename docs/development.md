# Developing StageLink Core

## Prerequisites

| Tool | Version used | Notes |
| --- | --- | --- |
| Windows | 10/11 x64 | WASAPI capture is Windows-only |
| Node.js | 22.x | |
| Rust | 1.99 (stable) | `rustup` |
| MSVC Build Tools | VS 2022, C++ workload | Rust needs the linker; electron-builder expects MSVC |
| Windows SDK | 10.0.22621 or later | |

```powershell
winget install Rustlang.Rustup
winget install Microsoft.VisualStudio.2022.BuildTools --override `
  "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --add Microsoft.VisualStudio.Component.Windows11SDK.22621"
```

## First build

```bash
npm install                                   # also runs `nuxt prepare`
npm run build --workspace @stagelink/shared-protocol
npm run build:service                         # cargo build --release
npm run icons                                 # .ico from the official logo
```

The protocol package must be built before the desktop app: the Electron bundle
and the renderer both import it.

## Running in development

```bash
npm run dev
```

That starts the Nuxt dev server on `127.0.0.1:3579`, builds the Electron
bundles, waits for the server, and launches Electron.

The host is pinned to `127.0.0.1` deliberately. Nuxt's default binds `::1` only,
and Electron then cannot reach `http://127.0.0.1:3579` — a blank window with no
useful error.

Renderer HMR works normally. Editing the Electron main or preload process needs
`npm run dev:electron` again; editing Rust needs `npm run build:service` and a
service restart (Settings → Restart audio service, or relaunch).

**A renderer reload does not stop audio.** The service is a child of the Electron
main process, not of the window.

## Building for production

```bash
npm run build              # shared-protocol + Electron bundles + nuxt generate
npm run build:service      # the Rust service, release profile
npm run package            # the NSIS installer
```

Or an unpacked build, which is faster and enough to test production routing:

```bash
cd apps/desktop
npx electron-builder --win --x64 --dir --config electron-builder.config.cjs
node ../../scripts/smoke-test.mjs --production
```

Output lands in `apps/desktop/release/`.

## Repository layout

```
apps/desktop/
  electron/main/      main process: window, tray, protocol, supervisor, CSP
  electron/preload/   the only renderer-facing surface
  electron/ipc/       channel registration and event forwarding
  app/                Nuxt 4 source (pages, stores, composables, components)
crates/               the Rust workspace — see docs/architecture.md
packages/
  shared-protocol/    the single protocol definition
assets/branding/      the official logo (committed; derivatives are not)
database/migrations/  SQL reference copies of the Rust migrations
docs/                 this documentation
scripts/              icon generation, probes, the smoke test
tests/                cross-implementation and integration tests
```

## Useful commands

```bash
# Everything
npm test --workspaces --if-present
cargo test --manifest-path crates/Cargo.toml
npm run typecheck --workspaces --if-present

# Diagnose an audio interface
cargo run -p audio-capture --example probe

# Drive the service without Electron
node scripts/probe-service.mjs

# Launch the app and assert it came up
node scripts/smoke-test.mjs

# Why is the window blank?
node scripts/renderer-console.mjs

# Keep the bundled icon list in step after adding an icon
node scripts/sync-icon-bundle.mjs
```

## Versioning

`versions.json` at the repo root is the single source of truth, and
`scripts/version.mjs` is the only thing that writes it or the manifests it
feeds. Hand-editing a version in `package.json`, `pubspec.yaml` or
`version.dart` fails `npm test`, which runs `--check` first.

```bash
npm run version:show                      # what the two apps are on
npm run version:check                     # are the manifests in step?
node scripts/version.mjs bugs mobile      # 0.1.0 -> 0.1.1, mobile only
node scripts/version.mjs features         # both apps
node scripts/version.mjs prod desktop
```

`1.2.3` is `prod.features.bugs`, and bumping a segment resets the ones below
it. There is a `/version-update` skill that wraps this.

**StageLink Core and StageLink Mobile version independently**, because they
ship independently: a phone-only fix should not move the installer's number,
and an operator and a performer will routinely be on different numbers. Each
app shows its own — the About page's "StageLink Core" field, and
`VersionLabel` on the mobile join screen and diagnostics sheet. The desktop
also shows each connected receiver's app version in the roster, which is how
an operator answers "what are you running?" without asking.

`crates/Cargo.toml`'s workspace version is neither app's number. The same
crates ship inside both, so it is the **engine** version and the script leaves
it alone; the About page shows it separately as "Audio service".

Android's `versionCode` must strictly increase, so `mobile.build` increments on
every mobile bump and is never derived from the version.

## Things worth knowing before you change them

**TypeScript is pinned to 6.x.** TypeScript 7 removed `typescript/lib/tsc`,
which `vue-tsc` requires, so `.vue` files cannot be typechecked at all on 7.
tsup's bundled `rollup-plugin-dts` also fails against it. Full typechecking is
worth more than being on the newest major.

**Declarations are emitted by `tsc`, not tsup.** See above.

**Icons must be listed, not just scanned.** Several icon names are chosen at
runtime, so Nuxt Icon's static scan misses them. They are enumerated in
`nuxt.config.ts` and kept in step by `scripts/sync-icon-bundle.mjs`. The
provider is `none` — fetching an icon from the internet is not an option on the
network StageLink is built for.

**The CSP pins inline script hashes in production.** Nuxt inlines a colour-mode
bootstrap and its runtime config into every HTML file. A policy without
`unsafe-inline` blocks them, `window.__NUXT__.config` is never assigned, and the
window renders blank with only a console violation to show for it.
`electron/main/csp.ts` hashes the scripts actually on disk instead.

**`isDevelopment` and `isPackaged` are functions.** `if (!isDevelopment)` is
valid TypeScript and always false. There is a test for this
(`test/build-mode-guards.test.ts`) because it already happened once and only
broke the packaged build.

**Do not exclude `node_modules` from the electron-builder `files` list.** tsup
externalises anything in `dependencies`, so `electron-log` is required at
runtime. Excluding node_modules produces a package that dies on its first
`require` — before logging exists, so the failure is completely silent.

**Nothing may be simulated.** If data is unavailable the UI shows an empty state.
There is no demo receiver, no placeholder level, no sample session. An operator
thirty seconds before a service has to be able to trust the screen.

## Current phase status

| Phase | State |
| --- | --- |
| 1 — Foundation | Complete |
| 2 — Real audio capture | Complete |
| 3 — Streaming proof of concept | Packetiser and queues done; UDP sender outstanding |
| 4 — Desktop management | Dashboard, audio, network, sessions, diagnostics, settings done; receiver management awaits Phase 5 |
| 5 — Security and reliability | Local IPC and secret storage done; pairing and encrypted transport outstanding |
| 6 — Windows release | Installer builds and the packaged app is verified; signing and performance testing outstanding |

Methods belonging to an incomplete phase return `NOT_IMPLEMENTED` naming it,
rather than a plausible-looking success.
