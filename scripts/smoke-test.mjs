#!/usr/bin/env node
/**
 * Desktop smoke test.
 *
 * Launches StageLink Core, attaches to the renderer over the Chrome DevTools
 * Protocol, and asserts that the application actually came up: the shell
 * rendered, the preload bridge is present, the Rust service connected, and real
 * device and adapter data arrived. Then it captures a PNG of the *application
 * window only* — not the screen — so the result can be looked at.
 *
 * This is the test that cannot be faked by unit tests: it exercises Electron,
 * the custom protocol or dev server, the preload bridge, the named-pipe
 * handshake and the Rust service together.
 *
 *   node scripts/smoke-test.mjs              # against the Nuxt dev server
 *   node scripts/smoke-test.mjs --production # against the generated bundle
 */

import { spawn } from 'node:child_process'
import { mkdirSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const here = path.dirname(fileURLToPath(import.meta.url))
const repoRoot = path.resolve(here, '..')
const desktopDir = path.join(repoRoot, 'apps', 'desktop')
const outputDir = path.join(repoRoot, 'tests', 'integration', 'artifacts')

const production = process.argv.includes('--production')
const DEBUG_PORT = 9229

let failures = 0
const check = (label, condition, detail = '') => {
  if (condition) {
    console.log(`PASS  ${label}`)
  } else {
    failures += 1
    console.log(`FAIL  ${label}${detail ? ` — ${detail}` : ''}`)
  }
}

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

/** Finds the renderer target, retrying while Electron starts. */
async function findRendererTarget(attempts = 90) {
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      const response = await fetch(`http://127.0.0.1:${DEBUG_PORT}/json/list`)
      const targets = await response.json()
      const page = targets.find(
        (target) => target.type === 'page' && !target.url.startsWith('devtools://'),
      )
      if (page?.webSocketDebuggerUrl) return page
    } catch {
      // Electron is not listening yet.
    }
    await delay(500)
  }
  throw new Error('the renderer never appeared on the debugging port')
}

/** Minimal CDP client. */
class Cdp {
  constructor(socket) {
    this.socket = socket
    this.nextId = 1
    this.pending = new Map()
    socket.addEventListener('message', (event) => {
      const message = JSON.parse(event.data)
      const entry = this.pending.get(message.id)
      if (!entry) return
      this.pending.delete(message.id)
      if (message.error) entry.reject(new Error(message.error.message))
      else entry.resolve(message.result)
    })
  }

  static async connect(url) {
    const socket = new WebSocket(url)
    await new Promise((resolve, reject) => {
      socket.addEventListener('open', resolve, { once: true })
      socket.addEventListener('error', () => reject(new Error('CDP connect failed')), {
        once: true,
      })
    })
    return new Cdp(socket)
  }

  send(method, params = {}) {
    const id = this.nextId++
    const answered = new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject })
      setTimeout(() => {
        if (this.pending.delete(id)) reject(new Error(`${method} timed out`))
      }, 30_000)
    })
    this.socket.send(JSON.stringify({ id, method, params }))
    return answered
  }

  /** Evaluates an expression in the page and returns its value. */
  async evaluate(expression) {
    // Runtime.evaluate has no top-level await, so an expression that uses it
    // is wrapped in an async IIFE and resolved through awaitPromise.
    const wrapped = expression.includes('await ')
      ? `(async () => { return (${expression}) })()`
      : expression
    const result = await this.send('Runtime.evaluate', {
      expression: wrapped,
      awaitPromise: true,
      returnByValue: true,
    })
    if (result.exceptionDetails) {
      throw new Error(result.exceptionDetails.text ?? 'evaluation threw')
    }
    return result.result.value
  }
}

/**
 * Navigates to a route the way an operator does: by clicking the sidebar link.
 *
 * `history.pushState` plus a synthetic `popstate` changes the URL without
 * reliably driving Vue Router's render, which produces the worst kind of test
 * result — a passing URL assertion over a page that never rendered. Clicking
 * the real link exercises the real path.
 */
