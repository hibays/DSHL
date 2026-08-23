// @dshl/control — the dshl control bridge (top-level aggregator).
//
// This plugin is the THIN ABSTRACTION LAYER. It depends on two sibling dsh
// plugins for the actual backend implementation:
//
//   * @dshl/native  → provides `dshlNativeBackend` (full kernel via napi-rs DLL)
//   * @dshl/pipe    → provides `dshlPipeBackend` (legacy remote control pipe)
//
// Both sibling plugins are optional (declared in optionalDependencies). This
// plugin try-requires each one at apply time via createRequire (Node 22+
// supports require(esm) synchronously). When a sibling is not installed the
// require throws and the backend is null — routes gracefully return 501.
//
// Responsibilities retained by this top-level plugin:
//   * folding the two backends into the unified `nativeCapabilities` service,
//   * HTTP routes (state, window, tray, supervisor, terminal, plugin-guard),
//   * the floating UI action bar injection,
//   * the `desktopPlugins` + `dshlPluginGuard` market contract (plugin-guard),
//   * the `desktopActions` back-compat shim.
//
// Backend selection order: native first, pipe as remote fallback.

import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { PluginGuard } from './plugin-guard.js'

const require = createRequire(import.meta.url)

// Try-require the two sibling backend plugins. Node 22+ supports
import { checkBackend, resolveLaunchOptions } from './backend-contract.js'

// Optional-seam consumption (canonical `ctx.get` idiom): a provider that is
// not loaded simply resolves to undefined. Never `require()` here — bypassing
// the container splits lifecycle ownership and freezes the binding at
// module-eval time.
let nativeBackend = null
let nativeLoadError = null
let pipeBackend = null

export const name = 'dshl-control'
export const inject = ['webServer']

const ROUTES = {
  state: '/dshl-control/state',
  openTerminal: '/dshl-control/open-terminal',
  restart: '/dshl-control/restart',
  shutdown: '/dshl-control/shutdown',
  windowShow: '/dshl-control/window/show',
  windowHide: '/dshl-control/window/hide',
  windowNavigate: '/dshl-control/window/navigate',
  trayShow: '/dshl-control/tray/show',
  trayHide: '/dshl-control/tray/hide',
  traySetIcon: '/dshl-control/tray/icon',
  launch: '/dshl-control/launch',
  ui: '/dshl-control/ui.js',
  guardPlugins: '/dshl-control/plugins/list',
  guardDisabled: '/dshl-control/plugins/disabled',
  guardDisable: null, // pattern /dshl-control/plugins/:name/disable
  guardEnable: null,  // pattern /dshl-control/plugins/:name/enable
  guardRollback: '/dshl-control/plugins/rollback',
  guardMarkHealthy: '/dshl-control/plugins/mark-healthy',
  guardMarkFailed: '/dshl-control/plugins/mark-failed',
  termSpawn: '/dshl-control/terminal/spawn',
  termList: '/dshl-control/terminal/list',
  termKill: '/dshl-control/terminal/kill',
  termResize: '/dshl-control/terminal/resize',
  termWrite: '/dshl-control/terminal/write',
  termEndpoint: '/dshl-control/terminal/endpoint',
}

function sendJson(res, status, value) {
  const body = JSON.stringify(value)
  res.statusCode = status
  res.setHeader('content-type', 'application/json; charset=utf-8')
  res.setHeader('cache-control', 'no-store')
  res.setHeader('x-content-type-options', 'nosniff')
  res.end(body)
}

function requestAllowed(req) {
  const remote = req.socket?.remoteAddress
  if (typeof remote !== 'string' || !(remote === '127.0.0.1' || remote === '::1' || remote === '::ffff:127.0.0.1')) {
    return false
  }
  const host = req.headers.host
  if (typeof host !== 'string') return false
  try {
    const authority = new URL(`http://${host}`)
    return authority.hostname === '127.0.0.1' || authority.hostname === 'localhost' || authority.hostname === '::1'
  } catch {
    return false
  }
}

function asPost(req, res) {
  if (req.method !== 'POST') {
    sendJson(res, 405, { error: 'requires a local POST' })
    return false
  }
  if (!requestAllowed(req)) {
    sendJson(res, 403, { error: 'dshl control authority rejected' })
    return false
  }
  return true
}

