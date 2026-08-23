// @dshl/native — native addon loader.
//
// Resolves the per-platform `.node` cdylib built from `crates/dshl-native`
// (napi-rs). The DLL links the FULL dshl kernel (webui window + tray icon +
// setup pipeline + supervisor loop + OS actions + PTY terminal) into a single
// per-platform binary. The plugin track runs the same kernel inside the
// hosting dsh/Node process via FFI — dual track = two ENTRY POINTS into the
// same Rust code.
//
// Resolution order:
//   1. the per-platform `@dshl/native-*` package declared in
//      `optionalDependencies` — npm/pnpm only installs the one that matches
//      the current host;
//   2. a `native/` directory next to this plugin (repo-local builds, produced
//      by `scripts/build-native.mjs`, gitignored).
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

let loadError = null

/**
 * The error that made the addon unloadable, if any. Null when the addon
 * loaded fine or is simply not installed. Surfaced through the package
 * export so hosts can log WHY the backend is null instead of showing a
 * bare `backend=none`.
 */
export function lastLoadError() { return loadError }

/** Load the native addon or return null when unavailable. Never throws: an
 *  addon that is present but unloadable degrades to null + lastLoadError(),
 *  so one broken binary cannot take the whole module (and every consumer of
 *  it) down. */
export function loadAddon() {
  loadError = null
  const pkg = PACKAGES[process.platform]?.[process.arch]
  if (pkg) {
    try {
      return require(pkg)
    } catch (cause) {
      // "Package not installed" is the expected miss — fall through to the
      // local build. Anything else (truncated download, wrong Node ABI,
      // missing VC runtime) means the addon IS installed but broken, and
      // must stay diagnosable instead of silently looking like a miss.
      if (!(cause?.code === 'MODULE_NOT_FOUND' && String(cause?.message ?? '').includes(pkg))) {
        loadError = cause
      }
    }
  }
  const local = join(here, '..', 'native', `dshl-native.${process.platform}-${process.arch}.node`)
  if (existsSync(local)) {
    try {
      return require(local)
    } catch (cause) {
      loadError = new Error(`[dshl-native] addon present but unloadable: ${local}`, { cause })
      return null
    }
  }
  return null
}
