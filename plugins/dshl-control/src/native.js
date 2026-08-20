// @dshl/control — optional native addon loader (Backend B plugin track).
//
// The addon is a napi-rs-built .node dll giving the pure-JS plugin the same
// OS primitives as the launcher binary (open-terminal / open-path / open-url
// / platform info) without needing a running dshl process. It is OPTIONAL:
// the plugin uses it when it is present and otherwise falls back to the
// control pipe.
//
// Resolution order:
//   1. the `@dshl/native` npm package, whose per-platform optionalDependencies
//      select the right `.node` for the host (the published plugin-track
//      distribution);
//   2. a `native/` directory next to this plugin (repo-local builds, produced
//      by `scripts/build-native.mjs`).
import { createRequire } from 'node:module'
import { existsSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

const require = createRequire(import.meta.url)
const here = dirname(fileURLToPath(import.meta.url))

// npm package name per platform/arch, matching the napi-rs publish convention.
const PACKAGES = {
  win32: { x64: '@dshl/native-win32-x64-msvc', arm64: '@dshl/native-win32-arm64-msvc' },
  darwin: { x64: '@dshl/native-darwin-x64', arm64: '@dshl/native-darwin-arm64' },
  linux: { x64: '@dshl/native-linux-x64-gnu', arm64: '@dshl/native-linux-arm64-gnu' },
}

function loadAddon() {
  const pkg = PACKAGES[process.platform]?.[process.arch]
  if (pkg) {
    try {
      // The wrapper package's main points at the right .node for this platform.
      return require(pkg)
    } catch {
      // not installed — fall through to the local build
    }
  }
  const local = join(here, '..', 'native', `dshl-native.${process.platform}-${process.arch}.node`)
  if (existsSync(local)) {
    try {
      return require(local)
    } catch (cause) {
      throw new Error(`[dshl-control] native addon present but unloadable: ${local}`, { cause })
    }
  }
  return null
}

/** The loaded addon module, or null when no native build is available. */
export const native = loadAddon()