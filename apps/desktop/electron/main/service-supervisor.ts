import { randomBytes } from 'node:crypto'
import { type ChildProcess, spawn } from 'node:child_process'
import { EventEmitter } from 'node:events'
import net from 'node:net'

import {
  CONTROL_PROTOCOL_VERSION,
  DEFAULT_REQUEST_TIMEOUT_MS,
  METHODS,
  SERVICE_HEALTHY_RUN_MS,
  SERVICE_MAX_RESTART_ATTEMPTS,
  SERVICE_RESTART_BACKOFF_MS,
  ServiceErrorCode,
  type ServiceError,
  type ServiceInfo,
  type ServiceState,
  checkControlCompatibility,
  isMethodName,
  serviceError,
} from '@stagelink/shared-protocol'

import { logServiceLine, logger } from './logger'
import { dataDirectory, isPackaged, resolveServiceBinary } from './paths'

/**
 * Spawns, supervises and talks to the Rust audio service.
 *
 * Responsibilities, in order of importance:
 *
 * 1. **Keep the audio alive.** The renderer reloading, or Electron's window
 *    closing to the tray, must not interrupt a session. The supervisor only
 *    shuts the service down on an explicit application quit.
 * 2. **Fail visibly.** A missing binary, a refused handshake or a crashed
 *    process produces a structured error the UI can show, never a silent
 *    no-op.
 * 3. **Never trust the renderer.** Every request is checked against the method
 *    registry and its zod params schema in this process, before it reaches the
 *    service.
 */
export class ServiceSupervisor extends EventEmitter {
  #child: ChildProcess | null = null
  #socket: net.Socket | null = null
  #buffer = ''
  #state: ServiceState = 'stopped'
  #pending = new Map<
    string,
    { resolve: (value: unknown) => void; reject: (error: ServiceError) => void; timer: NodeJS.Timeout }
  >()
  #nextRequestId = 1
  #restartCount = 0
  #restartTimer: NodeJS.Timeout | null = null
  #startedAt: number | null = null
  #lastError: ServiceError | null = null
  #info: Pick<
    ServiceInfo,
    'version' | 'controlProtocolVersion' | 'audioProtocolVersion' | 'availableAudioApis'
  > = {
    version: null,
    controlProtocolVersion: null,
    audioProtocolVersion: null,
    availableAudioApis: [],
  }
  #instanceId = ''
  #token = ''
  #quitting = false

  /** Current supervision snapshot, safe to send to the renderer. */
  get info(): ServiceInfo {
    return {
      state: this.#state,
      version: this.#info.version,
      controlProtocolVersion: this.#info.controlProtocolVersion,
      audioProtocolVersion: this.#info.audioProtocolVersion,
      pid: this.#child?.pid ?? null,
      startedAt: this.#startedAt,
      restartCount: this.#restartCount,
      lastError: this.#lastError,
      availableAudioApis: this.#info.availableAudioApis,
    }
  }

  /** Starts the service and connects to it. Safe to call when already running. */
  async start(): Promise<ServiceInfo> {
    if (this.#state === 'ready' || this.#state === 'starting' || this.#state === 'handshaking') {
      return this.info
    }

    const binary = resolveServiceBinary()
    if (!binary) {
      this.#fail(
        serviceError(
          ServiceErrorCode.ServiceUnavailable,
          'The StageLink audio service could not be found.',
          {
            detail: isPackaged()
              ? 'The installation looks incomplete. Reinstall StageLink Core.'
              : 'Build it first: npm run build:service',
          },
        ),
      )
      return this.info
    }

    // A fresh pipe name and token per launch: a token from a previous run is
    // useless, and the name cannot be squatted ahead of us.
    this.#instanceId = randomBytes(16).toString('hex')
    this.#token = randomBytes(24).toString('hex')

    this.#setState('starting')
    this.#lastError = null

    logger.info(`spawning audio service: ${binary}`)
    const child = spawn(
      binary,
      [
        '--instance-id',
        this.#instanceId,
        '--token',
        this.#token,
        '--data-dir',
        dataDirectory(),
        ...(isPackaged() ? ['--release-build'] : []),
      ],
      {
        // The token is on the command line rather than in the environment so
        // it is not inherited by anything the service might spawn.
        stdio: ['ignore', 'pipe', 'pipe'],
        windowsHide: true,
      },
    )
    this.#child = child
    this.#startedAt = Date.now()

