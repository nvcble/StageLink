import { describe, expect, expectTypeOf, it } from 'vitest'
import {
  AUDIO_PROTOCOL_VERSION,
  CONTROL_PROTOCOL_VERSION,
  METHODS,
  METHOD_NAMES,
  MAX_PAYLOAD_BYTES,
  PACKET_HEADER_BYTES,
  SESSION_TRANSITIONS,
  SUPPORTED_FRAME_DURATIONS_MS,
  ServiceErrorCode,
  frameDurationSchema,
  canTransition,
  checkControlCompatibility,
  datagramBytes,
  decodeEnvelope,
  defaultSettings,
  encodeEnvelope,
  estimatedBitrateBps,
  fitsInMtu,
  framesPerPacket,
  isEventName,
  isMethodName,
  isTransportAllowedInProduction,
  makeEvent,
  makeRequest,
  payloadBytesPerPacket,
  receiverLabel,
  restartRequiredFor,
  sequenceDelta,
  serviceError,
  type AudioStatus,
  type MethodParams,
  type MethodResult,
  type Receiver,
  type SessionState,
} from '../src/index'
import { streamingSettingsSchema } from '../src/settings'
import { receiverTelemetrySchema } from '../src/receivers'

describe('version negotiation', () => {
  it('accepts an identical version', () => {
    expect(checkControlCompatibility(CONTROL_PROTOCOL_VERSION).compatible).toBe(true)
  })

  it('accepts a newer minor on the peer', () => {
    expect(checkControlCompatibility('1.7.0', '1.0.0').compatible).toBe(true)
  })

  it('refuses a different major', () => {
    const result = checkControlCompatibility('2.0.0', '1.0.0')
    expect(result.compatible).toBe(false)
    expect(result.reason).toMatch(/major/i)
  })

  it('refuses an unparseable version rather than guessing', () => {
    expect(checkControlCompatibility('').compatible).toBe(false)
    expect(checkControlCompatibility('1.0').compatible).toBe(false)
    expect(checkControlCompatibility('banana').compatible).toBe(false)
  })

  it('tolerates prerelease and build suffixes', () => {
    expect(checkControlCompatibility('1.0.0-rc.1', '1.0.0').compatible).toBe(true)
  })
})

describe('method registry', () => {
  it('exposes every documented operation', () => {
    for (const method of [
      'audio.listDevices',
      'audio.selectDevice',
      'audio.getStatus',
      'stream.start',
      'stream.stop',
      'stream.getStatus',
      'receivers.list',
      'receivers.approve',
      'receivers.disconnect',
      'network.getStatus',
      'diagnostics.getSnapshot',
    ]) {
      expect(isMethodName(method), method).toBe(true)
    }
  })

  it('rejects names that are not in the registry', () => {
    expect(isMethodName('audio.eval')).toBe(false)
    expect(isMethodName('__proto__')).toBe(false)
    expect(isMethodName('constructor')).toBe(false)
    expect(isMethodName('toString')).toBe(false)
  })

  it('gives every method a timeout and a description', () => {
    for (const name of METHOD_NAMES) {
      const definition = METHODS[name]
      expect(definition.timeoutMs, name).toBeGreaterThan(0)
      expect(definition.description.length, name).toBeGreaterThan(10)
    }
  })

  it('validates params with the registered schema', () => {
    const selectDevice = METHODS['audio.selectDevice']
    expect(selectDevice.params.safeParse({ deviceId: 'abc', channelIndex: 0 }).success).toBe(true)
    // A negative channel index is not a physical input.
    expect(selectDevice.params.safeParse({ deviceId: 'abc', channelIndex: -1 }).success).toBe(false)
    // Missing deviceId must not fall through to a default.
    expect(selectDevice.params.safeParse({ channelIndex: 0 }).success).toBe(false)
  })

  it('applies declared defaults when parsing params', () => {
    const parsed = METHODS['stream.stop'].params.parse({})
    expect(parsed).toEqual({ immediate: false })
  })

  it('infers caller and result types from the registry', () => {
    expectTypeOf<MethodParams<'audio.selectDevice'>>().toExtend<{
      deviceId: string
      channelIndex: number
    }>()
    expectTypeOf<MethodResult<'audio.getStatus'>>().toEqualTypeOf<AudioStatus>()
  })
})

describe('event registry', () => {
  it('knows the metering and status events', () => {
    expect(isEventName('audio.levels')).toBe(true)
    expect(isEventName('receivers.changed')).toBe(true)
    expect(isEventName('audio.samples')).toBe(false)
  })
})

