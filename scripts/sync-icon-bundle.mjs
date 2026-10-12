#!/usr/bin/env node
/**
 * Keeps the explicit icon bundle list in `nuxt.config.ts` in step with the
 * icons the interface actually references.
 *
 * Nuxt Icon's static scan cannot see icon names chosen at runtime — the
 * sidebar's navigation array, severity-driven alert icons, the session event
 * log — so every icon must be listed. On the isolated network StageLink is
 * designed for there is no icon API to fall back to: an unlisted icon simply
 * does not render.
 *
 *   node scripts/sync-icon-bundle.mjs          # rewrite the list
 *   node scripts/sync-icon-bundle.mjs --check  # fail if it is out of date
 */

import { readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const here = path.dirname(fileURLToPath(import.meta.url))
const repoRoot = path.resolve(here, '..')
const appDir = path.join(repoRoot, 'apps', 'desktop', 'app')
const configPath = path.join(repoRoot, 'apps', 'desktop', 'nuxt.config.ts')

const START = '      /* icon-bundle:start */'
const END = '      /* icon-bundle:end */'

/** Collects every `i-lucide-*` reference under the app directory. */
function collectIcons(directory) {
  const found = new Set()
  const walk = (dir) => {
    for (const entry of readdirSync(dir)) {
      const candidate = path.join(dir, entry)
      if (statSync(candidate).isDirectory()) {
        walk(candidate)
      } else if (/\.(vue|ts)$/.test(entry)) {
        const source = readFileSync(candidate, 'utf8')
        for (const match of source.matchAll(/i-lucide-([a-z0-9-]+)/g)) {
          found.add(`lucide:${match[1]}`)
        }
      }
    }
  }
  walk(directory)
  return [...found].sort()
}

const icons = collectIcons(appDir)
const block = icons.map((name) => `        '${name}',`).join('\n')
const replacement = `${START}\n${block}\n${END}`

let config = readFileSync(configPath, 'utf8')

const startIndex = config.indexOf(START)
const endIndex = config.indexOf(END)
if (startIndex < 0 || endIndex < 0) {
  console.error(
    `[icons] could not find the ${START.trim()} / ${END.trim()} markers in nuxt.config.ts`,
  )
  process.exit(1)
}

const current = config.slice(startIndex, endIndex + END.length)

if (process.argv.includes('--check')) {
  if (current === replacement) {
    console.log(`[icons] nuxt.config.ts lists all ${icons.length} referenced icons`)
    process.exit(0)
  }
  console.error(
    '[icons] the icon bundle list is out of date. Run `node scripts/sync-icon-bundle.mjs`.',
  )
  process.exit(1)
}

config = config.slice(0, startIndex) + replacement + config.slice(endIndex + END.length)
writeFileSync(configPath, config)
console.log(`[icons] wrote ${icons.length} icon name(s) into nuxt.config.ts`)