function parseJsonBody(req, limit = 64 * 1024) {
  return new Promise((resolve, reject) => {
    let body = ''
    req.on('data', (chunk) => {
      body += chunk
      if (body.length > limit) { reject(new Error('body too large')); req.destroy() }
    })
    req.on('end', () => {
      if (body === '') return resolve({})
      try { resolve(JSON.parse(body)) } catch (e) { reject(e) }
    })
    req.on('error', reject)
  })
}

// ---------------------------------------------------------------------------
// Fold native + pipe backends into a single unified capabilities object.
// Native takes priority; pipe is the remote fallback. When neither is
// available, all capability fields are null — routes return 501.
// ---------------------------------------------------------------------------

function buildCapabilities(ctx) {
  // `backend === 'native'` is the capability marker: the service is ALWAYS
  // registered by @dshl/native, and `backend: null` means its .node failed
  // to load (see nativeLoadError below).
  const hasNative = nativeBackend?.backend === 'native'
  const hasPipe = pipeBackend !== null

  // Service Definition check (backend-contract.js): warn on drift instead of
  // throwing — a missing method degrades one route to 501, it should not take
  // the bridge down.
  for (const [tier, be] of [['native', nativeBackend], ['pipe', pipeBackend]]) {
    if (!be) continue
    const problems = checkBackend(tier, be)
    if (problems.length) {
      ctx.logger.warn(`[dshl-control] backend contract drift (${tier}): ${problems.join('; ')}`)
    }
  }

  const backend = hasNative ? 'native' : hasPipe ? 'pipe' : null
  const version = hasNative ? nativeBackend.version : hasPipe ? null : null

  const actions = hasNative
    ? nativeBackend.actions
    : hasPipe
      ? pipeBackend.actions
      : null

  const window = hasNative ? nativeBackend.window : null
  const tray = hasNative ? nativeBackend.tray : null
  const terminal = hasNative ? nativeBackend.terminal : null

  // Supervisor: prefer native; fall back to pipe; if neither, a stub that
  // returns "empty" so the route layer can decide exit semantics.
  const supervisor = hasNative
    ? nativeBackend.supervisor
    : hasPipe
      ? pipeBackend.supervisor
      : {
          shutdown: async () => ({ ok: true, empty: true }),
          restart: async () => ({ ok: false, empty: true, error: 'no backend available' }),
          launch: async () => ({ ok: false, error: 'native addon unavailable' }),
        }

  const status = hasNative
    ? nativeBackend.status
    : hasPipe
      ? pipeBackend.status
      : () => ({ backend: null, version: null, connected: false, error: 'no backend available' })

  return {
    backend, version, window, tray, supervisor, terminal, actions, status,
    hasNative, hasPipe, nativeBackend, pipeBackend,
    nativeLoadError: hasNative ? null : nativeBackend?.loadError ?? null,
  }
}

const PLUGIN_PREFIX = '/dshl-control/plugins/'

