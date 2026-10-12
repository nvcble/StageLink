/**
 * StageLink protocol versioning.
 *
 * Three independent version numbers travel over the wire, because the three
 * links in the chain evolve at different speeds:
 *
 * - `CONTROL_PROTOCOL_VERSION`  Electron main <-> Rust service (local IPC) and
 *                              desktop <-> receiver WebSocket control channel.
 * - `AUDIO_PROTOCOL_VERSION`    The binary audio packet header on the wire.
 * - `IPC_ENVELOPE_VERSION`      The JSON envelope that frames control messages.
 *
 * Compatibility rule: a differing MAJOR is a hard incompatibility and the
 * connection is refused. A newer MINOR on either side is tolerated; unknown
 * fields must be ignored rather than rejected so that a newer receiver can talk
 * to an older desktop.
 */

/** JSON envelope framing version. Bumped only when the envelope shape changes. */
export const IPC_ENVELOPE_VERSION = 1 as const

/** Control-plane semantic version (requests, responses, events). */
export const CONTROL_PROTOCOL_VERSION = '1.0.0' as const

/** Binary audio packet header version. Written into every audio datagram. */
export const AUDIO_PROTOCOL_VERSION = 1 as const

/** Minimum control-protocol MAJOR this build can talk to. */
export const CONTROL_PROTOCOL_MIN_MAJOR = 1 as const

export interface SemanticVersion {
  major: number
  minor: number
  patch: number
}

/** Parses `"1.2.3"`. Returns `null` for anything that is not a 3-part semver. */
export function parseSemanticVersion(value: string): SemanticVersion | null {
  const match = /^(\d+)\.(\d+)\.(\d+)(?:[-+].*)?$/.exec(value.trim())
  if (!match) return null
  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
  }
}

export type VersionCompatibility =
  | { compatible: true; reason?: undefined }
  | { compatible: false; reason: string }

/**
 * Decides whether a peer advertising `peerVersion` may connect.
 *
 * Same MAJOR is required. A peer MINOR newer than ours is accepted: the peer is
 * responsible for not relying on fields we do not send, and we ignore fields we
 * do not understand.
 */
export function checkControlCompatibility(
  peerVersion: string,
  ourVersion: string = CONTROL_PROTOCOL_VERSION,
): VersionCompatibility {
  const peer = parseSemanticVersion(peerVersion)
  const ours = parseSemanticVersion(ourVersion)

  if (!peer) {
    return { compatible: false, reason: `Unparseable protocol version "${peerVersion}".` }
  }
  if (!ours) {
    return { compatible: false, reason: `Unparseable local protocol version "${ourVersion}".` }
  }
  if (peer.major !== ours.major) {
    return {
      compatible: false,
      reason:
        `Protocol major version mismatch: peer speaks ${peer.major}.x, ` +
        `this build speaks ${ours.major}.x.`,
    }
  }
  if (peer.major < CONTROL_PROTOCOL_MIN_MAJOR) {
    return {
      compatible: false,
      reason: `Protocol major ${peer.major} is below the minimum supported ${CONTROL_PROTOCOL_MIN_MAJOR}.`,
    }
  }
  return { compatible: true }
}
