/**
 * Detecting payload fields the protocol silently discards.
 *
 * Zod strips undeclared keys rather than complaining, and it is the *parsed*
 * payload that is forwarded to the renderer. So a field the service sends but
 * the schema does not declare simply vanishes: no validation error, no log, no
 * sign that anything happened. A missing declared field is loud; an undeclared
 * extra field is invisible, which makes it the more dangerous of the two.
 *
 * That is not hypothetical. `engineConverted` was sent on every `audio.status`
 * event and never declared in `captureFormatSchema`, so the renderer's
 * "Windows is resampling your input" warning could not fire — a
 * latency-relevant alert that had never once been seen in the product.
 *
 * Kept free of any `electron` import so it can be tested in a plain Node
 * process, like `electron/main/protocol.ts`.
 */

/**
 * Dotted paths present in `sent` but absent from `kept`.
 *
 * Recursive, because the field that motivated this was nested two levels down
 * (`format.engineConverted`) and a shallow key comparison would have missed it.
 * Arrays are not descended into: the schemas describe arrays by element shape,
 * and reporting `receivers.3.foo` would name an index that means nothing to
 * whoever reads the log.
 */
export function droppedKeys(sent: unknown, kept: unknown, path = ''): string[] {
  if (!isPlainObject(sent) || !isPlainObject(kept)) return []

  const dropped: string[] = []
  for (const [key, value] of Object.entries(sent)) {
    const here = path ? `${path}.${key}` : key
    if (!(key in kept)) {
      dropped.push(here)
      continue
    }
    dropped.push(...droppedKeys(value, kept[key], here))
  }
  return dropped
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}
