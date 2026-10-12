import path from 'node:path'
import log from 'electron-log/main'

import { logDirectory } from './paths'

/**
 * Main-process logging.
 *
 * Writes to a rotated file under the StageLink data directory, so a problem at
 * a Sunday service can be looked at on Monday. Service stderr is forwarded here
 * too, which means one file holds the whole story across both processes.
 */
export function initLogging(level: 'error' | 'warn' | 'info' | 'debug' = 'info'): void {
  log.transports.file.resolvePathFn = () => path.join(logDirectory(), 'stagelink-desktop.log')
  log.transports.file.level = level
  log.transports.file.maxSize = 5 * 1024 * 1024
  log.transports.console.level = level
  log.transports.console.format = '[{level}] {text}'
  log.initialize()
  log.info('--- StageLink Core starting ---')
}

export const logger = log

/** Forwards a line of service stderr, preserving its level where parseable. */
export function logServiceLine(line: string): void {
  const trimmed = line.trimEnd()
  if (!trimmed) return
  if (/\bERROR\b/.test(trimmed)) log.error(`[service] ${trimmed}`)
  else if (/\bWARN\b/.test(trimmed)) log.warn(`[service] ${trimmed}`)
  else if (/\bDEBUG\b|\bTRACE\b/.test(trimmed)) log.debug(`[service] ${trimmed}`)
  else log.info(`[service] ${trimmed}`)
}