describe('envelope framing', () => {
  it('round-trips a request', () => {
    const line = encodeEnvelope(makeRequest('r1', 'audio.getStatus', {}))
    expect(line.endsWith('\n')).toBe(true)
    const decoded = decodeEnvelope(line.trimEnd())
    expect(decoded.ok).toBe(true)
    if (decoded.ok) {
      expect(decoded.envelope.kind).toBe('request')
    }
  })

  it('round-trips an event', () => {
    const decoded = decodeEnvelope(
      encodeEnvelope(makeEvent('audio.levels', { rmsDb: -12 }, 1_700_000_000_000)).trimEnd(),
    )
    expect(decoded.ok).toBe(true)
  })

  it('reports malformed JSON instead of throwing', () => {
    const result = decodeEnvelope('{ not json')
    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.reason).toMatch(/Malformed JSON/)
  })

  it('refuses an envelope with the wrong framing version', () => {
    const result = decodeEnvelope(JSON.stringify({ v: 99, kind: 'request', id: 'x', method: 'y' }))
    expect(result.ok).toBe(false)
  })

  it('refuses an unknown envelope kind', () => {
    const result = decodeEnvelope(JSON.stringify({ v: 1, kind: 'exec', cmd: 'calc.exe' }))
    expect(result.ok).toBe(false)
  })

  it('refuses an oversized line', () => {
    const result = decodeEnvelope('x'.repeat(2_000_000))
    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.reason).toMatch(/exceeds/)
  })

  it('never emits an embedded newline that would break NDJSON framing', () => {
    const line = encodeEnvelope(makeEvent('service.log', { message: 'line one\nline two' }))
    expect(line.slice(0, -1).includes('\n')).toBe(false)
  })
})

describe('audio packet geometry', () => {
  it('matches the documented 5 ms / 48 kHz configuration', () => {
    expect(framesPerPacket(48_000, 5)).toBe(240)
    expect(payloadBytesPerPacket(48_000, 5)).toBe(480)
    expect(datagramBytes(240, false)).toBe(PACKET_HEADER_BYTES + 480)
    expect(datagramBytes(240, true)).toBe(PACKET_HEADER_BYTES + 480 + 16)
  })

  it('keeps every supported frame duration inside one MTU', () => {
    for (const duration of SUPPORTED_FRAME_DURATIONS_MS) {
      const frames = framesPerPacket(48_000, duration)
      expect(fitsInMtu(frames, true), `${duration} ms`).toBe(true)
      expect(payloadBytesPerPacket(48_000, duration)).toBeLessThanOrEqual(MAX_PAYLOAD_BYTES)
    }
  })

  it('rejects a frame duration that would fragment the datagram', () => {
    // 20 ms of mono S16 at 48 kHz is 1920 payload bytes: too big for one
    // datagram, which is why it is not an offered option.
    expect(fitsInMtu(framesPerPacket(48_000, 20), true)).toBe(false)
    expect(frameDurationSchema.safeParse(20).success).toBe(false)
  })

  it('estimates a plausible egress bitrate', () => {
    // 8 receivers at 48 kHz / 5 ms is well inside a 100 Mbit link.
    const bps = estimatedBitrateBps(48_000, 5, 8, PACKET_HEADER_BYTES)
    expect(bps).toBeGreaterThan(5_000_000)
    expect(bps).toBeLessThan(12_000_000)
  })

  it('computes sequence distance across the 32-bit wrap', () => {
    expect(sequenceDelta(10, 11)).toBe(1)
    expect(sequenceDelta(10, 15)).toBe(5)
    // Reordered packet arrives late.
    expect(sequenceDelta(10, 9)).toBe(-1)
    // Wrap: 0xFFFFFFFF -> 0 is a single step forward, not a 4-billion gap.
    expect(sequenceDelta(0xffff_ffff, 0)).toBe(1)
    expect(sequenceDelta(0xffff_fffe, 1)).toBe(3)
    expect(sequenceDelta(0, 0xffff_ffff)).toBe(-1)
  })

  it('pins the header size so receivers can hard-code the offset', () => {
    expect(PACKET_HEADER_BYTES).toBe(24)
    expect(AUDIO_PROTOCOL_VERSION).toBe(1)
  })
})

describe('transport policy', () => {
  it('refuses plaintext audio in production', () => {
    expect(isTransportAllowedInProduction('udp-plaintext')).toBe(false)
    expect(isTransportAllowedInProduction('udp-encrypted')).toBe(true)
  })

  it('defaults new installs to the encrypted transport', () => {
    expect(defaultSettings().streaming.transport).toBe('udp-encrypted')
  })
})

