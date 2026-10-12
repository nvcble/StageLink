---
name: version-update
description: Bump the StageLink desktop and/or mobile version. Use when the user asks to bump, raise, or set a version, cut a release, or runs /version-update with prod/features/bugs.
---

# Version update

Versions are `1.2.3`:

| Segment | Name | Bump it for |
| --- | --- | --- |
| 1 | `prod` | A production release — a breaking or milestone change |
| 2 | `features` | New functionality, backwards compatible |
| 3 | `bugs` | Fixes only |

Bumping a segment resets every segment below it: `features` takes `1.2.3` to
`1.3.0`, not `1.3.3`.

`versions.json` at the repo root is the **single source of truth**.
`scripts/version.mjs` is the only thing that writes it and the manifests it
feeds. Never hand-edit a version in `package.json`, `pubspec.yaml` or
`version.dart`.

## Usage

```
/version-update <prod|features|bugs> [desktop|mobile|all]
```

The target defaults to `all`. StageLink Core and StageLink Mobile version
**independently** — they ship separately, so a phone-only fix should not move
the installer's number. Prefer naming the target.

## Steps

1. Run the bump:

   ```bash
   node scripts/version.mjs <segment> [target]
   ```

   It prints each `was -> now` and every file it wrote. If the user gave no
   target and the change is clearly confined to one app, say which target you
   chose and why rather than silently bumping both.

2. Confirm everything is consistent:

   ```bash
   node scripts/version.mjs --check
   ```

3. Report the old and new version per app, and the files that changed. Do not
   commit — that is `/commit-push`.

## What is derived from what

| File | Fed from | Reaches the user as |
| --- | --- | --- |
| `apps/desktop/package.json` | `desktop.version` | `app.getVersion()` → preload `runtime.appVersion` → About page, "StageLink Core" |
| `apps/mobile/pubspec.yaml` | `mobile.version` + `mobile.build` | Android `versionName` / `versionCode` |
| `apps/mobile/lib/src/version.dart` | the same | `VersionLabel` on the join screen and in the diagnostics sheet, and the `appVersion` sent to the desktop roster |

The mobile build number increments on **every** mobile bump and never goes
down, because Android refuses an APK whose `versionCode` is not greater than
the installed one.

## Out of scope

`crates/Cargo.toml`'s workspace version is the **engine** version — the same
code ships inside both apps, so it is neither app's number and this skill does
not touch it. The About page shows it separately as "Audio service". The root
`package.json` version is the monorepo wrapper and is not shown to anyone.

If asked to add a third versioned artefact, add a case to `render()` in
`scripts/version.mjs` and a row to the table above. Nothing else should learn
how to write a version.
