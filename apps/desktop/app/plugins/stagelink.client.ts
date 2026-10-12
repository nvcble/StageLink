import type { StageLinkBridge } from '@stagelink/shared-protocol'

/**
 * Initialises the typed Electron bridge.
 *
 * Client-only by filename: there is no server in this application, and a
 * server-side render would have no `window` to find the bridge on.
 *
 * The plugin does three things:
 *
 * 1. Verifies the application really is running inside Electron. Opening the
 *    Nuxt dev server in a normal browser is a legitimate thing to do while
 *    working on layout, and it should produce a clear explanation rather than
 *    a cascade of "cannot read property of undefined".
 * 2. Exposes the bridge through `useNuxtApp().$stagelink`, typed.
 * 3. Wires service events into the Pinia stores once, here, and tears the
 *    subscriptions down on app unmount. Components never subscribe to raw
 *    events; they read stores.
 */
export default defineNuxtPlugin({
  name: 'stagelink-bridge',
  // Deliberately *not* `enforce: 'pre'`. A pre-plugin runs before module
  // plugins, which means before `@pinia/nuxt` has installed Pinia — and every
  // `useXStore()` below would then throw "getActivePinia was called with no
  // active Pinia". The default ordering runs after module plugins and still
  // completes before any component mounts, which is what this needs.
  dependsOn: ['pinia'],

  async setup(nuxtApp) {
    const bridge = window.stagelink

    if (!bridge) {
      // Not an error worth throwing: the UI renders a plain explanation and the
      // rest of the app degrades to read-only.
      console.warn(
        '[stagelink] window.stagelink is not available. This page is not running inside ' +
          'StageLink Core, so no audio hardware or service state is reachable.',
      )
      nuxtApp.provide('stagelink', null as StageLinkBridge | null)
      return
    }

    // Provided *before* anything below runs, not by returning it at the end.
    // `useStageLinkBridge()` reads `$stagelink`, so a store action called
    // during this setup would otherwise see `undefined` and fail with
    // SERVICE_UNAVAILABLE — which is exactly what happened to the initial
    // loads: every store stayed empty until some page refetched on mount, so a
    // configured adapter and a chosen input looked unset on first launch.
    nuxtApp.provide('stagelink', bridge as StageLinkBridge | null)

    const service = useServiceStore()
    const audio = useAudioStore()
    const streaming = useStreamingStore()
    const receivers = useReceiverStore()
    const network = useNetworkStore()
    const session = useSessionStore()
    const settings = useSettingsStore()
    const diagnostics = useDiagnosticsStore()

    service.setRuntime(bridge.runtime)

    /**
     * Loads everything the shell needs, in the order it needs it.
     *
     * Called once at startup and again whenever the service becomes usable —
     * after a crash and restart, or simply because the renderer won the race on
     * first launch. `allSettled` because one unavailable store must not stop
     * the others, and each action records its own error.
     */
    async function loadInitialState() {
      // Settings first: other views read display preferences from them.
      await settings.load()
      await Promise.allSettled([
        audio.refreshStatus(),
        streaming.refreshStatus(),
        network.refresh(),
        session.refreshCurrent(),
        receivers.refresh(),
      ])
    }

    // -- event subscriptions ----------------------------------------------
    // Every `on` returns an unsubscribe function; they are all collected so the
    // teardown below cannot miss one.
    const unsubscribes: Array<() => void> = [
      bridge.on('audio.levels', (payload) => audio.applyLevels(payload)),
      bridge.on('audio.status', (payload) => audio.applyStatus(payload)),
      bridge.on('stream.status', (payload) => streaming.applyStatus(payload)),
      bridge.on('receivers.changed', (payload) => receivers.applyReceivers(payload.receivers)),
      bridge.on('receivers.joinCode', (payload) => receivers.applyJoinCode(payload)),
      bridge.on('network.status', (payload) => network.applyStatus(payload)),
      bridge.on('session.state', (payload) => session.applySession(payload)),
      bridge.on('session.event', (payload) => session.appendEvent(payload)),
      bridge.on('diagnostics.snapshot', (payload) => diagnostics.applySnapshot(payload)),
      bridge.on('service.state', (payload) => {
        const wasUsable = service.isUsable
        service.applyInfo(payload)
        // The renderer starts before the service finishes its handshake, so the
        // initial loads below can all fail with SERVICE_UNAVAILABLE. Nothing
        // retried them, and most state is only pushed when it *changes* — so a
        // selected adapter and a chosen input stayed invisible until the
        // operator happened to open the page that refetches them. On stage that
        // reads as StageLink having lost its configuration.
        if (!wasUsable && service.isUsable) void loadInitialState()
      }),
      bridge.on('service.error', (payload) => service.appendError(payload)),
    ]

    // A renderer reload is the only thing that ends this page's life, so
    // `beforeunload` is the correct teardown point. (Nuxt has no
    // `app:unmounted` runtime hook, and an `onScopeDispose` here would run
    // inside the plugin's own scope rather than the app's.)
    const teardown = () => {
      for (const unsubscribe of unsubscribes) unsubscribe()
    }
    window.addEventListener('beforeunload', teardown, { once: true })
    nuxtApp.hook('app:error', teardown)

    // -- initial state ------------------------------------------------------
    // The service also pushes a snapshot on connect, but the renderer may have
    // loaded after that happened (a reload mid-session), so it asks once here.
    service.applyInfo(await bridge.getServiceInfo())

    await loadInitialState()
  },
})
