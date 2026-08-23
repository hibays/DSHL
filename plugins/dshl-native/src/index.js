// @dshl/native — the dshl native backend dsh plugin.
//
// Loads the dshl-native napi-rs addon (.node DLL) and exposes its full
// capability set (window / tray / supervisor / terminal / OS actions) as the
// `dshlNativeBackend` Cordis service. When the addon is unavailable (no
// platform-matching binary installed), `backend` is null — callers
// feature-detect with a truthy check.
//
// The addon links the FULL dshl kernel, so this backend is NOT a subset of
// the installer track — it runs the same kernel inside the hosting dsh/Node
// process via FFI:
//   Track A (installer exe) : dshl.exe calls dshl_core::run_cli()
//   Track B (plugin DLL)    : addon.launch(opts) calls dshl_core::run_with_options()

export const name = 'dshl-native'

import { loadAddon, lastLoadError } from './loader.js'

const addon = loadAddon()

const hasNative = addon !== null

export { lastLoadError }

// napi-derive exposes snake_case Rust symbols to JS as camelCase
// (terminal_spawn -> terminalSpawn, ws_url -> wsUrl, started_at_ms ->
// startedAtMs, url_prefix -> urlPrefix).

function buildActions() {
  if (!hasNative) return null
  return {
    openTerminal: ({ cwd, path }) =>
      addon.openTerminal({ cwd: cwd ?? process.cwd(), path }) ?? false,
    openPath: (p) => addon.openPath(p) ?? false,
    openUrl: (u) => addon.openUrl(u) ?? false,
    ping: () => addon.ping?.() ?? { pong: true, version: '' },
    platformInfo: () =>
      addon.platformInfo?.() ?? { os: process.platform, arch: process.arch, shell: 'bash' },
  }
}

function buildWindow() {
  if (!hasNative) return null
  return {
    // Propagate the boot-window skip signal (`false`) from the kernel so
    // @dshl/control can answer 409 honestly; legacy .node builds that return
    // undefined are treated as success.
    show: () => addon.windowShow?.() !== false,
    hide: () => (addon.windowHide?.(), true),
    navigate: (u) => (addon.windowNavigate?.(u), true),
    isVisible: () => !!addon.windowIsVisible?.(),
  }
}

function buildTray() {
  if (!hasNative) return null
  return {
    show: () => (addon.trayShow?.(), true),
    hide: () => (addon.trayHide?.(), true),
    setIcon: (dark) => (addon.traySetIcon?.(!!dark), true),
    isVisible: () => !!addon.trayIsVisible?.(),
  }
}

function buildSupervisor() {
  if (!hasNative) return null
  return {
    shutdown: async () => {
      const fromKernel = !!addon.shutdown?.()
      return { ok: true, native: true, kernelRunning: !!addon.isKernelRunning?.(), fromKernel }
    },
    restart: async () => {
      if (addon.isKernelRunning?.()) {
        const ok = !!addon.restart?.(null)
        return { ok, native: true, inKernel: true }
      }
      const ok = !!addon.restart?.({
        cmd: process.execPath,
        args: process.argv.slice(1),
        cwd: process.cwd(),
        path: process.env.PATH,
      })
      if (!ok) return { ok: false, native: true, inKernel: false, error: 'failed to spawn restart child' }
      return { ok: true, native: true, inKernel: false, shouldExitHost: true }
    },
    launch: async (opts) => {
      const result = addon.launch?.(opts ?? {})
      return { ok: result === true || result === false, started: !!result }
    },
  }
}

function buildTerminal() {
  if (!hasNative || typeof addon.terminalSpawn !== 'function') return null
  return {
    async spawn(opts = {}) {
      const input = {}
      if (typeof opts.shell === 'string' && opts.shell.length > 0) input.shell = opts.shell
      if (typeof opts.cwd === 'string' && opts.cwd.length > 0) input.cwd = opts.cwd
      else input.cwd = process.cwd()
      if (opts.env && typeof opts.env === 'object') input.env = opts.env
      if (Array.isArray(opts.prependPath) && opts.prependPath.length > 0) {
        // napi-derive exposes object fields as camelCase — the key MUST be
        // prependPath here or napi silently drops it and the shell boots
        // without the dsh runtime PATH prefix.
        input.prependPath = opts.prependPath.map(String)
      }
      if (typeof opts.cols === 'number' && Number.isFinite(opts.cols)) input.cols = opts.cols | 0
      if (typeof opts.rows === 'number' && Number.isFinite(opts.rows)) input.rows = opts.rows | 0
      const r = await Promise.resolve(addon.terminalSpawn(input))
      return { id: r.id, pid: Number(r.pid), wsUrl: r.wsUrl }
    },
    async list() {
      const arr = await Promise.resolve(addon.terminalList?.() ?? [])
      return arr.map((s) => ({
        id: s.id,
        pid: Number(s.pid),
        shell: s.shell,
        cwd: s.cwd,
        startedAtMs: Number(s.startedAtMs ?? s.started_at_ms),
        alive: !!s.alive,
      }))
    },
    async kill(id) {
      return !!addon.terminalKill?.(String(id))
    },
    async resize(id, cols, rows) {
      return !!addon.terminalResize?.(String(id), Number(cols) | 0, Number(rows) | 0)
    },
    async write(id, data) {
      return !!addon.terminalWrite?.(String(id), String(data))
    },
    async endpoint() {
      const r = await Promise.resolve(addon.terminalWsEndpoint?.() ?? null)
      if (!r) return null
      return {
        host: r.host,
        port: Number(r.port),
        token: r.token,
        urlPrefix: r.urlPrefix ?? r.url_prefix,
      }
    },
  }
}

function buildStatus() {
  if (!hasNative) return null
  const version = (() => {
    const p = addon.ping?.()
    return typeof p?.version === 'string' ? p.version : null
  })()
  return () => {
    const ls = addon.launchStatus?.() ?? {}
    return {
      backend: 'native',
      version,
      launched: !!ls.launched,
      kernelRunning: !!ls.kernelRunning,
      windowVisible: !!ls.windowVisible,
      trayVisible: !!ls.trayVisible,
    }
  }
}

/**
 * The native backend object, or null when the addon is unavailable.
 * Shape: { backend, version, window, tray, supervisor, terminal, actions, status, isKernelRunning }
 */
export const backend = hasNative
  ? {
      backend: 'native',
      version: addon.ping?.()?.version ?? null,
      window: buildWindow(),
      tray: buildTray(),
      supervisor: buildSupervisor(),
      terminal: buildTerminal(),
      actions: buildActions(),
      status: buildStatus(),
      isKernelRunning: () => !!addon.isKernelRunning?.(),
    }
  : null

export const inject = []

export function apply(ctx) {
  // Always register the service, even when the .node failed to load: the
  // container is how @dshl/control discovers us (ctx.get), and carrying
  // `loadError` on the descriptor keeps the failure diagnosable through the
  // same path instead of vanishing into an absent registration.
  ctx.provide(
    'dshlNativeBackend',
    backend ?? {
      backend: null,
      version: null,
      window: null,
      tray: null,
      supervisor: null,
      terminal: null,
      actions: null,
      status: null,
      isKernelRunning: () => false,
      loadError: lastLoadError?.() ?? null,
    },
  )

  if (hasNative) {
    ctx.logger.info(`[dshl-native] addon loaded (version=${backend.version ?? 'n/a'})`)
  } else {
    const cause = lastLoadError()
    ctx.logger.warn(
      `[dshl-native] addon unavailable` +
        (cause ? `: ${cause instanceof Error ? cause.message : cause}` : ''),
    )
  }
}
