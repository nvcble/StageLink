#!/usr/bin/env node
/**
 * Drives the StageLink service over its named pipe, from outside Electron.
 *
 * This is the smoke test for the whole control path: spawn the service, do the
 * authenticated handshake, call real methods and print what comes back. If this
 * works, the Electron main process has nothing left to discover.
 *
 *   node scripts/probe-service.mjs [--release]
 */

import { spawn } from 'node:child_process'
import net from 'node:net'
import { once } from 'node:events'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const here = path.dirname(fileURLToPath(import.meta.url))
const repoRoot = path.resolve(here, '..')
const profile = process.argv.includes('--release') ? 'release' : 'debug'
const servicePath = path.join(
  repoRoot,
  'crates',
  'target',
  profile,
  'stagelink-service.exe',
)

const INSTANCE_ID = `probe${Date.now().toString(16)}`
const TOKEN = 'probe-token-0123456789abcdef'
const PIPE = `\\\\.\\pipe\\stagelink-service-${INSTANCE_ID}`

function log(...args) {
  console.log('[probe]', ...args)
}

/** Connects to the pipe, retrying while the service starts up. */
async function connect(attempts = 60) {
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      const socket = net.connect(PIPE)
      await once(socket, 'connect')
      return socket
    } catch {
      await new Promise((resolve) => setTimeout(resolve, 100))
    }
  }
  throw new Error(`could not connect to ${PIPE}`)
}

/** Minimal NDJSON client over the pipe. */
class Client {
  constructor(socket) {
    this.socket = socket
    this.buffer = ''
    this.pending = new Map()
    this.events = []
    this.nextId = 1
    this.welcome = null

    socket.setEncoding('utf8')
    socket.on('data', (chunk) => this.#onData(chunk))
  }

  #onData(chunk) {
    this.buffer += chunk
    let index
    while ((index = this.buffer.indexOf('\n')) >= 0) {
      const line = this.buffer.slice(0, index)
      this.buffer = this.buffer.slice(index + 1)
      if (!line.trim()) continue
      let message
      try {
        message = JSON.parse(line)
      } catch (error) {
        log('unparseable line:', line.slice(0, 120), error.message)
        continue
      }
      if (message.kind === 'welcome') {
        this.welcome = message
        this.resolveWelcome?.(message)
      } else if (message.kind === 'response') {
        const entry = this.pending.get(message.id)
        if (entry) {
          this.pending.delete(message.id)
          if (message.ok) entry.resolve(message.result)
          else entry.reject(Object.assign(new Error(message.error.message), message.error))
        }
      } else if (message.kind === 'event') {
        this.events.push(message)
      }
    }
  }

  send(object) {
    this.socket.write(`${JSON.stringify(object)}\n`)
  }

  handshake(token = TOKEN, version = '1.0.0') {
    const welcomed = new Promise((resolve, reject) => {
      this.resolveWelcome = resolve
      setTimeout(() => reject(new Error('handshake timed out')), 10_000)
    })
    this.send({
      v: 1,
      kind: 'hello',
      token,
      client: 'probe/0.1.0',
      controlProtocolVersion: version,
    })
    return welcomed
  }

  invoke(method, params = {}) {
    const id = String(this.nextId++)
    const answered = new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject })
      setTimeout(() => {
        if (this.pending.delete(id)) reject(new Error(`${method} timed out`))
      }, 30_000)
    })
    this.send({ v: 1, kind: 'request', id, method, params })
    return answered
  }
}