    child.stdout?.setEncoding('utf8')
    child.stdout?.on('data', (chunk: string) => logServiceLine(chunk))
    child.stderr?.setEncoding('utf8')
    child.stderr?.on('data', (chunk: string) => {
      for (const line of chunk.split('\n')) logServiceLine(line)
    })

    child.on('exit', (code, signal) => this.#onChildExit(code, signal))
    child.on('error', (error) => {
      this.#fail(
        serviceError(ServiceErrorCode.ServiceUnavailable, 'The audio service failed to start.', {
          detail: error.message,
        }),
      )
    })

    try {
      await this.#connectWithRetry()
      await this.#handshake()
      this.#setState('ready')
      logger.info(`audio service ready (pid ${child.pid})`)
    } catch (error) {
      const payload =
        typeof error === 'object' && error !== null && 'code' in error
          ? (error as ServiceError)
          : serviceError(
              ServiceErrorCode.ServiceUnavailable,
              'Could not connect to the audio service.',
              { detail: error instanceof Error ? error.message : String(error) },
            )
      this.#fail(payload)
    }

    return this.info
  }

  /** Connects to the named pipe, retrying while the service creates it. */
  async #connectWithRetry(attempts = 60, delayMs = 100): Promise<void> {
    const pipePath = `\\\\.\\pipe\\stagelink-service-${this.#instanceId}`

    for (let attempt = 0; attempt < attempts; attempt += 1) {
      if (this.#child === null || this.#child.exitCode !== null) {
        throw serviceError(
          ServiceErrorCode.ServiceUnavailable,
          'The audio service exited before it could be reached.',
        )
      }
      try {
        const socket = await new Promise<net.Socket>((resolve, reject) => {
          const candidate = net.connect(pipePath)
          candidate.once('connect', () => resolve(candidate))
          candidate.once('error', (error) => {
            candidate.destroy()
            reject(error)
          })
        })
        socket.setEncoding('utf8')
        socket.on('data', (chunk: string) => this.#onData(chunk))
        socket.on('close', () => this.#onSocketClose())
        socket.on('error', (error) => logger.warn(`control channel error: ${error.message}`))
        this.#socket = socket
        return
      } catch {
        await delay(delayMs)
      }
    }
    throw serviceError(
      ServiceErrorCode.Timeout,
      'The audio service did not open its control channel.',
    )
  }

  /** Authenticates and agrees a protocol version. */
  async #handshake(): Promise<void> {
    this.#setState('handshaking')
    const socket = this.#socket
    if (!socket) throw serviceError(ServiceErrorCode.ServiceUnavailable, 'No control channel.')

    const welcome = await new Promise<Record<string, unknown>>((resolve, reject) => {
      const timer = setTimeout(
        () =>
          reject(
            serviceError(ServiceErrorCode.Timeout, 'The audio service did not answer the handshake.'),
          ),
        10_000,
      )
      this.once('welcome', (message: Record<string, unknown>) => {
        clearTimeout(timer)
        resolve(message)
      })
      this.once('channel-closed', () => {
        clearTimeout(timer)
        reject(
          serviceError(
            ServiceErrorCode.Unauthorized,
            'The audio service refused the connection.',
            {
              detail:
                'The handshake token or protocol version was rejected. This usually means a stale service binary.',
            },
          ),
        )
      })

      socket.write(
        `${JSON.stringify({
          v: 1,
          kind: 'hello',
          token: this.#token,
          // Set from `app.getVersion()` at startup. `npm_package_version` only
          // exists when launched through npm, so it is the fallback, not the
          // source — the service log should not claim a version we guessed.
          client: `stagelink-desktop/${process.env.STAGELINK_APP_VERSION ?? process.env.npm_package_version ?? 'unknown'}`,
          controlProtocolVersion: CONTROL_PROTOCOL_VERSION,
        })}\n`,
      )
    })

    const peerVersion = String(welcome.controlProtocolVersion ?? '')
    const compatibility = checkControlCompatibility(peerVersion)
    if (!compatibility.compatible) {
      throw serviceError(
        ServiceErrorCode.ProtocolMismatch,
        'The audio service speaks an incompatible protocol version.',
        { detail: compatibility.reason },
      )
    }

    this.#info = {
      version: String(welcome.serviceVersion ?? ''),
      controlProtocolVersion: peerVersion,
      audioProtocolVersion: Number(welcome.audioProtocolVersion ?? 0),
      availableAudioApis: Array.isArray(welcome.availableAudioApis)
        ? (welcome.availableAudioApis as ServiceInfo['availableAudioApis'])
        : [],
    }
  }

  /**
   * Issues a control request.
   *
   * The method name and its params are validated here, in the main process,
   * against the shared registry. A renderer cannot reach a method that is not
   * listed, and cannot send a payload the service did not agree to accept.
   */
  async invoke(method: string, params: unknown): Promise<unknown> {
    if (!isMethodName(method)) {
      throw serviceError(ServiceErrorCode.UnknownMethod, `${method} is not a control method.`)
    }
    const definition = METHODS[method]

    const parsed = definition.params.safeParse(params)
    if (!parsed.success) {
      const first = parsed.error.issues[0]
      throw serviceError(ServiceErrorCode.InvalidRequest, first?.message ?? 'Invalid parameters.', {
        field: first ? `params.${first.path.join('.')}` : undefined,
      })
    }

    if (this.#state !== 'ready' && this.#state !== 'degraded') {
      throw serviceError(
        ServiceErrorCode.ServiceUnavailable,
        'The audio service is not running.',
        { detail: `Service state: ${this.#state}.`, retryable: true },
      )
    }

    const socket = this.#socket
    if (!socket) {
      throw serviceError(ServiceErrorCode.ServiceUnavailable, 'No control channel.', {
        retryable: true,
      })
    }

    const id = String(this.#nextRequestId++)
    const timeoutMs = definition.timeoutMs ?? DEFAULT_REQUEST_TIMEOUT_MS

    const result = await new Promise<unknown>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pending.delete(id)
        reject(
          serviceError(ServiceErrorCode.Timeout, `${method} did not answer within ${timeoutMs} ms.`, {
            retryable: true,
          }),
        )
      }, timeoutMs)

      this.#pending.set(id, { resolve, reject, timer })
      socket.write(
        `${JSON.stringify({ v: 1, kind: 'request', id, method, params: parsed.data })}\n`,
      )
    })

    // Validate what came back too: a misbehaving or mismatched service must not
    // be able to put an unexpected shape into a Pinia store.
    const validated = definition.result.safeParse(result)
    if (!validated.success) {
      logger.error(
        `${method} returned an unexpected shape: ${validated.error.issues
          .map((issue) => `${issue.path.join('.')}: ${issue.message}`)
          .join('; ')}`,
      )
      throw serviceError(
        ServiceErrorCode.ProtocolMismatch,
        `The audio service returned an unexpected result for ${method}.`,
        { detail: validated.error.issues[0]?.message },
      )
    }
    return validated.data
  }

  /** Restarts the service on operator request. */
  async restart(): Promise<ServiceInfo> {
    logger.info('restarting the audio service on request')
    this.#restartCount = 0
    await this.stop('restart')
    return this.start()
  }

  /**
   * Shuts the service down in an orderly way.
   *
   * Asks politely first so the service can notify receivers and release the
   * capture device, then terminates if it does not go.
   */
  async stop(reason: 'app-quit' | 'os-shutdown' | 'restart' | 'operator-request'): Promise<void> {
    this.#quitting = reason === 'app-quit' || reason === 'os-shutdown'
    if (this.#restartTimer) {
      clearTimeout(this.#restartTimer)
      this.#restartTimer = null
    }

    const child = this.#child
    if (!child || child.exitCode !== null) {
      this.#cleanupChannel()
      this.#setState('stopped')
      return
    }

    if (this.#state === 'ready' || this.#state === 'degraded') {
      try {
        await this.invoke('service.shutdown', { reason, graceMs: 1_500 })
      } catch (error) {
        logger.warn(`graceful shutdown request failed: ${describe(error)}`)
      }
    }

    this.#socket?.end()
    this.#cleanupChannel()

    const exited = await Promise.race([
      new Promise<boolean>((resolve) => child.once('exit', () => resolve(true))),
      delay(3_000).then(() => false),
    ])
    if (!exited) {
      logger.warn('the audio service did not exit in time; terminating it')
      child.kill()
    }
    this.#child = null
    this.#setState('stopped')
  }

  // -- wire handling -------------------------------------------------------

  #onData(chunk: string): void {
    this.#buffer += chunk
    let index: number
    while ((index = this.#buffer.indexOf('\n')) >= 0) {
      const line = this.#buffer.slice(0, index)
      this.#buffer = this.#buffer.slice(index + 1)
      if (!line.trim()) continue
      this.#handleLine(line)
    }
    // A peer that never sends a newline must not be able to grow this forever.
    if (this.#buffer.length > 2_000_000) {
      logger.error('control channel sent an oversized line; dropping the connection')
      this.#buffer = ''
      this.#socket?.destroy()
    }
  }

  #handleLine(line: string): void {
    let message: Record<string, unknown>
    try {
      message = JSON.parse(line) as Record<string, unknown>
    } catch (error) {
      logger.warn(`discarding a malformed control message: ${describe(error)}`)
      return
    }

    switch (message.kind) {
      case 'welcome':
        this.emit('welcome', message)
        return

      case 'response': {
        const id = String(message.id ?? '')
        const entry = this.#pending.get(id)
        if (!entry) return
        this.#pending.delete(id)
        clearTimeout(entry.timer)
        if (message.ok === true) entry.resolve(message.result)
        else
          entry.reject(
            (message.error as ServiceError) ??
              serviceError(ServiceErrorCode.Internal, 'The service returned an empty error.'),
          )
        return
      }

      case 'event': {
        const event = String(message.event ?? '')
        if (!event) return
        // Forwarded verbatim; the renderer-facing layer validates payloads
        // against the event registry before they reach a store.
        this.emit('service-event', event, message.payload)
        if (event === 'service.error') {
          this.#setState('degraded')
        }
        return
      }

      default:
        logger.warn(`ignoring an unexpected control message kind: ${String(message.kind)}`)
    }
  }

  #onSocketClose(): void {
    this.emit('channel-closed')
    this.#cleanupChannel()
  }

  #cleanupChannel(): void {
    for (const [, entry] of this.#pending) {
      clearTimeout(entry.timer)
      entry.reject(
        serviceError(ServiceErrorCode.ServiceUnavailable, 'The control channel closed.', {
          retryable: true,
        }),
      )
    }
    this.#pending.clear()
    this.#buffer = ''
    this.#socket?.removeAllListeners()
    this.#socket = null
  }

  #onChildExit(code: number | null, signal: NodeJS.Signals | null): void {
    const ranFor = this.#startedAt ? Date.now() - this.#startedAt : 0
    logger.warn(`audio service exited (code ${code}, signal ${signal ?? 'none'}) after ${ranFor} ms`)
    this.#cleanupChannel()
    this.#child = null

    if (this.#quitting) {
      this.#setState('stopped')
      return
    }

    // A run that lasted a while was healthy; do not hold its restart against
    // the next attempt.
    if (ranFor >= SERVICE_HEALTHY_RUN_MS) {
      this.#restartCount = 0
    }

    if (this.#restartCount >= SERVICE_MAX_RESTART_ATTEMPTS) {
      this.#fail(
        serviceError(
          ServiceErrorCode.ServiceUnavailable,
          'The audio service keeps stopping and will not be restarted automatically.',
          {
            detail: `Gave up after ${this.#restartCount} attempts. See the log for details.`,
          },
        ),
        'failed',
      )
      return
    }

    const backoff =
      SERVICE_RESTART_BACKOFF_MS[this.#restartCount] ??
      SERVICE_RESTART_BACKOFF_MS[SERVICE_RESTART_BACKOFF_MS.length - 1] ??
      5_000
    this.#restartCount += 1
    this.#setState('restarting')
    logger.info(`restarting the audio service in ${backoff} ms (attempt ${this.#restartCount})`)

    this.#restartTimer = setTimeout(() => {
      this.#restartTimer = null
      void this.start()
    }, backoff)
  }

  #fail(error: ServiceError, state: ServiceState = 'failed'): void {
    this.#lastError = error
    logger.error(`${error.code}: ${error.message}${error.detail ? ` (${error.detail})` : ''}`)
    this.#setState(state)
  }

  #setState(state: ServiceState): void {
    if (this.#state === state) return
    this.#state = state
    this.emit('state', this.info)
    // The renderer subscribes to this as a normal service event, so a state
    // change needs no special-casing in the bridge.
    this.emit('service-event', 'service.state', this.info)
  }
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

function describe(error: unknown): string {
  if (error instanceof Error) return error.message
  if (typeof error === 'object' && error !== null && 'message' in error) {
    return String((error as { message: unknown }).message)
  }
  return String(error)
}
