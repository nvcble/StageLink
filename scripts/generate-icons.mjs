#!/usr/bin/env node
/**
 * Generates Windows icon assets from the official StageLink logo.
 *
 * The source PNG in `assets/branding/StageLink-Logo.png` is the authority and
 * is never modified. Everything produced here goes into
 * `assets/branding/generated/`, which is gitignored — the logo is committed,
 * its derivatives are not.
 *
 * If the source is missing this script fails loudly rather than substituting a
 * placeholder. A silently-wrong application icon is the kind of thing that
 * ships.
 *
 *   node scripts/generate-icons.mjs
 */

import { existsSync, mkdirSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import { createRequire } from 'node:module'

const require = createRequire(import.meta.url)
const here = path.dirname(fileURLToPath(import.meta.url))
const repoRoot = path.resolve(here, '..')

const SOURCE = path.join(repoRoot, 'assets', 'branding', 'StageLink-Logo.png')
const OUTPUT_DIR = path.join(repoRoot, 'assets', 'branding', 'generated')

/**
 * Sizes embedded in the .ico.
 *
 * 16 and 32 are the taskbar and title bar; 48 is the Explorer medium view; 256
 * is what the installer and the Start menu use on a high-DPI display. Omitting
 * 256 is why some applications look blurry in the Start menu.
 */
const ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]

function fail(message) {
  console.error(`[icons] ${message}`)
  process.exit(1)
}

async function main() {
  if (!existsSync(SOURCE)) {
    fail(
      `The official StageLink logo is missing from ${path.relative(repoRoot, SOURCE)}.\n` +
        '        Place the supplied StageLink-Logo.png there and run this again. ' +
        'This script will not substitute a different image.',
    )
  }

  mkdirSync(OUTPUT_DIR, { recursive: true })

  // png-to-ico resizes internally and embeds every size in one .ico.
  let pngToIco
  try {
    // png-to-ico ships as a transpiled ES module, so the callable lives on
    // `.default` when required from CommonJS.
    const module = require('png-to-ico')
    pngToIco = typeof module === 'function' ? module : module.default
  } catch {
    fail('png-to-ico is not installed. Run `npm install` at the repository root.')
  }
  if (typeof pngToIco !== 'function') {
    fail('png-to-ico did not export a callable; the package layout may have changed.')
  }

  console.log(`[icons] source: ${path.relative(repoRoot, SOURCE)}`)

  const ico = await pngToIco(SOURCE, { sizes: ICO_SIZES })
  const icoPath = path.join(OUTPUT_DIR, 'icon.ico')
  writeFileSync(icoPath, ico)
  console.log(`[icons] wrote ${path.relative(repoRoot, icoPath)} (${ICO_SIZES.join(', ')} px)`)

  // The tray needs a small PNG; Electron resizes it at runtime but starting
  // from the full-resolution original keeps it crisp.
  const trayPath = path.join(OUTPUT_DIR, 'tray.png')
  const { copyFileSync } = await import('node:fs')
  copyFileSync(SOURCE, trayPath)
  console.log(`[icons] wrote ${path.relative(repoRoot, trayPath)}`)

  // electron-builder wants a 256×256 (or larger) PNG for the installer header
  // and for non-Windows targets.
  const appIconPath = path.join(OUTPUT_DIR, 'icon.png')
  copyFileSync(SOURCE, appIconPath)
  console.log(`[icons] wrote ${path.relative(repoRoot, appIconPath)}`)

  console.log('[icons] done. The source logo was not modified.')
}

main().catch((error) => {
  fail(error instanceof Error ? error.message : String(error))
})
