import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vitest/config'

export default defineConfig({
  resolve: {
    alias: {
      '@stagelink/shared-protocol': fileURLToPath(
        new URL('../packages/shared-protocol/src/index.ts', import.meta.url),
      ),
    },
  },
  test: {
    environment: 'node',
    include: ['**/*.test.ts'],
    // The protocol tests spawn a Rust process; give the suite room without
    // letting a hung probe stall CI indefinitely.
    testTimeout: 30_000,
    hookTimeout: 30_000,
    globalSetup: ['./protocol/global-setup.ts'],
  },
})
