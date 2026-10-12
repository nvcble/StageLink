#!/usr/bin/env node
/**
 * Prints everything the renderer logs, plus any uncaught exception.
 *
 * A diagnostic for the case where the window opens but nothing renders — the
 * failure is almost always a plugin or a store throwing during app setup, and
 * the only place that is visible is the renderer console.
 *
 *   node scripts/renderer-console.mjs
 */

import { spawn } from 'node:child_process'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const here = path.dirname(fileURLToPath(import.meta.url))
const repoRoot = path.resolve(here, '..')
const desktopDir = path.join(repoRoot, 'apps', 'desktop')
const DEBUG_PORT = 9230

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

async function findTarget(attempts = 90) {
  for (let i = 0; i < attempts; i += 1) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${DEBUG_PORT}/json/list`)).json()
      const page = list.find((t) => t.type === 'page' && !t.url.startsWith('devtools://'))
      if (page?.webSocketDebuggerUrl) return page
    } catch {
      /* not up yet */
    }
    await delay(500)
  }
  throw new Error('renderer never appeared')
}

const electron = spawn(
  path.join(repoRoot, 'node_modules', 'electron', 'dist', 'electron.exe'),
  ['.', `--remote-debugging-port=${DEBUG_PORT}`],
  { cwd: desktopDir, env: { ...process.env, NODE_ENV: 'development' }, stdio: 'ignore' },
)

try {
  const target = await findTarget()
  const socket = new WebSocket(target.webSocketDebuggerUrl)
  await new Promise((resolve) => socket.addEventListener('open', resolve, { once: true }))

  let id = 1
  const send = (method, params = {}) => socket.send(JSON.stringify({ id: id++, method, params }))

  socket.addEventListener('message', (event) => {
    const message = JSON.parse(event.data)
    if (message.method === 'Runtime.consoleAPICalled') {
      const text = message.params.args
        .map((arg) => arg.value ?? arg.description ?? arg.unserializableValue ?? '?')
        .join(' ')
      console.log(`[${message.params.type}] ${text}`)
    } else if (message.method === 'Runtime.exceptionThrown') {
      const details = message.params.exceptionDetails
      console.log(
        `[exception] ${details.text} ${details.exception?.description ?? ''}`.trim(),
      )
    } else if (message.method === 'Log.entryAdded') {
      console.log(`[${message.params.entry.level}] ${message.params.entry.text}`)
    }
  })

  send('Runtime.enable')
  send('Log.enable')
  // Reload so the listeners catch setup-time errors too.
  send('Page.enable')
  await delay(500)
  send('Page.reload', { ignoreCache: true })

  await delay(6_000)
  // Navigate to the Receivers page so its errors are captured too.
  send('Runtime.evaluate', { expression: 'window.history.pushState({}, "", "/receivers"); window.dispatchEvent(new PopStateEvent("popstate"))' })
  await delay(4_000)

  // Report the DOM state so "nothing rendered" is distinguishable from
  // "rendered but empty".
  const result = await new Promise((resolve) => {
    const probeId = id++
    const handler = (event) => {
      const message = JSON.parse(event.data)
      if (message.id === probeId) {
        socket.removeEventListener('message', handler)
        resolve(message.result)
      }
    }
    socket.addEventListener('message', handler)
    socket.send(
      JSON.stringify({
        id: probeId,
        method: 'Runtime.evaluate',
        params: {
          expression:
            'JSON.stringify({ nuxtRoot: !!document.querySelector("#__nuxt"), nuxtChildren: document.querySelector("#__nuxt")?.children.length ?? -1, bodyLength: document.body.innerHTML.length, head: document.body.innerHTML.slice(0, 400) })',
          returnByValue: true,
        },
      }),
    )
  })
  console.log('\n[dom]', result?.result?.value ?? '(no value)')
} finally {
  electron.kill()
  await delay(1_000)
  process.exit(0)
}
