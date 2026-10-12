import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vitest/config'

export default defineConfig({
  resolve: {
    alias: {
      '@stagelink/shared-protocol': fileURLToPath(
        new URL('../../packages/shared-protocol/src/index.ts', import.meta.url),
      ),
    },
  },
  test: {
    environment: 'node',
    include: ['test/**/*.test.ts'],
    // Electron is not available in a plain Node test process. The tests here
    // cover the logic that does not need it — protocol path resolution, URL
    // policy, the IPC contract surface — and the packaging smoke test checks
    // the built artefacts rather than running Electron.
    server: {
      deps: {
        inline: ['@stagelink/shared-protocol'],
      },
    },
  },
})