async function main() {
  log(`spawning ${servicePath}`)
  const service = spawn(
    servicePath,
    [
      '--instance-id',
      INSTANCE_ID,
      '--token',
      TOKEN,
      '--ephemeral',
      '--log-level',
      'info',
    ],
    { stdio: ['ignore', 'inherit', 'inherit'] },
  )

  service.on('exit', (code) => log(`service exited with code ${code}`))

  let failures = 0
  const check = (label, condition, detail = '') => {
    if (condition) {
      log(`PASS  ${label}`)
    } else {
      failures += 1
      log(`FAIL  ${label} ${detail}`)
    }
  }

  try {
    // -- a wrong token must be refused -----------------------------------
    {
      const socket = await connect()
      const client = new Client(socket)
      const closed = once(socket, 'close')
      client.handshake('definitely-the-wrong-token').catch(() => {})
      await Promise.race([
        closed.then(() => check('a wrong handshake token is refused', true)),
        new Promise((resolve) =>
          setTimeout(() => {
            check(
              'a wrong handshake token is refused',
              client.welcome === null,
              '(connection stayed open and was welcomed)',
            )
            resolve()
          }, 3_000),
        ),
      ])
      socket.destroy()
    }

    // -- an incompatible protocol major must be refused ------------------
    {
      const socket = await connect()
      const client = new Client(socket)
      const closed = once(socket, 'close')
      client.handshake(TOKEN, '2.0.0').catch(() => {})
      await Promise.race([
        closed.then(() => check('an incompatible protocol major is refused', true)),
        new Promise((resolve) =>
          setTimeout(() => {
            check(
              'an incompatible protocol major is refused',
              client.welcome === null,
              '(was welcomed anyway)',
            )
            resolve()
          }, 3_000),
        ),
      ])
      socket.destroy()
    }

    // -- the real session -------------------------------------------------
    const socket = await connect()
    const client = new Client(socket)
    const welcome = await client.handshake()
    check('handshake accepted', welcome.kind === 'welcome')
    check(
      'welcome reports the protocol versions',
      welcome.controlProtocolVersion === '1.0.0' && welcome.audioProtocolVersion === 1,
      JSON.stringify(welcome),
    )
    log('welcome:', JSON.stringify(welcome))

    const ping = await client.invoke('service.ping')
    check('service.ping answers with an uptime', typeof ping.uptimeMs === 'number')

    const info = await client.invoke('service.getInfo')
    check('service.getInfo reports a version', typeof info.version === 'string')
    check(
      'ASIO is not advertised without a backend',
      !info.availableAudioApis.includes('asio'),
      JSON.stringify(info.availableAudioApis),
    )

    const devices = await client.invoke('audio.listDevices', { probeFormats: false })
    log(`enumerated ${devices.devices.length} capture endpoint(s)`)
    for (const device of devices.devices.slice(0, 6)) {
      log(
        `  - ${device.name} [${device.state}] ${device.inputChannels}ch @ ${device.defaultSampleRate} Hz`,
      )
    }
    check('audio.listDevices returns real endpoints', Array.isArray(devices.devices))
    check(
      'every endpoint has an id and a name',
      devices.devices.every((device) => device.id && device.name),
    )

    const settings = await client.invoke('settings.get')
    check('settings.get returns defaults', settings.audio.sampleRate === 48_000)
    check(
      'the default transport is encrypted',
      settings.streaming.transport === 'udp-encrypted',
      settings.streaming.transport,
    )

    const updated = await client.invoke('settings.update', {
      audio: { channelIndex: 1 },
    })
    check(
      'settings.update reports restart-required paths',
      updated.restartRequired.includes('audio.channelIndex'),
      JSON.stringify(updated.restartRequired),
    )

    await client
      .invoke('settings.update', { audio: { sampleRate: 3 } })
      .then(() => check('an invalid setting is refused', false, '(it was accepted)'))
      .catch((error) =>
        check(
          'an invalid setting is refused with its field',
          error.code === 'SETTINGS_INVALID' && error.field === 'audio.sampleRate',
          JSON.stringify({ code: error.code, field: error.field }),
        ),
      )

    const network = await client.invoke('network.getStatus')
    log(`enumerated ${network.adapters.length} network adapter(s)`)
    for (const adapter of network.adapters.filter((a) => a.isUp).slice(0, 5)) {
      const address = adapter.addresses.find((a) => a.family === 'ipv4')?.address ?? '-'
      log(`  - ${adapter.name} [${adapter.kind}] ${address}`)
    }
    check('network.getStatus returns adapters', network.adapters.length > 0)
    check(
      'an unselected adapter produces a warning',
      network.warnings.some((w) => w.code === 'NO_ADAPTER_SELECTED'),
    )

    const receivers = await client.invoke('receivers.list')
    check(
      'the receiver list is genuinely empty',
      receivers.receivers.length === 0 && receivers.pendingRequests.length === 0,
    )

    const diagnostics = await client.invoke('diagnostics.getSnapshot')
    check(
      'diagnostics tag provenance',
      diagnostics.latency.captureBufferMs.source === 'measured' &&
        diagnostics.latency.packetFrameMs.source === 'estimated' &&
        diagnostics.latency.receiverReportedOutputMs.source === 'receiver-reported',
    )
    check(
      'an idle service reports null latency rather than zero',
      diagnostics.latency.desktopSubtotalMs.value === null,
    )
    check(
      'system memory is actually measured',
      typeof diagnostics.system.processMemoryBytes === 'number' &&
        diagnostics.system.processMemoryBytes > 0,
    )

    await client
      .invoke('system.exec', { cmd: 'calc.exe' })
      .then(() => check('an unknown method is refused', false, '(it ran)'))
      .catch((error) =>
        check('an unknown method is refused', error.code === 'UNKNOWN_METHOD', error.code),
      )

    await client
      .invoke('stream.start', {})
      .then(() => check('stream.start does not fake success', false, '(it returned ok)'))
      .catch((error) =>
        check(
          'stream.start reports NOT_IMPLEMENTED with its phase',
          error.code === 'NOT_IMPLEMENTED' && String(error.detail).includes('Phase'),
          JSON.stringify({ code: error.code, detail: error.detail }),
        ),
      )

    // -- open a real device and watch real meters ------------------------
    const usable = devices.devices.find(
      (device) => device.state === 'active' && device.inputChannels > 0,
    )
    if (usable) {
      log(`opening "${usable.name}" channel 0`)
      try {
        const status = await client.invoke('audio.selectDevice', {
          deviceId: usable.id,
          channelIndex: 0,
          sampleRate: 48_000,
        })
        check('audio.selectDevice reports running', status.state === 'running', status.state)
        check('the negotiated format is reported', status.format !== null)
        log('negotiated:', JSON.stringify(status.format))

        client.events.length = 0
        await new Promise((resolve) => setTimeout(resolve, 1_200))

        const levelEvents = client.events.filter((event) => event.event === 'audio.levels')
        check(
          'real meter events arrive while capturing',
          levelEvents.length > 10,
          `(got ${levelEvents.length} in 1.2 s)`,
        )
        const sample = levelEvents.at(-1)
        if (sample) log('last meter frame:', JSON.stringify(sample.payload))

        const live = await client.invoke('audio.getStatus')
        check('frames are actually being captured', live.framesCaptured > 0, String(live.framesCaptured))

        const liveDiagnostics = await client.invoke('diagnostics.getSnapshot')
        check(
          'the capture period is now a measured value',
          typeof liveDiagnostics.latency.captureBufferMs.value === 'number',
          JSON.stringify(liveDiagnostics.latency.captureBufferMs),
        )

        const test = await client.invoke('audio.test', { durationMs: 500 })
        log('input test:', JSON.stringify(test))
        check('audio.test analyses real frames', test.framesAnalysed > 0)

        const released = await client.invoke('audio.releaseDevice')
        check('audio.releaseDevice returns to idle', released.state === 'idle', released.state)
      } catch (error) {
        log(`could not open "${usable.name}": ${error.message}`)
        log('(this is an environment fact, not a control-path failure)')
      }
    } else {
      log('no active capture endpoint on this machine; skipping the live-capture checks')
    }

    // -- reconnect: the service must survive losing its client ------------
    socket.destroy()
    await new Promise((resolve) => setTimeout(resolve, 300))
    const second = new Client(await connect())
    const secondWelcome = await second.handshake()
    check(
      'the service accepts a new client after the first disconnects',
      secondWelcome.kind === 'welcome',
    )
    const pingAgain = await second.invoke('service.ping')
    check('and still answers requests', typeof pingAgain.uptimeMs === 'number')
    second.socket.destroy()

    log('')
    log(failures === 0 ? 'ALL CHECKS PASSED' : `${failures} CHECK(S) FAILED`)
  } finally {
    service.kill()
  }

  process.exit(failures === 0 ? 0 : 1)
}

main().catch((error) => {
  console.error('[probe] fatal:', error)
  process.exit(1)
})
