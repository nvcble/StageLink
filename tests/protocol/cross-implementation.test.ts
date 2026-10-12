import { describe, expect, it } from 'vitest'

import {
  AUDIO_PROTOCOL_VERSION,
  CONTROL_PROTOCOL_VERSION,
  IPC_ENVELOPE_VERSION,
  METHOD_NAMES,
  PACKET_HEADER_BYTES,
  PACKET_MAGIC,
  SUPPORTED_FRAME_DURATIONS_MS,
  decodeEnvelope,
  defaultSettings,
  framesPerPacket,
  sequenceDelta,
} from '@stagelink/shared-protocol'

import {
  controlProtocolVersion,
  decodeRustEnvelope,
  encodeRustEnvelope,
  packetGeometry,
  rustDefaultSettings,
  rustMethodNames,
  rustPacketConstants,
  sequenceDeltaRust,
} from './rust-bridge'

/**
 * Cross-implementation protocol tests.
 *
 * The control and audio protocols are defined twice — in TypeScript for the
 * desktop and in Rust for the service — because neither process can import the
 * other's types. These tests run *both* implementations against the same inputs
 * so the two cannot drift silently.
 *
 * Drift here is not a theoretical worry: the two sides are built and shipped
 * together, so a mismatch is a runtime failure that unit tests on either side
 * alone would never catch.
 */

describe('version constants agree', () => {
  it('the control protocol version matches', async () => {
    expect(await controlProtocolVersion()).toBe(CONTROL_PROTOCOL_VERSION)
  })

  it('the audio protocol version and packet geometry match', async () => {
    const rust = await rustPacketConstants()
    expect(rust.audioProtocolVersion).toBe(AUDIO_PROTOCOL_VERSION)
    expect(rust.headerBytes).toBe(PACKET_HEADER_BYTES)
    expect(rust.magic).toBe(PACKET_MAGIC)
  })

  it('the envelope framing version matches', async () => {
    const line = await encodeRustEnvelope({
      kind: 'request',
      id: 'x',
      method: 'service.ping',
      params: {},
    })
    const parsed = JSON.parse(line) as { v: number }
    expect(parsed.v).toBe(IPC_ENVELOPE_VERSION)
  })
})

describe('envelopes round-trip between implementations', () => {
  it('TypeScript can decode a Rust-encoded request', async () => {
    const line = await encodeRustEnvelope({
      kind: 'request',
      id: '42',
      method: 'audio.getStatus',
      params: {},
    })
    const decoded = decodeEnvelope(line.trim())
    expect(decoded.ok, decoded.ok ? '' : decoded.reason).toBe(true)
    if (decoded.ok) {
      expect(decoded.envelope.kind).toBe('request')
    }
  })

  it('TypeScript can decode a Rust-encoded event', async () => {
    const line = await encodeRustEnvelope({
      kind: 'event',
      event: 'audio.levels',
      payload: {
        at: 1,
        rmsDb: -18,
        peakDb: -6,
        peakHoldDb: -6,
        clipping: false,
        clipCount: 0,
        windowFrames: 1920,
      },
      at: 1,
    })
    const decoded = decodeEnvelope(line.trim())
    expect(decoded.ok, decoded.ok ? '' : decoded.reason).toBe(true)
  })

  it('TypeScript can decode a Rust-encoded error response', async () => {
    const line = await encodeRustEnvelope({
      kind: 'response',
      id: '7',
      ok: false,
      error: {
        code: 'AUDIO_DEVICE_NOT_FOUND',
        message: 'gone',
        retryable: false,
      },
    })
    const decoded = decodeEnvelope(line.trim())
    expect(decoded.ok, decoded.ok ? '' : decoded.reason).toBe(true)
  })

  it('Rust can decode a TypeScript-encoded hello', async () => {
    const result = await decodeRustEnvelope(
      JSON.stringify({
        v: IPC_ENVELOPE_VERSION,
        kind: 'hello',
        token: 'deadbeefdeadbeefdeadbeef',
        client: 'stagelink-desktop/0.1.0',
        controlProtocolVersion: CONTROL_PROTOCOL_VERSION,
      }),
    )
    expect(result.ok, result.ok ? '' : result.reason).toBe(true)
    expect(result.kind).toBe('hello')
  })

  it('Rust rejects what TypeScript rejects', async () => {
    for (const line of [
      '{ not json',
      'null',
      JSON.stringify({ v: 99, kind: 'request', id: 'a', method: 'b', params: {} }),
      JSON.stringify({ v: 1, kind: 'exec', command: 'calc.exe' }),
    ]) {
      const tsResult = decodeEnvelope(line)
      const rustResult = await decodeRustEnvelope(line)
      expect(tsResult.ok, `TypeScript should reject ${line}`).toBe(false)
      expect(rustResult.ok, `Rust should reject ${line}`).toBe(false)
    }
  })
})

