import { defineConfig } from 'tsup'

export default defineConfig({
  entry: ['src/index.ts'],
  // Dual output: the Nuxt renderer consumes ESM, the Electron main and the
  // sandboxed preload bundle are CommonJS.
  format: ['esm', 'cjs'],
  outExtension({ format }) {
    return { js: format === 'cjs' ? '.cjs' : '.js' }
  },
  // Declarations are emitted by `tsc` (see tsconfig.build.json) rather than by
  // tsup. tsup bundles its own rollup-plugin-dts pinned to TypeScript 5, which
  // cannot load against the TypeScript 7 compiler this repo uses.
  dts: false,
  sourcemap: true,
  clean: true,
  target: 'node22',
  treeshake: true,
})