describe('session state machine', () => {
  it('defines transitions for every state', () => {
    const states: SessionState[] = [
      'idle',
      'preparing',
      'ready',
      'streaming',
      'stopping',
      'recovering',
      'error',
    ]
    for (const state of states) {
      expect(SESSION_TRANSITIONS[state], state).toBeDefined()
    }
  })

  it('allows the normal start and stop path', () => {
    expect(canTransition('idle', 'preparing')).toBe(true)
    expect(canTransition('preparing', 'ready')).toBe(true)
    expect(canTransition('ready', 'streaming')).toBe(true)
    expect(canTransition('streaming', 'stopping')).toBe(true)
    expect(canTransition('stopping', 'idle')).toBe(true)
  })

  it('forbids skipping preparation and double-starting', () => {
    expect(canTransition('idle', 'streaming')).toBe(false)
    expect(canTransition('streaming', 'streaming')).toBe(false)
    expect(canTransition('stopping', 'streaming')).toBe(false)
    expect(canTransition('idle', 'idle')).toBe(false)
  })

  it('only leaves the error state via idle or a fresh preparation', () => {
    expect(canTransition('error', 'idle')).toBe(true)
    expect(canTransition('error', 'preparing')).toBe(true)
    expect(canTransition('error', 'streaming')).toBe(false)
  })
})

describe('settings', () => {
  it('produces a fully defaulted object', () => {
    const settings = defaultSettings()
    expect(settings.audio.sampleRate).toBe(48_000)
    expect(settings.streaming.frameDurationMs).toBe(5)
    expect(settings.network.streamPort).toBeGreaterThan(1_023)
    expect(settings.startup.showSetupWizard).toBe(true)
  })

  it('flags the settings that need a stream restart', () => {
    expect(restartRequiredFor({ audio: { channelIndex: 3 } })).toEqual(['audio.channelIndex'])
    expect(restartRequiredFor({ appearance: { theme: 'light' } })).toEqual([])
    const several = restartRequiredFor({
      audio: { deviceId: 'x', sampleRate: 48_000 },
      streaming: { frameDurationMs: 10 },
    })
    expect(several).toContain('audio.deviceId')
    expect(several).toContain('audio.sampleRate')
    expect(several).toContain('streaming.frameDurationMs')
  })

  it('rejects an out-of-range frame duration', () => {
    expect(streamingSettingsSchema.safeParse({ frameDurationMs: 7 }).success).toBe(false)
    expect(streamingSettingsSchema.safeParse({ frameDurationMs: 10 }).success).toBe(true)
  })
})

describe('receiver telemetry labelling', () => {
  it('tags receiver telemetry with its provenance', () => {
    const parsed = receiverTelemetrySchema.safeParse({
      source: 'receiver-reported',
      receivedAt: 1,
      reportedAt: 1,
      packetsReceived: 100,
      packetsLost: 1,
      lossPercent: 1,
      jitterMs: 2,
      bufferHealthMs: 40,
      underruns: 0,
      playbackState: 'playing',
      outputLatencyMs: null,
    })
    expect(parsed.success).toBe(true)
    // The discriminator cannot be forged into something that looks measured.
    expect(
      receiverTelemetrySchema.safeParse({ source: 'measured', receivedAt: 1 }).success,
    ).toBe(false)
  })

  it('prefers the operator label over the self-reported device name', () => {
    const base: Receiver = {
      id: 'abcdef12-0000',
      deviceName: 'Pixel 8',
      displayName: null,
      platform: 'android',
      platformVersion: null,
      appVersion: null,
      ipAddress: '10.0.0.5',
      connectionState: 'connected',
      pairingState: 'approved',
      firstSeenAt: 0,
      lastSeenAt: 0,
      lastHeartbeatAt: null,
      packetsSentToReceiver: 0,
      packetsDroppedLocally: 0,
      telemetry: null,
    }
    expect(receiverLabel(base)).toBe('Pixel 8')
    expect(receiverLabel({ ...base, displayName: 'Lead Vocal' })).toBe('Lead Vocal')
    expect(receiverLabel({ ...base, displayName: '   ' })).toBe('Pixel 8')
    expect(receiverLabel({ ...base, deviceName: '', displayName: null })).toBe('abcdef12')
  })
})

describe('structured errors', () => {
  it('defaults retryable to false so callers must opt in', () => {
    const error = serviceError(ServiceErrorCode.AudioDeviceNotFound, 'gone')
    expect(error.retryable).toBe(false)
    expect(error.code).toBe('AUDIO_DEVICE_NOT_FOUND')
  })

  it('carries a field path for validation failures', () => {
    const error = serviceError(ServiceErrorCode.InvalidRequest, 'bad', {
      field: 'params.channelIndex',
    })
    expect(error.field).toBe('params.channelIndex')
  })
})
