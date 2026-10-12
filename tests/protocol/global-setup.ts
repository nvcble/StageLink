import { existsSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

/**
 * Ensures the Rust protocol probe exists before the cross-check tests run.
 *
 * Fails with the exact command to run rather than letting every test fail with
 * a confusing spawn error.
 */
export default function setup(): void {
  const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..')
  const executable = process.platform === 'win32' ? 'protocol-probe.exe' : 'protocol-probe'

  const found = ['debug', 'release'].some((profile) =>
    existsSync(path.join(repoRoot, 'crates', 'target', profile, executable)),
  )

  if (!found) {
    throw new Error(
      'The Rust protocol probe is not built, so the cross-implementation tests ' +
        'cannot run.\n\n  cargo build --manifest-path crates/Cargo.toml --bin protocol-probe\n',
    )
  }
}
