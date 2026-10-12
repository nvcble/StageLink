import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    environment: 'node',
    include: ['test/**/*.test.ts'],
    typecheck: {
      // `expectTypeOf` assertions in the protocol tests guard the inference
      // chain from the method registry through to the renderer.
      enabled: false,
    },
  },
})
