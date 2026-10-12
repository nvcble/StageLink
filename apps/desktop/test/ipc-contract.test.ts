import { describe, expect, it } from 'vitest'

import {
  BRIDGE_CHANNELS,
  EVENTS,
  EVENT_NAMES,
  METHODS,
  METHOD_NAMES,
  ServiceErrorCode,
  isBridgeErrorPayload,
  isEventName,
  isMethodName,
  toServiceError,
} from '@stagelink/shared-protocol'

/**
 * IPC contract tests.
 *
 * These guard the boundary between the renderer and the main process. The thing
 * being protected is the property that the renderer can only reach a fixed,
 * enumerable set of operations — there is no generic "run this command" path,
 * and no way to pass a channel name as data.
 */

describe('the renderer-facing channel surface', () => {
  it('is small and fully enumerated', () => {
    const channels = Object.values(BRIDGE_CHANNELS)
    // If this count changes, a new capability was granted to the renderer and
    // somebody should have thought about it.
    expect(channels).toHaveLength(8)
    expect(new Set(channels).size).toBe(channels.length)
  })

  it('namespaces every channel so it cannot collide with anything else', () => {
    for (const channel of Object.values(BRIDGE_CHANNELS)) {
      expect(channel.startsWith('stagelink:')).toBe(true)
    }
  })

  it('exposes no channel that takes a command name as data', () => {
    // A channel called something like "stagelink:exec" would be a generic
    // escape hatch. The invoke channel is the only dispatcher, and it validates
    // against the method registry.
    const names = Object.keys(BRIDGE_CHANNELS)
    expect(names).not.toContain('exec')
    expect(names).not.toContain('eval')
    expect(names).not.toContain('shell')
  })
})

describe('method registry as a whitelist', () => {
  it('admits only registered names', () => {
    expect(isMethodName('audio.getStatus')).toBe(true)
    expect(isMethodName('audio.getstatus')).toBe(false)
    expect(isMethodName('system.exec')).toBe(false)
    expect(isMethodName('')).toBe(false)
  })

  it('is not fooled by prototype properties', () => {
    // `method in METHODS` would return true for these; the registry uses
    // hasOwnProperty for exactly this reason.
    for (const name of ['__proto__', 'constructor', 'toString', 'valueOf', 'hasOwnProperty']) {
      expect(isMethodName(name), name).toBe(false)
    }
  })

  it('gives every method a params schema that rejects a non-object', () => {
    for (const name of METHOD_NAMES) {
      const definition = METHODS[name]
      // A string or an array must never be accepted where an object is
      // expected, whatever the method.
      expect(definition.params.safeParse('not an object').success, name).toBe(false)
      expect(definition.params.safeParse(42).success, name).toBe(false)
    }
  })

  it('gives every method a result schema', () => {
    for (const name of METHOD_NAMES) {
      expect(METHODS[name].result, name).toBeDefined()
      // Validating the result means a misbehaving service cannot inject an
      // unexpected shape into a Pinia store.
      expect(typeof METHODS[name].result.safeParse, name).toBe('function')
    }
  })

  it('bounds every method with a timeout', () => {
    for (const name of METHOD_NAMES) {
      const timeout = METHODS[name].timeoutMs
      expect(timeout, name).toBeGreaterThan(0)
      // Nothing should be able to hang the UI for a minute.
      expect(timeout, name).toBeLessThanOrEqual(45_000)
    }
  })

  it('gives hardware operations a longer deadline than pure reads', () => {
    expect(METHODS['audio.selectDevice'].timeoutMs).toBeGreaterThan(
      METHODS['audio.getStatus'].timeoutMs,
    )
    expect(METHODS['service.ping'].timeoutMs).toBeLessThanOrEqual(
      METHODS['audio.getStatus'].timeoutMs,
    )
  })

  it('marks reads as non-mutating and writes as mutating', () => {
    expect(METHODS['audio.getStatus'].mutates).toBe(false)
    expect(METHODS['settings.get'].mutates).toBe(false)
    expect(METHODS['diagnostics.getSnapshot'].mutates).toBe(false)
    expect(METHODS['audio.selectDevice'].mutates).toBe(true)
    expect(METHODS['stream.start'].mutates).toBe(true)
    expect(METHODS['receivers.revoke'].mutates).toBe(true)
  })
})

