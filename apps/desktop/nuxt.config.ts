import { defineNuxtConfig } from 'nuxt/config'

/**
 * Nuxt 4 configuration for StageLink Core.
 *
 * This is a desktop renderer, not a web server. The important consequences:
 *
 * - `ssr: false` — there is no server at runtime. Nuxt generates static,
 *   client-rendered assets that Electron serves over the `stagelink://`
 *   protocol.
 * - `baseURL: '/'` with a real custom-protocol origin, so absolute asset URLs
 *   and history-mode routing both work after packaging. (This is why the app
 *   is not loaded from `file://`.)
 * - Prerendering every route means a refresh or direct navigation lands on a
 *   real `index.html` instead of a 404, with the protocol handler falling back
 *   to the SPA entry for anything it has not prerendered.
 */
export default defineNuxtConfig({
  ssr: false,

  compatibilityDate: '2025-10-01',

  typescript: {
    strict: true,
    // Checked by `npm run typecheck` rather than on every dev-server start, so
    // HMR stays fast while editing a live audio UI.
    typeCheck: false,
  },

  modules: ['@pinia/nuxt', '@nuxt/ui', '@vueuse/nuxt'],

  css: ['~/assets/css/main.css'],

  app: {
    baseURL: '/',
    buildAssetsDir: '/_nuxt/',
    head: {
      title: 'StageLink Core',
      htmlAttrs: { lang: 'en', class: 'dark' },
      meta: [
        { charset: 'utf-8' },
        { name: 'viewport', content: 'width=device-width, initial-scale=1' },
        { name: 'color-scheme', content: 'dark' },
      ],
    },
  },

  // StageLink ships dark-first. Colour mode is still available for the light
  // theme offered in Settings, but it must never flash light on launch.
  colorMode: {
    preference: 'dark',
    fallback: 'dark',
    classSuffix: '',
  },

  ui: {
    // Nuxt UI's colour aliases, mapped onto the StageLink palette defined in
    // assets/css/main.css.
    theme: {
      colors: ['primary', 'secondary', 'success', 'warning', 'error', 'neutral'],
    },
  },

  icon: {
    /**
     * Icons must be compiled into the bundle, never fetched at runtime.
     *
     * Nuxt Icon's default provider fetches from api.iconify.design on demand.
     * On the deployment StageLink is designed for — a dedicated access point
     * with no internet uplink — that means every icon in the interface silently
     * fails to appear. The renderer's `connect-src` policy blocks the request
     * anyway, so the only correct answer is to ship them.
     *
     * `@iconify-json/lucide` provides the collection locally and `clientBundle`
     * compiles the ones actually used into the client build.
     */
    provider: 'none',
    clientBundle: {
      // `scan` alone is not enough: several icon names are chosen at runtime
      // (the sidebar navigation array, severity-driven alert icons, the session
      // event log), and a static scan cannot see those. Every icon the
      // interface can render is therefore listed explicitly, kept in step by
      // `node scripts/sync-icon-bundle.mjs`.
      scan: true,
      icons: [
      /* icon-bundle:start */
        'lucide:activity',
        'lucide:arrow-right',
        'lucide:audio-waveform',
        'lucide:book-open',
        'lucide:check',
        'lucide:chevron-down',
        'lucide:chevron-up',
        'lucide:circle',
        'lucide:circle-alert',
        'lucide:circle-check',
        'lucide:circle-dashed',
        'lucide:circle-dot',
        'lucide:circle-pause',
        'lucide:circle-play',
        'lucide:circle-x',
        'lucide:construction',
        'lucide:cpu',
        'lucide:database',
        'lucide:ethernet-port',
        'lucide:external-link',
        'lucide:eye-off',
        'lucide:file-audio',
        'lucide:file-text',
        'lucide:folder-open',
        'lucide:history',
        'lucide:inbox',
        'lucide:info',
        'lucide:layout-dashboard',
        'lucide:list-checks',
        'lucide:list-x',
        'lucide:mic-off',
        'lucide:network',
        'lucide:octagon-alert',
        'lucide:package',
        'lucide:palette',
        'lucide:pencil',
        'lucide:play',
        'lucide:play-circle',
        'lucide:power',
        'lucide:qr-code',
        'lucide:radio',
        'lucide:refresh-cw',
        'lucide:rotate-ccw',
        'lucide:scroll-text',
        'lucide:search-check',
        'lucide:server',
        'lucide:server-off',
        'lucide:settings',
        'lucide:settings-2',
        'lucide:shield',
        'lucide:shield-alert',
        'lucide:shield-off',
        'lucide:shuffle',
        'lucide:sliders-horizontal',
        'lucide:smartphone',
        'lucide:sparkles',
        'lucide:square',
        'lucide:stethoscope',
        'lucide:timer',
        'lucide:triangle-alert',
        'lucide:unplug',
        'lucide:volume-x',
        'lucide:wand-sparkles',
      /* icon-bundle:end */
      ],
      sizeLimitKb: 1024,
    },
  },

  nitro: {
    // Every page is prerendered so production routing does not depend on a
    // server or on protocol-handler guesswork.
    prerender: {
      crawlLinks: false,
      routes: [
        '/',
        '/audio',
        '/receivers',
        '/network',
        '/sessions',
        '/diagnostics',
        '/settings',
        '/about',
        '/setup',
      ],
    },
  },

  // No telemetry from a device that is meant to run on an isolated network.
  telemetry: false,

  devtools: { enabled: false },

  vite: {
    optimizeDeps: {
      // zod is imported by @stagelink/shared-protocol, which is a workspace
      // source dependency rather than a published package. Pre-bundling it up
      // front avoids Vite discovering it mid-session and triggering a full
      // page reload — which, in a desktop app, looks like the UI falling over.
      include: ['zod'],
    },
    // Electron serves the bundle from a custom protocol; relative sourcemap
    // paths keep stack traces readable in the detached devtools.
    build: {
      sourcemap: true,
    },
    server: {
      // The Electron window connects to this exact origin; see paths.ts.
      strictPort: true,
    },
  },

  experimental: {
    // Payload extraction assumes a static host that can serve `_payload.json`
    // alongside each route. The custom protocol can, but there is nothing to
    // gain in a local app and it complicates the protocol handler.
    payloadExtraction: false,
  },
})