export function apply(ctx) {
  nativeBackend = ctx.get('dshlNativeBackend') ?? null
  pipeBackend = ctx.get('dshlPipeBackend') ?? null
  const caps = buildCapabilities(ctx)
  // NOTE: no dispose of caps.pipeBackend.client here — the pipe plugin owns
  // that lifecycle via its own effect; duplicating it here split ownership.

  const deferredExit = (code) => {
    setTimeout(() => process.exit(code), 0)
  }

  // ---- Plugin guard (independent disable list + crash rollback state). ----
  let initialBundles = []
  try {
    const cfg = ctx.root?.config ?? null
    const bundles = Array.isArray(cfg?.profile?.bundles) ? cfg.profile.bundles : []
    initialBundles = bundles.map((b) => (typeof b === 'string' ? b : (b?.name ?? b?.plugin ?? null))).filter(Boolean)
    if (typeof ctx.registry === 'object' && ctx.registry) {
      for (const [n] of Object.entries(ctx.registry)) if (n && typeof n === 'string' && !initialBundles.includes(n)) initialBundles.push(n)
    }
  } catch { /* ignore */ }
  if (!initialBundles.includes('@dshl/control')) initialBundles.push('@dshl/control')

  const guard = new PluginGuard({
    dshHome: process.env.DSH_HOME || process.env.DSHL_CACHE || null,
    currentBundleList: initialBundles,
  })
  const startupReport = guard.beginStartup({ currentBundles: initialBundles })
  if (startupReport.rollback?.enabled) {
    ctx.logger.warn(
      `[dshl-control] crash rollback: recorded ${startupReport.autoDisabledThisRound.length} suspicious plugins` +
        ` (crashes=${startupReport.consecutiveCrashes}; recorded only — the dsh plugin loader does not consume the disable list yet)`,
    )
  } else if (startupReport.consecutiveCrashes > 0) {
    ctx.logger.info(`[dshl-control] recent consecutive crashes=${startupReport.consecutiveCrashes}; threshold=${3}`)
  }

  ctx.provide('dshlPluginGuard', {
    list: (opts) => guard.list(opts),
    isDisabled: (pkg) => guard.isDisabled(pkg),
    disabledPackageNames: () => guard.disabledPackageNames(),
    disable: (pkg, opts) => guard.disable(pkg, opts),
    enable: (pkg) => guard.enable(pkg),
    nextStartupRollbackInfo: () => guard.nextStartupRollbackInfo(),
    markHealthy: (opts) => guard.markHealthy(opts),
    markFailed: (opts) => guard.markFailed(opts),
    paths: () => guard.paths(),
    _startupReport: startupReport,
  })

  ctx.provide('desktopPlugins', {
    async list() {
      const bundles = guard.list()
      return bundles.map((b) => ({
        id: b.id,
        packageName: b.packageName,
        name: b.packageName,
        status: b.status,
        mutable: b.mutable,
        disabledReason: b.disabledReason,
        disabledAt: b.disabledAt,
        pluginType: b.packageName === '@dshl/control' ? 'native' : 'standard',
        canDisable: b.mutable && b.status !== 'disabled',
        canEnable: b.mutable && b.status === 'disabled',
      }))
    },
    async previewDisable(packageName) {
      if (!packageName || typeof packageName !== 'string') {
        return { ok: false, error: 'packageName (string) required' }
      }
      if (packageName === '@dshl/control') {
        return { ok: false, error: 'cannot disable the guard plugin itself' }
      }
      const current = guard.list().find((b) => b.packageName === packageName)
      if (!current) {
        return { ok: true, packageName, alreadyDisabled: false, inCurrentProfile: false, affected: [{ packageName, reason: 'not in current profile; persisted for next load' }], needRestart: true }
      }
      if (current.status === 'disabled') {
        return { ok: true, packageName, alreadyDisabled: true, inCurrentProfile: true, affected: [], needRestart: false }
      }
      return { ok: true, packageName, alreadyDisabled: false, inCurrentProfile: true, affected: [{ packageName, reason: 'direct disable (flat graph; no dependents tracked)' }], needRestart: true }
    },
    async executeDisable(packageName, options) {
      const reason = typeof options === 'string' ? options : (options?.reason ?? 'manual')
      try {
        const r = guard.disable(packageName, { reason })
        return { ok: true, ...r }
      } catch (e) {
        return { ok: false, error: e instanceof Error ? e.message : String(e) }
      }
    },
    async enable(packageName) {
      const r = guard.enable(packageName)
      return { ok: true, ...r }
    },
    async crashRollbackInfo() {
      return guard.nextStartupRollbackInfo()
    },
    async markHealthy(opts) {
      return guard.markHealthy({ bundles: opts?.bundles ?? null })
    },
    async markFailed(opts) {
      return guard.markFailed({ report: opts?.report ?? null })
    },
  })

  ctx.provide('nativeCapabilities', {
    backend: caps.backend,
    version: caps.version,
    window: caps.window,
    tray: caps.tray,
    supervisor: caps.supervisor,
    terminal: caps.terminal,
    actions: caps.actions,
    status: caps.status,
  })
  ctx.provide('desktopActions', {
    openTerminal() {
      if (!caps.actions) return
      void Promise.resolve(caps.actions.openTerminal({ cwd: process.cwd(), path: process.env.PATH }))
    },
    requestRestart() {
      return caps.supervisor.restart().then((r) => {
        if (r?.shouldExitHost) deferredExit(0)
        return r
      })
    },
  })

  const disposers = []

  // ---- State route ----
  disposers.push(
    ctx.webServer.register({ kind: 'exact', path: ROUTES.state, handler: async (_req, res) => {
      if (!requestAllowed(_req)) {
        sendJson(res, 403, { error: 'dshl control authority rejected' })
        return
      }
      const snap = await Promise.resolve(caps.status())
      const connected =
        snap.connected === true ||
        (caps.backend === 'native' && (snap.kernelRunning || snap.launched))
      sendJson(res, 200, {
        connected,
        ...snap,
        guard: {
          startupReport,
          rollback: guard.nextStartupRollbackInfo(),
          disabledCount: guard.disabledPackageNames().length,
        },
      })
    }}),
  )

  // ---- Open terminal ----
  disposers.push(
    ctx.webServer.register({ kind: 'exact', path: ROUTES.openTerminal, handler: async (req, res) => {
      if (!asPost(req, res)) return
      if (!caps.actions?.openTerminal) {
        sendJson(res, 501, { error: 'openTerminal not implemented by current backend' })
        return
      }
      try {
        const ok = await Promise.resolve(caps.actions.openTerminal({ cwd: process.cwd(), path: process.env.PATH }))
        if (!ok) throw new Error('backend returned false')
        sendJson(res, 200, { ok: true })
      } catch (cause) {
        sendJson(res, 502, { error: cause instanceof Error ? cause.message : String(cause) })
      }
    }}),
  )

  // ---- Shutdown ----
  disposers.push(
    ctx.webServer.register({ kind: 'exact', path: ROUTES.shutdown, handler: async (req, res) => {
      if (!asPost(req, res)) return
      sendJson(res, 200, { ok: true })
      try {
        const r = await caps.supervisor.shutdown()
        if (r?.empty || (r?.native && !r?.kernelRunning)) {
          deferredExit(0)
        }
      } catch (cause) {
        ctx.logger.warn(`[dshl-control] shutdown failed: ${cause instanceof Error ? cause.message : cause}`)
        deferredExit(0)
      }
    }}),
  )

  // ---- Restart ----
  disposers.push(
    ctx.webServer.register({ kind: 'exact', path: ROUTES.restart, handler: async (req, res) => {
      if (!asPost(req, res)) return
      sendJson(res, 200, { ok: true })
      try {
        const r = await caps.supervisor.restart()
        if (r?.shouldExitHost) deferredExit(0)
        if (!r?.ok) {
          ctx.logger.warn(`[dshl-control] restart rejected by backend: ${r?.error ?? 'unknown'}`)
        }
      } catch (cause) {
        ctx.logger.warn(`[dshl-control] restart failed: ${cause instanceof Error ? cause.message : cause}`)
      }
    }}),
  )

  // ---- Window / tray controls (native-only: 501 on pipe-only) ----
  const requireNative = (fnName, res, run) => {
    if (caps.backend !== 'native') {
      sendJson(res, 501, { error: `${fnName} requires the dshl-native addon backend` })
      return
    }
    run()
  }
  disposers.push(
    ctx.webServer.register({ kind: 'exact', path: ROUTES.windowShow, handler: async (req, res) => {
      if (!asPost(req, res)) return
      requireNative('window/show', res, () => {
        const shown = caps.window?.show?.()
        if (shown === false) {
          // Boot-window skip: machine code lets the UI localize honestly
          // instead of flashing success for a click that did nothing.
          sendJson(res, 409, { ok: false, code: 'booting', error: 'launcher is still starting' })
          return
        }
        sendJson(res, 200, { ok: true })
      })
    }}),
  )
  disposers.push(
    ctx.webServer.register({ kind: 'exact', path: ROUTES.windowHide, handler: async (req, res) => {
      if (!asPost(req, res)) return
      requireNative('window/hide', res, () => { caps.window?.hide?.(); sendJson(res, 200, { ok: true }) })
    }}),
  )
  disposers.push(
    ctx.webServer.register({ kind: 'exact', path: ROUTES.windowNavigate, handler: async (req, res) => {
      if (!asPost(req, res)) return
      requireNative('window/navigate', res, () => {
        let body = ''
        req.on('data', (chunk) => (body += chunk))
        req.on('end', () => {
          try {
            const { url } = body ? JSON.parse(body) : {}
            if (typeof url !== 'string' || url === '') { sendJson(res, 400, { error: 'url required' }); return }
            caps.window?.navigate?.(url)
            sendJson(res, 200, { ok: true })
          } catch (cause) {
            sendJson(res, 400, { error: cause instanceof Error ? cause.message : String(cause) })
          }
        })
      })
    }}),
  )
  disposers.push(
    ctx.webServer.register({ kind: 'exact', path: ROUTES.trayShow, handler: async (req, res) => {
      if (!asPost(req, res)) return
      requireNative('tray/show', res, () => { caps.tray?.show?.(); sendJson(res, 200, { ok: true }) })
    }}),
  )
  disposers.push(
    ctx.webServer.register({ kind: 'exact', path: ROUTES.trayHide, handler: async (req, res) => {
      if (!asPost(req, res)) return
      requireNative('tray/hide', res, () => { caps.tray?.hide?.(); sendJson(res, 200, { ok: true }) })
    }}),
  )
  disposers.push(
    ctx.webServer.register({ kind: 'exact', path: ROUTES.traySetIcon, handler: async (req, res) => {
      if (!asPost(req, res)) return
      requireNative('tray/setIcon', res, () => {
        let body = ''
        req.on('data', (chunk) => (body += chunk))
        req.on('end', () => {
          try {
            const { dark } = body ? JSON.parse(body) : {}
            caps.tray?.setIcon?.(!!dark)
            sendJson(res, 200, { ok: true })
          } catch (cause) {
            sendJson(res, 400, { error: cause instanceof Error ? cause.message : String(cause) })
          }
        })
      })
    }}),
  )
  disposers.push(
    ctx.webServer.register({ kind: 'exact', path: ROUTES.launch, handler: async (req, res) => {
      if (!asPost(req, res)) return
      requireNative('launch', res, () => {
        let body = ''
        req.on('data', (chunk) => (body += chunk))
        req.on('end', async () => {
          try {
            // 显式优于隐式: only documented LaunchOptions keys pass through —
            // raw passthrough would silently accept snake_case mistakes that
            // napi drops anyway.
            const raw = body ? JSON.parse(body) : {}
            const r = await caps.supervisor.launch(resolveLaunchOptions(raw))
            sendJson(res, 200, r)
          } catch (cause) {
            sendJson(res, 400, { error: cause instanceof Error ? cause.message : String(cause) })
          }
        })
      })
    }}),
  )

  // ---- Plugin-guard HTTP routes ----
  disposers.push(
    ctx.webServer.register({ kind: 'prefix', path: PLUGIN_PREFIX, handler: async (req, res) => {
      // Authority FIRST: nothing derived from the unauthenticated request
      // (not even URL decoding) runs before the loopback check.
      if (!requestAllowed(req)) { sendJson(res, 403, { error: 'dshl guard authority rejected' }); return }
      const raw = req.url || ''
      const rest = raw.startsWith(PLUGIN_PREFIX) ? raw.slice(PLUGIN_PREFIX.length) : raw
      let seg1 = '', seg2 = ''
      try {
        ;[seg1 = '', seg2 = ''] = rest.split('?')[0].split('/').map(decodeURIComponent)
      } catch {
        sendJson(res, 400, { error: 'malformed URL encoding' })
        return
      }
      if (req.method === 'GET') {
        if (seg1 === '' || seg1 === 'list') {
          let bundles = null
          try { bundles = guard.list() } catch { bundles = [] }
          sendJson(res, 200, { bundles })
          return
        }
        if (seg1 === 'disabled') {
          const names = guard.disabledPackageNames()
          sendJson(res, 200, { disabled: names, count: names.length })
          return
        }
        if (seg1 === 'rollback') {
          sendJson(res, 200, guard.nextStartupRollbackInfo())
          return
        }
        sendJson(res, 404, { error: 'not found' })
        return
      }
      if (req.method !== 'POST') { sendJson(res, 405, { error: 'requires GET or local POST' }); return }
      if (seg1 === 'mark-healthy') {
        try {
          const body = await parseJsonBody(req)
          const r = guard.markHealthy({ bundles: Array.isArray(body?.bundles) ? body.bundles : null })
          sendJson(res, 200, r)
        } catch (e) { sendJson(res, 400, { error: e instanceof Error ? e.message : String(e) }) }
        return
      }
      if (seg1 === 'mark-failed') {
        try {
          const body = await parseJsonBody(req)
          const r = guard.markFailed({ report: typeof body?.report === 'string' ? body.report : null })
          sendJson(res, 200, r)
        } catch (e) { sendJson(res, 400, { error: e instanceof Error ? e.message : String(e) }) }
        return
      }
      const name = seg1
      if (!name) { sendJson(res, 400, { error: 'plugin name required (seg1)' }); return }
      if (seg2 === 'disable') {
        try {
          const body = await parseJsonBody(req)
          const r = guard.disable(name, { reason: typeof body?.reason === 'string' ? body.reason : 'manual' })
          sendJson(res, 200, r)
        } catch (e) { sendJson(res, 400, { error: e instanceof Error ? e.message : String(e) }) }
        return
      }
      if (seg2 === 'enable') {
        try {
          const r = guard.enable(name)
          sendJson(res, 200, r)
        } catch (e) { sendJson(res, 400, { error: e instanceof Error ? e.message : String(e) }) }
        return
      }
      sendJson(res, 404, { error: `unknown guard action: /${seg1}/${seg2}` })
    }}),
  )

  // ---- UI script ----
  disposers.push(
    ctx.webServer.register({ kind: 'exact', path: ROUTES.ui, handler: async (_req, res) => {
      res.statusCode = 200
      res.setHeader('content-type', 'text/javascript; charset=utf-8')
      res.setHeader('cache-control', 'no-store')
      res.setHeader('x-content-type-options', 'nosniff')
      res.end(uiScript())
    }}),
  )

  // ---- Vendored xterm.js assets (offline embedded terminal) ----
  // Fixed whitelist — NOT a generic static dir server (no path handling, so
  // no traversal surface). Files live in assets/xterm (shipped in the npm
  // tarball, see package.json `files`), so the terminal works with no CDN
  // access at all.
  const XTERM_ASSETS = {
    '/dshl-control/assets/xterm/xterm.mjs': { file: '../assets/xterm/xterm.mjs', type: 'text/javascript; charset=utf-8' },
    '/dshl-control/assets/xterm/addon-fit.mjs': { file: '../assets/xterm/addon-fit.mjs', type: 'text/javascript; charset=utf-8' },
    '/dshl-control/assets/xterm/xterm.css': { file: '../assets/xterm/xterm.css', type: 'text/css; charset=utf-8' },
  }
  const assetCache = new Map()
  for (const [route, asset] of Object.entries(XTERM_ASSETS)) {
    disposers.push(
      ctx.webServer.register({ kind: 'exact', path: route, handler: async (_req, res) => {
        let body = assetCache.get(route)
        if (body === undefined) {
          body = readFileSync(new URL(asset.file, import.meta.url))
          assetCache.set(route, body)
        }
        res.statusCode = 200
        res.setHeader('content-type', asset.type)
        res.setHeader('cache-control', 'no-store')
        res.setHeader('x-content-type-options', 'nosniff')
        res.end(body)
      }}),
    )
  }

  // ---- Terminal routes (native-only) ----
  if (caps.terminal) {
    const requireTerminal = (name, res, run) => {
      if (!caps.terminal) { sendJson(res, 501, { error: `${name} requires native dshl PTY backend` }); return false }
      run(); return true
    }
    disposers.push(
      ctx.webServer.register({ kind: 'exact', path: ROUTES.termSpawn, handler: async (req, res) => {
        if (!asPost(req, res)) return
        if (!requireTerminal('terminal/spawn', res, () => {})) return
        try {
          const body = await parseJsonBody(req)
          const r = await caps.terminal.spawn({
            shell: typeof body?.shell === 'string' ? body.shell : undefined,
            cwd: typeof body?.cwd === 'string' && body.cwd.length ? body.cwd : process.cwd(),
            env: body?.env && typeof body.env === 'object' && !Array.isArray(body.env) ? Object.fromEntries(Object.entries(body.env).filter(([,v]) => typeof v === 'string')) : undefined,
            prependPath: Array.isArray(body?.prependPath) ? body.prependPath.filter((s) => typeof s === 'string') : undefined,
            cols: Number.isFinite(Number(body?.cols)) ? Number(body.cols) : undefined,
            rows: Number.isFinite(Number(body?.rows)) ? Number(body.rows) : undefined,
          })
          sendJson(res, 200, r)
        } catch (e) { sendJson(res, 502, { error: e instanceof Error ? e.message : String(e) }) }
      }}),
    )
    disposers.push(
      ctx.webServer.register({ kind: 'exact', path: ROUTES.termList, handler: async (req, res) => {
        if (!requestAllowed(req)) { sendJson(res, 403, { error: 'authority rejected' }); return }
        if (!requireTerminal('terminal/list', res, () => {})) return
        try { sendJson(res, 200, { sessions: await caps.terminal.list() }) }
        catch (e) { sendJson(res, 502, { error: e instanceof Error ? e.message : String(e) }) }
      }}),
    )
    disposers.push(
      ctx.webServer.register({ kind: 'exact', path: ROUTES.termKill, handler: async (req, res) => {
        if (!asPost(req, res)) return
        if (!requireTerminal('terminal/kill', res, () => {})) return
        try {
          const body = await parseJsonBody(req)
          const id = String(body?.id ?? '')
          if (!id) return sendJson(res, 400, { error: 'id required' })
          const ok = await caps.terminal.kill(id)
          sendJson(res, ok ? 200 : 404, { ok })
        } catch (e) { sendJson(res, 502, { error: e instanceof Error ? e.message : String(e) }) }
      }}),
    )
    disposers.push(
      ctx.webServer.register({ kind: 'exact', path: ROUTES.termResize, handler: async (req, res) => {
        if (!asPost(req, res)) return
        if (!requireTerminal('terminal/resize', res, () => {})) return
        try {
          const body = await parseJsonBody(req)
          const id = String(body?.id ?? '')
          const cols = Number(body?.cols) | 0
          const rows = Number(body?.rows) | 0
          if (!id || cols < 1 || rows < 1) return sendJson(res, 400, { error: 'id + cols + rows required' })
          const ok = await caps.terminal.resize(id, cols, rows)
          sendJson(res, ok ? 200 : 404, { ok })
        } catch (e) { sendJson(res, 502, { error: e instanceof Error ? e.message : String(e) }) }
      }}),
    )
    disposers.push(
      ctx.webServer.register({ kind: 'exact', path: ROUTES.termWrite, handler: async (req, res) => {
        if (!asPost(req, res)) return
        if (!requireTerminal('terminal/write', res, () => {})) return
        try {
          const body = await parseJsonBody(req)
          const id = String(body?.id ?? '')
          const data = typeof body?.data === 'string' ? body.data : ''
          if (!id) return sendJson(res, 400, { error: 'id required' })
          const ok = await caps.terminal.write(id, data)
          sendJson(res, ok ? 200 : 404, { ok })
        } catch (e) { sendJson(res, 502, { error: e instanceof Error ? e.message : String(e) }) }
      }}),
    )
    disposers.push(
      ctx.webServer.register({ kind: 'exact', path: ROUTES.termEndpoint, handler: async (req, res) => {
        if (!requestAllowed(req)) { sendJson(res, 403, { error: 'authority rejected' }); return }
        if (!requireTerminal('terminal/endpoint', res, () => {})) return
        try {
          const r = await caps.terminal.endpoint()
          if (!r) { sendJson(res, 503, { error: 'pty ws server not running (spawn a session first)' }); return }
          sendJson(res, 200, r)
        } catch (e) { sendJson(res, 502, { error: e instanceof Error ? e.message : String(e) }) }
      }}),
    )
  }

  ctx.effect(() => () => {
    // Normal plugin teardown = graceful exit; tells the guard's next
    // beginStartup not to count this run as a crash even when the renderer
    // never reached markHealthy.
    try { guard.markShutdown() } catch { /* best-effort */ }
    for (const dispose of disposers) typeof dispose === 'function' && dispose()
  })

  ctx.webServer.tapIndex((html) =>
    html.replace('</body>', `<script src="${ROUTES.ui}" defer></script></body>`),
  )

  // The pipe URL embeds the control bearer token (`dshl://<token>@…`);
  // never log it verbatim — logs land on disk.
  const pipeUrl = process.env.DSHL_CONTROL_URL
  const shownPipe = caps.hasPipe && pipeUrl ? pipeUrl.replace(/\/\/[^@]*@/, '//***@') : ''
  ctx.logger.info(
    `[dshl-control] bridge active — backend=${caps.backend ?? 'none'} version=${caps.version ?? 'n/a'}` +
      (caps.hasNative ? ' (dshl-native addon loaded)' : '') +
      (shownPipe ? ` (legacy pipe ${shownPipe})` : ''),
  )
  if (!caps.hasNative && caps.nativeLoadError) {
    // The @dshl/native package is installed but its .node failed to load
    // (truncated download, wrong Node ABI, missing VC runtime). Without this
    // line the only symptom is a bare `backend=none`.
    ctx.logger.warn(
      `[dshl-control] dshl-native addon load error: ${caps.nativeLoadError instanceof Error ? caps.nativeLoadError.message : caps.nativeLoadError}`,
    )
  }
}

let cachedUiScript
function uiScript() {
  if (cachedUiScript === undefined) {
    cachedUiScript = readFileSync(new URL('./ui.js', import.meta.url), 'utf8')
  }
  return cachedUiScript
}