describe('event registry as a whitelist', () => {
  it('admits only registered names', () => {
    expect(isEventName('audio.levels')).toBe(true)
    expect(isEventName('audio.samples')).toBe(false)
    expect(isEventName('__proto__')).toBe(false)
  })

  it('validates every event payload before it can reach a store', () => {
    for (const name of EVENT_NAMES) {
      const schema = EVENTS[name]
      // An event carrying a string where an object belongs must be dropped.
      expect(schema.safeParse('whatever').success, name).toBe(false)
    }
  })

  it('rejects a malformed meter frame', () => {
    const schema = EVENTS['audio.levels']
    expect(
      schema.safeParse({
        at: 1,
        rmsDb: -20,
        peakDb: -6,
        peakHoldDb: -6,
        clipping: false,
        clipCount: 0,
        windowFrames: 1_920,
      }).success,
    ).toBe(true)
    // A negative clip count is not a thing.
    expect(
      schema.safeParse({
        at: 1,
        rmsDb: -20,
        peakDb: -6,
        peakHoldDb: -6,
        clipping: false,
        clipCount: -5,
        windowFrames: 1_920,
      }).success,
    ).toBe(false)
  })

  it('accepts null decibel values, because silence is a real reading', () => {
    expect(
      EVENTS['audio.levels'].safeParse({
        at: 1,
        rmsDb: null,
        peakDb: null,
        peakHoldDb: null,
        clipping: false,
        clipCount: 0,
        windowFrames: 0,
      }).success,
    ).toBe(true)
  })

  it('never defines an event that would carry audio samples', () => {
    // Audio must not cross the Electron IPC boundary at any rate. If a future
    // change added such an event, this would catch it.
    for (const name of EVENT_NAMES) {
      expect(name).not.toMatch(/sample|pcm|buffer|frame(s)?$/i)
    }
  })
})

describe('phase errors survive to the renderer', () => {
  it('accepts NOT_IMPLEMENTED as a declared code', () => {
    // Regression: NOT_IMPLEMENTED was emitted by the service but absent from
    // the enum, so toServiceError failed its parse and rewrote it as
    // INTERNAL / "Unknown error". Every Phase-3/5 method was mislabelled in
    // the UI, and the invariant that unimplemented work names its phase was
    // broken at the bridge. Nothing asserted it through the bridge, only over
    // the pipe, so it went unnoticed.
    const fromService = {
      code: 'NOT_IMPLEMENTED',
      message: 'stream.start is not available in this build.',
      detail: 'Scheduled for Phase 3 (audio streaming).',
      retryable: false,
    }

    const crossed = toServiceError(fromService)
    expect(crossed.code).toBe('NOT_IMPLEMENTED')
    expect(crossed.detail).toContain('Phase 3')
    expect(crossed.message).toContain('not available in this build')
  })

  it('still collapses a genuinely unknown shape to INTERNAL', () => {
    expect(toServiceError({ nonsense: true }).code).toBe('INTERNAL')
    expect(toServiceError('a bare string').code).toBe('INTERNAL')
  })

  it('preserves every declared code across the boundary', () => {
    for (const code of Object.values(ServiceErrorCode)) {
      const crossed = toServiceError({ code, message: 'x', retryable: false })
      expect(crossed.code, `${code} must survive`).toBe(code)
    }
  })
})

describe('error envelopes', () => {
  it('recognises a tagged bridge error', () => {
    expect(
      isBridgeErrorPayload({
        __stagelinkError: true,
        error: { code: 'INTERNAL', message: 'x', retryable: false },
      }),
    ).toBe(true)
  })

  it('does not mistake an ordinary result for an error', () => {
    expect(isBridgeErrorPayload({ state: 'running' })).toBe(false)
    expect(isBridgeErrorPayload(null)).toBe(false)
    expect(isBridgeErrorPayload(undefined)).toBe(false)
    expect(isBridgeErrorPayload('error')).toBe(false)
    expect(isBridgeErrorPayload({ __stagelinkError: 'yes' })).toBe(false)
  })
})