async function navigate(cdp, route) {
  const clicked = await cdp.evaluate(
    `(() => {
      const link = document.querySelector('nav a[href="${route}"]')
      if (!link) return false
      link.click()
      return true
    })()`,
  )
  if (clicked !== true) {
    throw new Error(`no sidebar link for ${route}`)
  }
}

async function main() {
  mkdirSync(outputDir, { recursive: true })

  console.log(`[smoke] launching StageLink Core (${production ? 'production' : 'development'})`)

  // In production the *packaged* executable is launched rather than Electron
  // pointed at a source directory. `process.resourcesPath` only resolves to the
  // bundled renderer and service inside a real package, and verifying that
  // resolution — along with the stagelink:// protocol handler — is the whole
  // point of the production run.
  const executable = production
    ? path.join(desktopDir, 'release', 'win-unpacked', 'StageLink Core.exe')
    : path.join(repoRoot, 'node_modules', 'electron', 'dist', 'electron.exe')

  const electron = spawn(
    executable,
    production
      ? [`--remote-debugging-port=${DEBUG_PORT}`]
      : ['.', `--remote-debugging-port=${DEBUG_PORT}`],
    {
      cwd: production ? path.dirname(executable) : desktopDir,
      env: {
        ...process.env,
        NODE_ENV: production ? 'production' : 'development',
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    },
  )

  const logLines = []
  const capture = (chunk) => {
    const text = chunk.toString()
    logLines.push(text)
    if (process.env.STAGELINK_SMOKE_VERBOSE) process.stdout.write(text)
  }
  electron.stdout.on('data', capture)
  electron.stderr.on('data', capture)

  try {
    const target = await findRendererTarget()
    console.log(`[smoke] attached to ${target.url}`)
    const cdp = await Cdp.connect(target.webSocketDebuggerUrl)
    await cdp.send('Runtime.enable')
    await cdp.send('Page.enable')

    // Wait for the shell to mount rather than guessing at a delay. The
    // plugin fetches initial state before the first component renders, so the
    // sidebar appearing means the whole startup path completed.
    const mounted = await (async () => {
      for (let attempt = 0; attempt < 60; attempt += 1) {
        const links = await cdp.evaluate('document.querySelectorAll("nav a").length')
        if (Number(links) > 0) return true
        await delay(500)
      }
      return false
    })()
    check('the application shell mounted', mounted)

    // -- the shell rendered ------------------------------------------------
    const title = await cdp.evaluate('document.title')
    check('the window has the application title', String(title).includes('StageLink'), String(title))

    const navItems = await cdp.evaluate(
      'Array.from(document.querySelectorAll("nav a")).map(a => a.textContent.trim())',
    )
    check(
      'the sidebar renders every page link',
      Array.isArray(navItems) && navItems.length === 8,
      JSON.stringify(navItems),
    )
    for (const expected of ['Dashboard', 'Audio Input', 'Receivers', 'Network', 'Diagnostics']) {
      check(`the sidebar links to ${expected}`, navItems.includes(expected))
    }

    const logoSrc = await cdp.evaluate(
      'document.querySelector("img[alt=\'StageLink\']")?.getAttribute("src") ?? null',
    )
    check('the official logo is rendered', logoSrc === '/stagelink-logo.png', String(logoSrc))

    const logoLoaded = await cdp.evaluate(
      '(() => { const img = document.querySelector("img[alt=\'StageLink\']"); return img ? img.complete && img.naturalWidth > 0 : false })()',
    )
    check('the logo image actually loaded', logoLoaded === true)

    // -- the bridge is present and narrow ---------------------------------
    const bridgeKeys = await cdp.evaluate(
      'window.stagelink ? Object.keys(window.stagelink).sort() : null',
    )
    check(
      'the preload bridge is exposed',
      Array.isArray(bridgeKeys),
      bridgeKeys === null ? 'window.stagelink is missing' : '',
    )
    check(
      'the bridge exposes exactly the documented surface',
      JSON.stringify(bridgeKeys) ===
        JSON.stringify([
          'getServiceInfo',
          'invoke',
          'on',
          'openExternal',
          'restartService',
          'revealPath',
          'runtime',
          'window',
        ]),
      JSON.stringify(bridgeKeys),
    )

    const leaked = await cdp.evaluate(
      'JSON.stringify({ require: typeof window.require, process: typeof window.process, ipcRenderer: typeof window.ipcRenderer, Buffer: typeof window.Buffer })',
    )
    const leakedShape = JSON.parse(String(leaked))
    check('require is not exposed to the renderer', leakedShape.require === 'undefined')
    check('ipcRenderer is not exposed to the renderer', leakedShape.ipcRenderer === 'undefined')
    check('Buffer is not exposed to the renderer', leakedShape.Buffer === 'undefined')
    if (production) {
      // Vite defines a `process.env` shim in development. A packaged build is
      // served from the custom protocol with no such shim, and must not have
      // one — that would be a real leak.
      check(
        'process is not exposed in a packaged build',
        leakedShape.process === 'undefined',
        String(leaked),
      )
    } else {
      console.log(
        'SKIP  process shim check (Vite defines one in development: ' +
          leakedShape.process +
          ')',
      )
    }

    // -- the service connected --------------------------------------------
    const serviceInfo = await cdp.evaluate('JSON.stringify(await window.stagelink.getServiceInfo())')
    const service = JSON.parse(String(serviceInfo))
    check('the audio service reached the ready state', service.state === 'ready', service.state)
    check('the service reported its version', typeof service.version === 'string' && service.version.length > 0)
    const rendererProtocol = await cdp.evaluate(
      'window.stagelink.runtime.controlProtocolVersion',
    )
    check(
      'the renderer and the service agree on the control protocol',
      service.controlProtocolVersion === rendererProtocol,
      `service=${service.controlProtocolVersion} renderer=${rendererProtocol}`,
    )
    check('WASAPI is the available backend', service.availableAudioApis.includes('wasapi'))
    check('ASIO is not advertised', !service.availableAudioApis.includes('asio'))

    // -- real data over the bridge ----------------------------------------
    const devices = JSON.parse(
      String(
        await cdp.evaluate(
          'JSON.stringify(await window.stagelink.invoke("audio.listDevices", { probeFormats: false }))',
        ),
      ),
    )
    check('real audio endpoints were enumerated', Array.isArray(devices.devices))
    console.log(`[smoke] ${devices.devices.length} capture endpoint(s) via the bridge`)
    check(
      'every endpoint carries an id and a name',
      devices.devices.every((device) => device.id && device.name),
    )

    const network = JSON.parse(
      String(await cdp.evaluate('JSON.stringify(await window.stagelink.invoke("network.getStatus"))')),
    )
    check('real network adapters were enumerated', network.adapters.length > 0)
    console.log(`[smoke] ${network.adapters.length} network adapter(s) via the bridge`)

    // -- honesty invariants in the UI --------------------------------------
    const receiverList = JSON.parse(
      String(await cdp.evaluate('JSON.stringify(await window.stagelink.invoke("receivers.list"))')),
    )
    check(
      'the receiver list is empty, not populated with demo devices',
      receiverList.receivers.length === 0,
      `got ${receiverList.receivers.length}`,
    )

    const bodyText = await cdp.evaluate('document.body.innerText')
    check(
      'the dashboard reports that it is not streaming',
      String(bodyText).includes('Not streaming'),
    )
    check(
      'an unselected input is reported as such rather than shown as a level',
      String(bodyText).includes('No audio input selected'),
    )

    // -- the join QR renders locally --------------------------------------
    await navigate(cdp, '/receivers')
    await delay(1_200)

    const joinReport = JSON.parse(
      String(
        await cdp.evaluate(`JSON.stringify({
          hasPanel: document.body.innerText.includes("Add a receiver"),
          statesAuthorisation: document.body.innerText.includes("Treat it like a key"),
          hasApprovalQueue: document.body.innerText.includes("Awaiting your approval"),
          gatedOnAdapter: document.body.innerText.includes("Choose a streaming network adapter first"),
        })`),
      ),
    )
    check('the Receivers page shows the join panel', joinReport.hasPanel, JSON.stringify(joinReport))
    check(
      'the panel says the code is a credential, not just an address',
      joinReport.statesAuthorisation,
      JSON.stringify(joinReport),
    )
    check(
      'there is no approval queue left in the interface',
      joinReport.hasApprovalQueue === false,
      JSON.stringify(joinReport),
    )

    // Minting needs a streaming adapter, which this machine may not have
    // configured. Both outcomes are correct; what must not happen is a QR
    // appearing without a real code behind it.
    const mint = JSON.parse(
      String(
        await cdp.evaluate(
          'window.stagelink.invoke("receivers.createJoinCode")' +
            '.then(c => JSON.stringify({ ok: true, payload: c.qrPayload, token: c.token, host: c.host }))' +
            '.catch(e => JSON.stringify({ ok: false, code: e.code, message: e.message }))',
        ),
      ),
    )

    // The join panel gates itself on the renderer's view of the network while
    // minting gates on the service's. If those two disagree, one of them is
    // showing the operator something untrue — a warning telling them to select
    // an adapter they have selected, or a QR for an adapter that is gone.
    const networkAgreement = JSON.parse(
      String(
        await cdp.evaluate(
          'window.stagelink.invoke("network.getStatus")' +
            '.then(s => JSON.stringify({ ok: true, adapter: s.selectedAdapterId, address: s.selectedAddress }))' +
            '.catch(e => JSON.stringify({ ok: false, code: e.code }))',
        ),
      ),
    )
    // If a round trip through the Network page un-gates the panel, the panel
    // was reading stale store state rather than disagreeing about policy.
    await navigate(cdp, '/network')
    await delay(900)
    await navigate(cdp, '/receivers')
    await delay(900)
    const afterNetworkVisit = await cdp.evaluate(
      'document.body.innerText.includes("Choose a streaming network adapter first")',
    )
    check(
      'the join panel does not need the Network page to have been visited first',
      afterNetworkVisit === joinReport.gatedOnAdapter,
      JSON.stringify({ gatedOnFirstVisit: joinReport.gatedOnAdapter, afterNetworkVisit }),
    )

    check(
      'the service and the join panel agree on whether a network is configured',
      networkAgreement.ok &&
        (networkAgreement.address !== null) === mint.ok &&
        (networkAgreement.address !== null) !== joinReport.gatedOnAdapter,
      JSON.stringify({ networkAgreement, minted: mint.ok, gated: joinReport.gatedOnAdapter }),
    )

    if (mint.ok) {
      check(
        'a minted join code carries a scannable payload',
        String(mint.payload).startsWith('stagelink://join?') &&
          String(mint.payload).includes(mint.token),
        JSON.stringify(mint),
      )
      check('the join token is 256 bits of hex', /^[0-9a-f]{64}$/.test(String(mint.token)))

      await delay(800)
      const qr = JSON.parse(
        String(
          await cdp.evaluate(`(() => {
            const svg = document.querySelector("svg[viewBox]")
            const text = document.body.innerText
            return JSON.stringify({
              rendered: svg !== null,
              shapes: svg ? svg.querySelectorAll("path, rect").length : 0,
              remote: document.body.innerHTML.includes("api.qrserver") ||
                      document.body.innerHTML.includes("chart.googleapis"),
              // Why it is missing, when it is missing. A bare "rendered: false"
              // cannot distinguish "the panel refused to show a QR" from "the
              // code never reached the store" from "the encoder threw", and
              // those have nothing to do with each other.
              stillGated: text.includes("Choose a streaming network adapter first"),
              sawCountdown: text.includes("Expires in"),
              sawExpired: text.includes("Expired"),
            })
          })()`),
        ),
      )
      check('the QR rendered as inline SVG', qr.rendered && qr.shapes > 0, JSON.stringify(qr))
      check('the QR was generated locally, not fetched', qr.remote === false, JSON.stringify(qr))
    } else {
      check(
        'minting is refused with guidance when no adapter is selected',
        mint.code === 'NETWORK_ADAPTER_UNAVAILABLE' ||
          /streaming network adapter/.test(String(mint.message)),
        JSON.stringify(mint),
      )
      console.log('[smoke] no streaming adapter configured; QR rendering not exercised')
    }

    await navigate(cdp, '/')
    await delay(600)

    // -- icons are bundled, not fetched -----------------------------------
    // Nuxt Icon renders in CSS mode: a span whose mask-image is the icon. A
    // *data URI* mask is the thing being asserted — it proves the glyph was
    // compiled into the bundle rather than fetched from api.iconify.design,
    // which would leave the whole interface iconless on an isolated network.
    const iconReport = JSON.parse(
      String(
        await cdp.evaluate(
          '(() => { const spans = [...document.querySelectorAll("nav a span.iconify")]; const masks = spans.map(s => getComputedStyle(s).maskImage || ""); return JSON.stringify({ count: spans.length, dataUris: masks.filter(m => m.includes("data:image/svg+xml")).length, remote: masks.filter(m => m.includes("iconify.design")).length }) })()',
        ),
      ),
    )
    check(
      'every navigation icon rendered',
      iconReport.count >= 8,
      JSON.stringify(iconReport),
    )
    check(
      'icons are bundled locally, not fetched from the network',
      iconReport.dataUris === iconReport.count && iconReport.remote === 0,
      JSON.stringify(iconReport),
    )

    // -- a method the renderer must not be able to reach -------------------
    const forbidden = await cdp.evaluate(
      'window.stagelink.invoke("system.exec", { cmd: "calc.exe" }).then(() => "RESOLVED").catch(e => JSON.stringify({ code: e.code, message: e.message }))',
    )
    const refusal = JSON.parse(String(forbidden))
    check(
      'an unregistered method is refused at the bridge',
      refusal.code === 'UNKNOWN_METHOD' || /is not a control method/.test(String(refusal.message)),
      String(forbidden),
    )

    // -- routing -----------------------------------------------------------
    for (const route of ['/audio', '/receivers', '/network', '/diagnostics', '/settings', '/about']) {
      await navigate(cdp, route)
      await delay(400)
      const pathname = await cdp.evaluate('window.location.pathname')
      check(`client-side routing reaches ${route}`, pathname === route, String(pathname))
    }
    await navigate(cdp, '/')
    await delay(600)

    // -- capture the window, not the screen --------------------------------
    const shot = await cdp.send('Page.captureScreenshot', { format: 'png' })
    const shotPath = path.join(outputDir, `dashboard-${production ? 'production' : 'development'}.png`)
    writeFileSync(shotPath, Buffer.from(shot.data, 'base64'))
    console.log(`[smoke] wrote ${path.relative(repoRoot, shotPath)}`)

    console.log('')
    console.log(failures === 0 ? 'ALL SMOKE CHECKS PASSED' : `${failures} SMOKE CHECK(S) FAILED`)
  } catch (error) {
    failures += 1
    console.error('[smoke] fatal:', error instanceof Error ? error.message : error)
    console.error('[smoke] last output:\n' + logLines.slice(-30).join(''))
  } finally {
    electron.kill()
    // Electron can take a moment to let go of the audio service.
    await delay(1_500)
  }

  process.exit(failures === 0 ? 0 : 1)
}

main().catch((error) => {
  console.error('[smoke] fatal:', error)
  process.exit(1)
})
