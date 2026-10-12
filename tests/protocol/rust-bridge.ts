import { type ChildProcessWithoutNullStreams, spawn } from 'node:child_process'
import { existsSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

/**
 * Drives the Rust `protocol-probe` binary so the cross-check tests can compare
 * the two protocol implementations directly.
 *
 * One long-lived process answers every query — spawning per query would make
 * the suite take minutes rather than seconds.
 */

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..')

function probePath(): string {
  const executable = process.platform === 'win32' ? 'protocol-probe.exe' : 'protocol-probe'
  for (const profile of ['debug', 'release']) {
    const candidate = path.join(repoRoot, 'crates', 'target', profile, executable)
    if (existsSync(candidate)) return candidate
  }
  throw new Error(
    'protocol-probe has not been built. Run:\n' +
      '  cargo build --manifest-path crates/Cargo.toml --bin protocol-probe',
  )
}

let child: ChildProcessWithoutNullStreams | null = null
let buffer = ''
const queue: Array<(value: unknown) => void> = []

function ensureProbe(): ChildProcessWithoutNullStreams {
  if (child && child.exitCode === null) return child

  child = spawn(probePath(), [], { stdio: ['pipe', 'pipe', 'pipe'] })
  child.stdout.setEncoding('utf8')
  child.stdout.on('data', (chunk: string) => {
    buffer += chunk
    let index: number
    while ((index = buffer.indexOf('\n')) >= 0) {
      const line = buffer.slice(0, index).trim()
      buffer = buffer.slice(index + 1)
      if (!line) continue
      const resolve = queue.shift()
      if (!resolve) continue
      try {
        resolve(JSON.parse(line))
      } catch {
        resolve({ ok: false, reason: `unparseable probe answer: ${line.slice(0, 120)}` })
      }
    }
  })
  child.stderr.setEncoding('utf8')
  child.stderr.on('data', (chunk: string) => {
    if (process.env.STAGELINK_PROBE_VERBOSE) process.stderr.write(`[probe] ${chunk}`)
  })

  return child
}

/** Sends one query and awaits its answer. */
async function ask<T>(query: Record<string, unknown>): Promise<T> {
  const probe = ensureProbe()
  const answered = new Promise<T>((resolve, reject) => {
    queue.push(resolve as (value: unknown) => void)
    setTimeout(() => reject(new Error(`probe query timed out: ${JSON.stringify(query)}`)), 15_000)
  })
  probe.stdin.write(`${JSON.stringify(query)}\n`)
  return answered
}

/** Shuts the probe down. Called from the suite teardown. */
export function closeProbe(): void {
  child?.stdin.end()
  child?.kill()
  child = null
  buffer = ''
  queue.length = 0
}

// -- typed queries -----------------------------------------------------------

export async function controlProtocolVersion(): Promise<string> {
  const { value } = await ask<{ value: string }>({ q: 'controlProtocolVersion' })
  return value
}

export interface RustPacketConstants {
  audioProtocolVersion: number
  headerBytes: number
  magic: string
  aeadTagBytes: number
  maxPayloadBytes: number
}

export function rustPacketConstants(): Promise<RustPacketConstants> {
  return ask<RustPacketConstants>({ q: 'packetConstants' })
}

export async function rustMethodNames(): Promise<string[]> {
  const { methods } = await ask<{ methods: string[] }>({ q: 'methodNames' })
  return methods
}

export interface RustSettings {
  audio: {
    sampleRate: number
    channelIndex: number
    shareMode: string
    autoReconnect: boolean
  }
  network: { streamPort: number; controlPort: number; discoveryEnabled: boolean }
  streaming: {
    frameDurationMs: number
    transport: string
    maxReceivers: number
    payloadFormat: string
  }
  startup: { showSetupWizard: boolean }
  appearance: { theme: string }
  diagnostics: { intervalMs: number }
}

export function rustDefaultSettings(): Promise<RustSettings> {
  return ask<RustSettings>({ q: 'defaultSettings' })
}

export async function encodeRustEnvelope(envelope: Record<string, unknown>): Promise<string> {
  const result = await ask<{ ok: boolean; line?: string; reason?: string }>({
    q: 'encodeEnvelope',
    envelope,
  })
  if (!result.ok || !result.line) {
    throw new Error(`Rust could not encode the envelope: ${result.reason ?? 'unknown'}`)
  }
  return result.line
}

export function decodeRustEnvelope(
  line: string,
): Promise<{ ok: boolean; kind?: string; reason?: string }> {
  return ask({ q: 'decodeEnvelope', line })
}

export interface RustPacketGeometry {
  framesPerPacket: number
  datagramBytesPlaintext: number
  datagramBytesEncrypted: number
  fitsInMtu: boolean
}

export function packetGeometry(
  sampleRate: number,
  frameDurationMs: number,
): Promise<RustPacketGeometry> {
  return ask<RustPacketGeometry>({ q: 'packetGeometry', sampleRate, frameDurationMs })
}

export async function sequenceDeltaRust(previous: number, current: number): Promise<number> {
  const { value } = await ask<{ value: number }>({ q: 'sequenceDelta', previous, current })
  return value
}

/**
 * A roster entry as the Rust serialiser emits it, for validation against
 * `receiverSchema`. The entry is a fixture inside the probe; the application's
 * own roster is empty until a device joins.
 */
export function rustRosterEntry(withTelemetry = true): Promise<Record<string, unknown>> {
  return ask<Record<string, unknown>>({ q: 'rosterEntryShape', withTelemetry })
}

export async function aeadNonceRust(sessionId: number, sequence: number): Promise<number[]> {
  const { bytes } = await ask<{ bytes: number[] }>({ q: 'aeadNonce', sessionId, sequence })
  return bytes
}
