import { z } from 'zod'

/**
 * Structured error codes. Every failure that crosses a process boundary carries
 * one of these so the UI can react programmatically instead of string-matching
 * on messages.
 */
export const ServiceErrorCode = {
  /** The request was malformed or failed schema validation. */
  InvalidRequest: 'INVALID_REQUEST',
  /** Method name is not known to this protocol version. */
  UnknownMethod: 'UNKNOWN_METHOD',
  /** The native service is not running / not yet connected. */
  ServiceUnavailable: 'SERVICE_UNAVAILABLE',
  /** The native service did not answer within the deadline. */
  Timeout: 'TIMEOUT',
  /** Peer speaks an incompatible protocol major version. */
  ProtocolMismatch: 'PROTOCOL_MISMATCH',
  /** Local IPC handshake token rejected. */
  Unauthorized: 'UNAUTHORIZED',

  /** No such audio device, or the device vanished. */
  AudioDeviceNotFound: 'AUDIO_DEVICE_NOT_FOUND',
  /** The device exists but refused the requested format. */
  AudioFormatUnsupported: 'AUDIO_FORMAT_UNSUPPORTED',
  /** The selected channel index is out of range for the device. */
  AudioChannelOutOfRange: 'AUDIO_CHANNEL_OUT_OF_RANGE',
  /** Capture could not be started (driver/exclusive-mode failure). */
  AudioCaptureFailed: 'AUDIO_CAPTURE_FAILED',
  /** The capture device disconnected mid-session. */
  AudioDeviceDisconnected: 'AUDIO_DEVICE_DISCONNECTED',

  /** Streaming cannot start because no input is captured. */
  StreamNoInput: 'STREAM_NO_INPUT',
  /** Socket bind failed (port in use, adapter gone). */
  StreamBindFailed: 'STREAM_BIND_FAILED',
  /** Refusing an insecure transport in a production build. */
  StreamTransportRefused: 'STREAM_TRANSPORT_REFUSED',

  /** Requested state transition is not legal from the current state. */
  InvalidStateTransition: 'INVALID_STATE_TRANSITION',
  /** Operation already in progress (duplicate start/stop). */
  OperationInProgress: 'OPERATION_IN_PROGRESS',

  /** No such receiver id. */
  ReceiverNotFound: 'RECEIVER_NOT_FOUND',
  /** Receiver limit reached. */
  ReceiverLimitReached: 'RECEIVER_LIMIT_REACHED',
  /** Pairing code wrong or expired. */
  PairingRejected: 'PAIRING_REJECTED',

  /** No such network adapter, or it has no usable address. */
  NetworkAdapterUnavailable: 'NETWORK_ADAPTER_UNAVAILABLE',

  /** Settings failed validation. */
  SettingsInvalid: 'SETTINGS_INVALID',
  /** Persistence layer failure. */
  StorageFailure: 'STORAGE_FAILURE',

  /** Anything genuinely unexpected. */
  Internal: 'INTERNAL',
} as const

export type ServiceErrorCode = (typeof ServiceErrorCode)[keyof typeof ServiceErrorCode]

export const serviceErrorCodeSchema = z.enum(
  Object.values(ServiceErrorCode) as [ServiceErrorCode, ...ServiceErrorCode[]],
)

export const serviceErrorSchema = z.object({
  code: serviceErrorCodeSchema,
  /** Human-readable, safe to show in the UI. */
  message: z.string().min(1),
  /** Optional operator-facing detail (driver text, OS error code, ...). */
  detail: z.string().optional(),
  /** True when retrying the same request could plausibly succeed. */
  retryable: z.boolean().default(false),
  /** Field path for validation errors, e.g. `params.sampleRate`. */
  field: z.string().optional(),
})

export type ServiceError = z.infer<typeof serviceErrorSchema>

/** Severity shared by operational errors, warnings and session events. */
export const severitySchema = z.enum(['info', 'warning', 'error', 'critical'])
export type Severity = z.infer<typeof severitySchema>

export const operationalErrorSchema = z.object({
  id: z.string(),
  at: z.number().int(),
  severity: severitySchema,
  code: serviceErrorCodeSchema,
  message: z.string(),
  detail: z.string().optional(),
  /** Which subsystem reported it. */
  source: z.enum(['audio', 'stream', 'network', 'session', 'receiver', 'storage', 'service']),
})

export type OperationalError = z.infer<typeof operationalErrorSchema>

/** Builds a `ServiceError` with the `retryable` default applied. */
export function serviceError(
  code: ServiceErrorCode,
  message: string,
  options: { detail?: string; retryable?: boolean; field?: string } = {},
): ServiceError {
  return {
    code,
    message,
    retryable: options.retryable ?? false,
    ...(options.detail !== undefined ? { detail: options.detail } : {}),
    ...(options.field !== undefined ? { field: options.field } : {}),
  }
}

/**
 * Error thrown on the TypeScript side when a control request fails. Carries the
 * structured payload so callers can branch on `code`.
 */
export class StageLinkServiceError extends Error {
  readonly code: ServiceErrorCode
  readonly detail: string | undefined
  readonly retryable: boolean
  readonly field: string | undefined

  constructor(error: ServiceError) {
    super(error.message)
    this.name = 'StageLinkServiceError'
    this.code = error.code
    this.detail = error.detail
    this.retryable = error.retryable
    this.field = error.field
  }

  toJSON(): ServiceError {
    return serviceError(this.code, this.message, {
      ...(this.detail !== undefined ? { detail: this.detail } : {}),
      retryable: this.retryable,
      ...(this.field !== undefined ? { field: this.field } : {}),
    })
  }
}

/** Narrows an unknown thrown value to a `ServiceError` payload. */
export function toServiceError(cause: unknown): ServiceError {
  if (cause instanceof StageLinkServiceError) return cause.toJSON()
  const parsed = serviceErrorSchema.safeParse(cause)
  if (parsed.success) return parsed.data
  if (cause instanceof Error) {
    return serviceError(ServiceErrorCode.Internal, cause.message, { detail: cause.stack })
  }
  return serviceError(ServiceErrorCode.Internal, 'Unknown error', {
    detail: typeof cause === 'string' ? cause : undefined,
  })
}
