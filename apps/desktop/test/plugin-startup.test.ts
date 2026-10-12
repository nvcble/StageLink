import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

/**
 * Guards the one ordering constraint in the bridge plugin.
 *
 * `useStageLinkBridge()` reads `$stagelink` off the Nuxt app. A plugin that
 * supplies it by *returning* `{ provide: { stagelink } }` has not provided it
 * until its setup resolves — so every store action the plugin itself awaits
 * sees `undefined` and fails with SERVICE_UNAVAILABLE.
 *
 * This actually happened. The initial loads all failed silently (they are
 * `allSettled`, and each store records its own error), so on first launch the
 * application showed its unconfigured empty states: no selected adapter, no
 * selected input, no join QR — until the operator happened to open a page that
 * refetched on mount. Nothing failed loudly, the typechecker was happy, and
 * every unit test passed because none of them went through the plugin.
 *
 * Asserted against the source text because the failure is one of *ordering*
 * inside a Nuxt plugin setup, which is not observable from the module's
 * exports.
 */

const pluginPath = fileURLToPath(
  new URL('../app/plugins/stagelink.client.ts', import.meta.url),
)
const source = readFileSync(pluginPath, 'utf8')

describe('the bridge plugin provides before it loads', () => {
  it('provides the bridge imperatively rather than by returning it', () => {
    expect(source).toContain("nuxtApp.provide('stagelink'")
    expect(
      /return\s*\{\s*provide\s*:/.test(source),
      'returning the provide object defers it until setup resolves',
    ).toBe(false)
  })

  it('provides the bridge before the first store load', () => {
    const provided = source.indexOf("nuxtApp.provide('stagelink', bridge")
    const loaded = source.indexOf('await loadInitialState()')
    expect(provided, 'the bridge must be provided for the real-bridge path').toBeGreaterThan(-1)
    expect(loaded, 'the plugin must still perform the initial load').toBeGreaterThan(-1)
    expect(provided).toBeLessThan(loaded)
  })

  it('reloads state when the service becomes usable, not only at startup', () => {
    // The renderer can win the race against the service handshake, and most
    // state is pushed only when it changes — so without this a failed startup
    // load is never retried.
    expect(source).toMatch(/isUsable\)\s*void loadInitialState\(\)/)
  })
})
