#!/usr/bin/env node
/**
 * The end-to-end test: a real desktop service, a real join, real audio.
 *
 * Spawns `stagelink-service`, drives it over its named pipe exactly as the
 * Electron main process does, mints a join code, starts a stream, then runs
 * one or more `stagelink-receiver` processes which join over the real control
 * channel and record what they hear to a WAV file.
 *
 *   node scripts/end-to-end.mjs                  # one receiver, tone source
 *   node scripts/end-to-end.mjs --receivers 2    # prove multi-receiver
 *   node scripts/end-to-end.mjs --device "NUX"   # use a real audio interface
 *
 * This is the test that cannot be faked. Every other test in the repository
 * checks one side of a boundary; this one puts audio in at the desktop and
 * takes a listenable file out at the receiver, through the real packetiser,
 * the real socket, the real join handshake and the real jitter buffer.
 *
 * ## Why it does not need a mixer
 *
 * With no `--device` it selects the first capture endpoint it can open, which
 * on most machines is a microphone, and the result is a WAV of the room. That
 * is enough to prove delivery. For a real monitor mix, pass `--device` with
 * part of the interface's name and feed it a signal.
 */

import { spawn } from 'node:child_process'
import net from 'node:net'
import { once } from 'node:events'
import { mkdirSync, statSync } from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const here = path.dirname(fileURLToPath(import.meta.url))
const repoRoot = path.resolve(here, '..')
const profile = process.argv.includes('--release') ? 'release' : 'debug'
const binary = (name) =>
  path.join(repoRoot, 'crates', 'target', profile, `${name}.exe`)

const argumentValue = (flag, fallback) => {
  const index = process.argv.indexOf(flag)
  return index >= 0 && process.argv[index + 1] ? process.argv[index + 1] : fallback
}

const RECEIVERS = Number(argumentValue('--receivers', '1'))
const SECONDS = Number(argumentValue('--seconds', '12'))
const DEVICE_HINT = argumentValue('--device', null)
const OUTPUT_DIR = path.join(repoRoot, 'tests', 'integration', 'artifacts')

const INSTANCE_ID = `e2e${Date.now().toString(16)}`
const TOKEN = 'end-to-end-token-0123456789abcdef'
const PIPE = `\\\\.\\pipe\\stagelink-service-${INSTANCE_ID}`

let failures = 0
const check = (label, condition, detail = '') => {
  if (condition) {
    console.log(`PASS  ${label}`)
  } else {
    failures += 1
    console.log(`FAIL  ${label}${detail ? ` — ${detail}` : ''}`)
  }
}
const log = (...args) => console.log('[e2e]', ...args)
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

/** Minimal NDJSON client over the service's named pipe. */
class Client {
  constructor(socket) {
    this.socket = socket
    this.buffer = ''
    this.pending = new Map()
    this.events = []
    this.nextId = 1
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
      } catch {
        continue
      }
      if (message.kind === 'welcome') this.resolveWelcome?.(message)
      else if (message.kind === 'response') {
        const entry = this.pending.get(message.id)
        if (entry) {
          this.pending.delete(message.id)
          if (message.ok) entry.resolve(message.result)
          else entry.reject(Object.assign(new Error(message.error.message), message.error))
        }
      } else if (message.kind === 'event') this.events.push(message)
    }
  }

  handshake() {
    const welcomed = new Promise((resolve, reject) => {
      this.resolveWelcome = resolve
      setTimeout(() => reject(new Error('handshake timed out')), 10_000)
    })
    this.socket.write(
      `${JSON.stringify({
        v: 1,
        kind: 'hello',
        token: TOKEN,
        client: 'end-to-end/0.1.0',
        controlProtocolVersion: '1.0.0',
      })}\n`,
    )
    return welcomed
  }

  invoke(method, params = {}) {
    const id = String(this.nextId++)
    const answered = new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject })
      setTimeout(() => {
        if (this.pending.delete(id)) reject(new Error(`${method} timed out`))
      }, 45_000)
    })
    this.socket.write(`${JSON.stringify({ v: 1, kind: 'request', id, method, params })}\n`)
    return answered
  }

  eventsNamed(name) {
    return this.events.filter((event) => event.event === name)
  }
}

async function connect(attempts = 80) {
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      const socket = net.connect(PIPE)
      await once(socket, 'connect')
      return socket
    } catch {
      await delay(100)
    }
  }
  throw new Error(`could not connect to ${PIPE}`)
}

