import { describe, expect, it, vi } from 'vitest'

/**
 * Security configuration tests.
 *
 * Electron is not available in a plain Node test process, so the pieces it
 * needs are stubbed. What is actually under test is StageLink's own policy
 * decisions: which URLs may be loaded in-app, which may be handed to the system
 * browser, and the webPreferences the window is created with.
 */

const openedExternally: string[] = []

vi.mock('electron', () => ({
  app: {
    isPackaged: false,
    getAppPath: () => 'C:/stagelink/apps/desktop',
    getPath: () => 'C:/Users/test/AppData/Roaming',
    getVersion: () => '0.1.0',
  },
  shell: {
    openExternal: async (url: string) => {
      openedExternally.push(url)
    },
  },
  BrowserWindow: class {},
  protocol: { registerSchemesAsPrivileged: () => {}, handle: () => {} },
}))

vi.mock('../electron/main/logger', () => ({
  logger: { info: () => {}, warn: () => {}, error: () => {}, debug: () => {} },
  logServiceLine: () => {},
  initLogging: () => {},
}))

const { isPermittedUrl, openExternalUrl } = await import('../electron/main/window')

describe('in-app navigation policy', () => {
  it('permits the application protocol', () => {
    expect(isPermittedUrl('stagelink://app/')).toBe(true)
    expect(isPermittedUrl('stagelink://app/receivers')).toBe(true)
    expect(isPermittedUrl('stagelink://app/_nuxt/entry.js')).toBe(true)
  })

  it('permits the dev server in development', () => {
    // `app.isPackaged` is false in this stub, so the build is development.
    expect(isPermittedUrl('http://127.0.0.1:3579/')).toBe(true)
    expect(isPermittedUrl('ws://127.0.0.1:3579/_nuxt/hmr')).toBe(true)
  })

  it('refuses a different port on localhost', () => {
    // Another local service must not be loadable into the window.
    expect(isPermittedUrl('http://127.0.0.1:8080/')).toBe(false)
    expect(isPermittedUrl('http://localhost:3579/')).toBe(false)
  })

  it('refuses remote origins', () => {
    expect(isPermittedUrl('https://example.com/')).toBe(false)
    expect(isPermittedUrl('http://example.com/')).toBe(false)
    expect(isPermittedUrl('https://127.0.0.1.evil.com/')).toBe(false)
  })

  it('refuses file and other local schemes', () => {
    expect(isPermittedUrl('file:///C:/Windows/System32/calc.exe')).toBe(false)
    expect(isPermittedUrl('smb://share/payload.exe')).toBe(false)
    expect(isPermittedUrl('about:blank')).toBe(false)
    expect(isPermittedUrl('javascript:alert(1)')).toBe(false)
    expect(isPermittedUrl('data:text/html,<script>alert(1)</script>')).toBe(false)
  })

  it('refuses an unparseable URL rather than letting it through', () => {
    expect(isPermittedUrl('')).toBe(false)
    expect(isPermittedUrl('not a url')).toBe(false)
    expect(isPermittedUrl('///')).toBe(false)
  })
})

describe('external URL policy', () => {
  it('opens an https URL in the system browser', async () => {
    openedExternally.length = 0
    expect(await openExternalUrl('https://stagelink.example/docs')).toBe(true)
    expect(openedExternally).toEqual(['https://stagelink.example/docs'])
  })

  it('opens a mailto URL', async () => {
    openedExternally.length = 0
    expect(await openExternalUrl('mailto:help@stagelink.example')).toBe(true)
    expect(openedExternally).toHaveLength(1)
  })

  it('refuses to hand a local path to the shell', async () => {
    openedExternally.length = 0
    // shell.openExternal on a file: URL launches local content.
    expect(await openExternalUrl('file:///C:/Windows/System32/calc.exe')).toBe(false)
    expect(await openExternalUrl('smb://attacker/payload.exe')).toBe(false)
    expect(openedExternally).toHaveLength(0)
  })

  it('refuses plain http, so nothing is sent in the clear on our prompting', async () => {
    openedExternally.length = 0
    expect(await openExternalUrl('http://example.com')).toBe(false)
    expect(openedExternally).toHaveLength(0)
  })

  it('refuses script and data URLs', async () => {
    openedExternally.length = 0
    expect(await openExternalUrl('javascript:alert(1)')).toBe(false)
    expect(await openExternalUrl('data:text/html,<h1>x</h1>')).toBe(false)
    expect(openedExternally).toHaveLength(0)
  })

  it('refuses an unparseable URL', async () => {
    expect(await openExternalUrl('nonsense')).toBe(false)
    expect(await openExternalUrl('')).toBe(false)
  })
})

