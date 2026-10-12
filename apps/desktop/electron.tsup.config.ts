import { defineConfig } from 'tsup'

/**
 * Bundles the Electron main and preload processes.
 *
 * Nuxt owns the renderer build; this handles the two Node-side entry points.
 * Keeping them separate avoids fighting a plugin that wants to own Nuxt's Vite
 * config, and means a renderer HMR update never rebuilds the main process.
 *
 * Both outputs are CommonJS: a sandboxed preload must be CJS, and matching the
 * main process to it keeps one module format across the Node side.
 */
export default defineConfig({
  entry: {
    'main/index': 'electron/main/index.ts',
    'preload/index': 'electron/preload/index.ts',
  },
  outDir: 'dist-electron',
  format: ['cjs'],
  outExtension() {
    return { js: '.cjs' }
  },
  target: 'node22',
  platform: 'node',
  // Electron supplies these at runtime.
  external: ['electron'],
  // The protocol package is bundled in rather than resolved at runtime: the
  // sandboxed preload has no module resolution, and a packaged app should not
  // depend on node_modules layout.
  noExternal: ['@stagelink/shared-protocol', 'zod'],
  sourcemap: true,
  clean: true,
  splitting: false,
  dts: false,
  minify: false,
})