/** Runs one headless receiver, returning its exit code and output. */
function runReceiver(index, qr, seconds, expectSignal) {
  const output = path.join(OUTPUT_DIR, `e2e-receiver-${index}.wav`)
  // A distinct key per receiver, so the desktop sees separate devices rather
  // than one device reconnecting.
  const key = ['ab', 'cd', 'ef', '12', '34', '56', '78', '9a'][index % 8]
  const child = spawn(
    binary('stagelink-receiver'),
    [
      '--qr', qr,
      '--duration-secs', String(seconds),
      '--output', output,
      '--device-name', `Receiver ${index + 1}`,
      '--key', key,
      // Several receivers on one machine cannot all hold UDP 45872. Each
      // takes an OS-chosen port and declares it in the handshake.
      '--ephemeral-audio-port',
      // Only demanded when the input really has signal. A line input with
      // nothing plugged in captures digital silence, and failing the whole
      // test for that would be reporting a missing cable as a bug.
      ...(expectSignal ? ['--assert-clean'] : []),
    ],
    { stdio: ['ignore', 'pipe', 'pipe'] },
  )

  let stderr = ''
  child.stderr.setEncoding('utf8')
  child.stderr.on('data', (chunk) => {
    stderr += chunk
  })
  child.stdout.resume()

  return new Promise((resolve) => {
    child.on('exit', (code) => resolve({ code, stderr, output, index }))
  })
}