describe('window security settings', () => {
  it('pins the webPreferences the window must be created with', async () => {
    // Asserted against the source rather than a live BrowserWindow, because
    // these are the settings that cannot be allowed to drift. Reading the file
    // is crude but it catches the one mistake that matters: somebody turning
    // nodeIntegration on to make something work.
    const { readFileSync } = await import('node:fs')
    const source = readFileSync(
      new URL('../electron/main/window.ts', import.meta.url),
      'utf8',
    )

    expect(source).toContain('contextIsolation: true')
    expect(source).toContain('nodeIntegration: false')
    expect(source).toContain('nodeIntegrationInWorker: false')
    expect(source).toContain('nodeIntegrationInSubFrames: false')
    expect(source).toContain('sandbox: true')
    expect(source).toContain('webSecurity: true')
    expect(source).toContain('allowRunningInsecureContent: false')

    expect(source).not.toMatch(/nodeIntegration:\s*true/)
    expect(source).not.toMatch(/contextIsolation:\s*false/)
    expect(source).not.toMatch(/webSecurity:\s*false/)
    expect(source).not.toMatch(/sandbox:\s*false/)
  })

  it('denies renderer permissions apart from the validated openExternal', async () => {
    const { readFileSync } = await import('node:fs')
    const source = readFileSync(
      new URL('../electron/main/window.ts', import.meta.url),
      'utf8',
    )
    expect(source).toContain('setPermissionRequestHandler')
    expect(source).toContain('setPermissionCheckHandler')

    // Audio capture happens in the Rust service, so the renderer must never be
    // granted a microphone — or a camera, clipboard or geolocation fix.
    const allowlist = /ALLOWED_PERMISSIONS = new Set\(\[([^\]]*)\]\)/.exec(source)
    expect(allowlist, 'the permission allowlist must be explicit').not.toBeNull()
    const allowed = allowlist![1]
    expect(allowed).toContain('openExternal')
    for (const forbidden of ['media', 'microphone', 'camera', 'geolocation', 'clipboard']) {
      expect(allowed, `${forbidden} must not be allowed`).not.toContain(forbidden)
    }
  })

  it('applies the policy as a response header', async () => {
    const { readFileSync } = await import('node:fs')
    const source = readFileSync(
      new URL('../electron/main/window.ts', import.meta.url),
      'utf8',
    )
    // A header cannot be removed by anything running in the page, unlike a
    // meta tag.
    expect(source).toContain('Content-Security-Policy')
    expect(source).toContain('onHeadersReceived')
  })
})

describe('content security policy', () => {
  it('locks production down with no remote sources and no unsafe-inline scripts', async () => {
    const { buildContentSecurityPolicy } = await import('../electron/main/csp')
    const policy = buildContentSecurityPolicy({
      development: false,
      inlineScriptHashes: ["'sha256-abc123'"],
    })

    expect(policy).toContain("default-src 'none'")
    expect(policy).toContain("object-src 'none'")
    expect(policy).toContain("base-uri 'none'")
    expect(policy).toContain("form-action 'none'")
    expect(policy).toContain("frame-ancestors 'none'")

    // Inline scripts are permitted by hash, never by unsafe-inline.
    expect(policy).toContain("'sha256-abc123'")
    expect(policy).not.toContain("'unsafe-inline' 'unsafe-eval'")
    const scriptSrc = /script-src ([^;]*)/.exec(policy)?.[1] ?? ''
    expect(scriptSrc).not.toContain('unsafe-inline')
    expect(scriptSrc).not.toContain('unsafe-eval')
    expect(scriptSrc).not.toContain('http://')
    expect(scriptSrc).not.toContain('https://')

    // StageLink needs no internet access for live audio, so the renderer is
    // given no way to reach the network.
    expect(policy).toContain("connect-src 'self'")
    expect(policy).not.toMatch(/connect-src[^;]*https?:/)
  })

  it('relaxes only development, and only for the dev server', async () => {
    const { buildContentSecurityPolicy } = await import('../electron/main/csp')
    const policy = buildContentSecurityPolicy({ development: true })

    // Vite's HMR client evaluates code and the dev server regenerates inline
    // scripts per request, so neither can be hashed.
    const scriptSrc = /script-src ([^;]*)/.exec(policy)?.[1] ?? ''
    expect(scriptSrc).toContain('unsafe-eval')
    expect(scriptSrc).toContain('127.0.0.1:3579')
    // Still nothing off-machine.
    expect(scriptSrc).not.toMatch(/https:\/\/(?!127\.0\.0\.1)/)
  })

  it('hashes the inline scripts Nuxt actually emits', async () => {
    const { extractInlineScripts, scriptHash } = await import('../electron/main/csp')

    const html = `<!doctype html><html><head>
      <script type="importmap">{"imports":{}}</script>
      <script src="/_nuxt/entry.js"></script>
      <script>window.__NUXT__={};window.__NUXT__.config={}</script>
    </head><body></body></html>`

    const bodies = extractInlineScripts(html)
    // The two inline blocks, not the external one.
    expect(bodies).toHaveLength(2)
    expect(bodies[1]).toContain('__NUXT__')

    const hash = scriptHash('window.__NUXT__={}')
    expect(hash).toMatch(/^'sha256-[A-Za-z0-9+/]+=*'$/)
    // Deterministic, so the policy is stable for a given build.
    expect(scriptHash('window.__NUXT__={}')).toBe(hash)
    expect(scriptHash('window.__NUXT__={} ')).not.toBe(hash)
  })

  it('ignores a script tag that only has a src', async () => {
    const { extractInlineScripts } = await import('../electron/main/csp')
    expect(extractInlineScripts('<script src="/a.js"></script>')).toEqual([])
    expect(extractInlineScripts('<script defer src="/a.js"></script>')).toEqual([])
  })
})

describe('the preload surface', () => {
  it('exposes nothing beyond the named bridge', async () => {
    const { readFileSync } = await import('node:fs')
    const source = readFileSync(
      new URL('../electron/preload/index.ts', import.meta.url),
      'utf8',
    )
    // Exactly one thing is exposed to the page.
    const exposures = source.match(/exposeInMainWorld/g) ?? []
    expect(exposures).toHaveLength(1)
    expect(source).toContain("exposeInMainWorld('stagelink', bridge)")

    // And none of these ever reach the renderer.
    expect(source).not.toMatch(/exposeInMainWorld\(\s*['"]ipcRenderer/)
    expect(source).not.toMatch(/exposeInMainWorld\(\s*['"]require/)
    expect(source).not.toMatch(/exposeInMainWorld\(\s*['"]process/)
    expect(source).not.toContain('nodeRequire')
  })
})