describe('the method registries match', () => {
  it('Rust dispatches exactly the methods TypeScript declares', async () => {
    const rust = (await rustMethodNames()).sort()
    const typescript = [...METHOD_NAMES].sort()

    const missingInRust = typescript.filter((name) => !rust.includes(name))
    const extraInRust = rust.filter((name) => !typescript.includes(name))

    expect(missingInRust, 'declared in TypeScript but not dispatched in Rust').toEqual([])
    expect(extraInRust, 'dispatched in Rust but not declared in TypeScript').toEqual([])
  })
})

describe('packet arithmetic matches', () => {
  it('frames per packet agree for every supported configuration', async () => {
    for (const duration of SUPPORTED_FRAME_DURATIONS_MS) {
      for (const rate of [44_100, 48_000, 96_000]) {
        const rust = await packetGeometry(rate, duration)
        expect(rust.framesPerPacket, `${rate} Hz / ${duration} ms`).toBe(
          framesPerPacket(rate, duration),
        )
      }
    }
  })

  it('sequence distance agrees across the 32-bit wrap', async () => {
    const cases: Array<[number, number]> = [
      [10, 11],
      [10, 15],
      [10, 9],
      [0xffff_ffff, 0],
      [0xffff_fffe, 1],
      [0, 0xffff_ffff],
      [5, 5],
    ]
    for (const [previous, current] of cases) {
      const rust = await sequenceDeltaRust(previous, current)
      expect(rust, `delta(${previous}, ${current})`).toBe(sequenceDelta(previous, current))
    }
  })
})

describe('default settings match', () => {
  it('the two implementations produce the same defaults', async () => {
    const rust = await rustDefaultSettings()
    const typescript = defaultSettings()

    // Compared field by field so a mismatch names the field rather than
    // dumping two large objects.
    expect(rust.audio.sampleRate).toBe(typescript.audio.sampleRate)
    expect(rust.audio.channelIndex).toBe(typescript.audio.channelIndex)
    expect(rust.audio.shareMode).toBe(typescript.audio.shareMode)
    expect(rust.audio.autoReconnect).toBe(typescript.audio.autoReconnect)

    expect(rust.network.streamPort).toBe(typescript.network.streamPort)
    expect(rust.network.controlPort).toBe(typescript.network.controlPort)
    expect(rust.network.discoveryEnabled).toBe(typescript.network.discoveryEnabled)

    expect(rust.streaming.frameDurationMs).toBe(typescript.streaming.frameDurationMs)
    expect(rust.streaming.transport).toBe(typescript.streaming.transport)
    expect(rust.streaming.maxReceivers).toBe(typescript.streaming.maxReceivers)
    expect(rust.streaming.payloadFormat).toBe(typescript.streaming.payloadFormat)

    expect(rust.startup.showSetupWizard).toBe(typescript.startup.showSetupWizard)
    expect(rust.appearance.theme).toBe(typescript.appearance.theme)
    expect(rust.diagnostics.intervalMs).toBe(typescript.diagnostics.intervalMs)
  })

  it('both default to the encrypted transport', async () => {
    const rust = await rustDefaultSettings()
    expect(rust.streaming.transport).toBe('udp-encrypted')
    expect(defaultSettings().streaming.transport).toBe('udp-encrypted')
  })
})