async function main() {
  mkdirSync(OUTPUT_DIR, { recursive: true })
  log(`spawning ${binary('stagelink-service')}`)

  const service = spawn(
    binary('stagelink-service'),
    ['--instance-id', INSTANCE_ID, '--token', TOKEN, '--ephemeral', '--log-level', 'info'],
    { stdio: ['ignore', 'inherit', 'inherit'] },
  )
  service.on('exit', (code) => log(`service exited with code ${code}`))

  const client = new Client(await connect())
  const welcome = await client.handshake()
  check('the service accepted the control handshake', welcome.kind === 'welcome')

  try {
    // -- pick a network adapter -------------------------------------------
    const network = await client.invoke('network.getStatus')
    const usable = network.adapters.filter(
      (adapter) =>
        adapter.isUp &&
        adapter.kind !== 'loopback' &&
        adapter.addresses.some((address) => address.family === 'ipv4'),
    )
    check('the machine has a usable network adapter', usable.length > 0)
    if (usable.length === 0) throw new Error('no adapter to stream from')

    const chosen = usable[0]
    const selected = await client.invoke('network.selectAdapter', { adapterId: chosen.id })
    log(`streaming from ${chosen.name} at ${selected.selectedAddress}`)
    check('an adapter was selected', selected.selectedAddress !== null)

    // -- open an audio input ----------------------------------------------
    const devices = await client.invoke('audio.listDevices', { probeFormats: false })
    check('real capture devices were enumerated', devices.devices.length > 0)

    const candidates = DEVICE_HINT
      ? devices.devices.filter((device) =>
          device.name.toLowerCase().includes(DEVICE_HINT.toLowerCase()),
        )
      : devices.devices
    check(
      DEVICE_HINT ? `a device matching ${DEVICE_HINT} exists` : 'there is a device to open',
      candidates.length > 0,
      DEVICE_HINT ? devices.devices.map((device) => device.name).join(', ') : '',
    )

    // Prefer a device that is actually carrying signal. A line input with
    // nothing plugged into it captures digital silence, and a WAV of silence
    // proves delivery but not that non-zero samples survive the trip — which
    // is the more interesting half.
    let opened = null
    let signal = null
    for (const device of candidates) {
      try {
        const status = await client.invoke('audio.selectDevice', {
          deviceId: device.id,
          channelIndex: 0,
        })
        if (status.state !== 'running') continue

        const test = await client.invoke('audio.test', { durationMs: 700 })
        log(`${device.name}: ${test.verdict}`)
        if (test.signalDetected) {
          opened = status
          signal = test
          break
        }
        // Keep the first device that opened at all, in case nothing has
        // signal: proving delivery is still worth doing.
        opened ??= status
      } catch (error) {
        log(`could not open ${device.name}: ${error.message}`)
      }
    }
    if (opened && !signal) {
      // Re-open the fallback, since the loop moved on from it.
      const fallback = candidates.find((device) => device.id === opened.deviceId)
      if (fallback) {
        opened = await client.invoke('audio.selectDevice', {
          deviceId: fallback.id,
          channelIndex: 0,
        })
      }
    }

    check('a real capture device is running', opened?.state === 'running', opened?.lastError ?? '')
    if (opened?.state !== 'running') throw new Error('no capture device could be opened')
    if (signal) {
      log(`real signal present at ${signal.peakDb?.toFixed(1)} dBFS peak`)
    } else {
      log(
        'no input has signal, so delivery is verified but the audio content is digital silence',
      )
    }

    // -- choose the transport ----------------------------------------------
    // The encrypted transport is the default, is what a release build
    // requires, and is what this test uses. `--plaintext` exists to exercise
    // the development transport the headless tools use when there is no key
    // exchange — a release build refuses it outright.
    if (process.argv.includes('--plaintext')) {
      await client.invoke('settings.update', {
        streaming: { transport: 'udp-plaintext' },
      })
      log('using the plaintext transport (development builds only)')
    } else {
      log('using the encrypted transport')
    }

    // -- mint a join code --------------------------------------------------
    const code = await client.invoke('receivers.createJoinCode')
    check(
      'the join code carries a scannable payload',
      String(code.qrPayload).startsWith('stagelink://join?'),
    )
    check('the join token is 256 bits of hex', /^[0-9a-f]{64}$/.test(String(code.token)))
    log(`join code valid for ${Math.round((code.expiresAt - code.issuedAt) / 1000)}s`)

    // -- start streaming ---------------------------------------------------
    const started = await client.invoke('stream.start')
    check('the stream started', started.state === 'running', JSON.stringify(started))
    check(
      'the stream reports its real configuration',
      started.config?.sampleRate > 0 && started.config?.payloadFormat === 'pcm_s16le',
      JSON.stringify(started.config),
    )

    // -- run the receivers -------------------------------------------------
    log(`running ${RECEIVERS} receiver(s) for ${SECONDS}s`)
    const results = await Promise.all(
      Array.from({ length: RECEIVERS }, (_, index) =>
        runReceiver(index, code.qrPayload, SECONDS, signal !== null),
      ),
    )

    for (const result of results) {
      const name = `receiver ${result.index + 1}`
      check(`${name} completed its session`, result.code === 0, lastLines(result.stderr))

      const packets = /received=(\d+) lost=(\d+)/.exec(result.stderr)
      if (packets) {
        const [, received, lost] = packets
        check(`${name} received real packets`, Number(received) > 100, `received=${received}`)
        check(
          `${name} lost few packets`,
          Number(lost) <= Number(received) * 0.05,
          `lost=${lost} of ${received}`,
        )
      } else {
        check(`${name} reported its packet counters`, false, lastLines(result.stderr))
      }
      if (process.argv.includes('--verbose')) {
        console.log(`--- ${name} output ---
${result.stderr}`)
      }

      try {
        const { size } = statSync(result.output)
        // 44-byte WAV header plus two bytes per frame. A second of audio is
        // 96 KB, so anything under 10 KB is not a recording.
        check(`${name} wrote a listenable WAV`, size > 10_000, `${size} bytes`)
        log(`${name}: ${result.output} (${Math.round(size / 1024)} KB)`)
      } catch (error) {
        check(`${name} wrote a WAV`, false, error.message)
      }
    }

    // -- the desktop's view -------------------------------------------------
    const streamStatus = await client.invoke('stream.getStatus')
    check(
      'the desktop sent packets',
      streamStatus.packetsSent > 100,
      JSON.stringify({ sent: streamStatus.packetsSent, failures: streamStatus.sendFailures }),
    )
    check(
      'the desktop had few send failures',
      streamStatus.sendFailures <= streamStatus.packetsSent * 0.01,
      `${streamStatus.sendFailures} of ${streamStatus.packetsSent}`,
    )

    const roster = await client.invoke('receivers.list')
    check(
      'the receivers appeared in the roster',
      client.eventsNamed('receivers.changed').length > 0,
      `${client.eventsNamed('receivers.changed').length} roster events`,
    )
    log(`roster now holds ${roster.receivers.length} receiver(s)`)

    // Telemetry is the receiver's own claim, and the desktop must label it so.
    const withTelemetry = client
      .eventsNamed('receivers.changed')
      .flatMap((event) => event.payload.receivers ?? [])
      .filter((receiver) => receiver.telemetry !== null)
    check(
      'receiver telemetry arrived and is labelled as the receiver claim',
      withTelemetry.length > 0 &&
        withTelemetry.every((receiver) => receiver.telemetry.source === 'receiver-reported'),
      `${withTelemetry.length} reports`,
    )

    // -- stop --------------------------------------------------------------
    const stopped = await client.invoke('stream.stop', { immediate: false })
    check('the stream stopped', stopped.state === 'stopped')
    await client.invoke('audio.releaseDevice')
  } finally {
    try {
      await client.invoke('service.shutdown', { graceMs: 500 })
    } catch {
      // The service may already be going down.
    }
    await delay(600)
    service.kill()
  }

  console.log(failures === 0 ? '\nALL END-TO-END CHECKS PASSED' : `\n${failures} CHECK(S) FAILED`)
  process.exit(failures === 0 ? 0 : 1)
}

function lastLines(text, count = 3) {
  return text.trim().split('\n').slice(-count).join(' | ').slice(0, 400)
}

main().catch((error) => {
  console.error('[e2e] fatal:', error.message)
  process.exit(1)
})
