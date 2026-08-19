// @dshl/control — the dshl control bridge plugin.
//
// Runs inside dsh (as a Cordis plugin row). It reads the launcher's control
// endpoint from DSHL_CONTROL_URL, connects over the local NDJSON pipe, and
// exposes the launcher's native actions to the dsh web UI:
//
//   * `desktopActions` Cordis service (the desktop-app contract, so a future
//     community-market integration can reuse it unchanged);
//   * HTTP routes the injected web widget (and any same-origin page code)
//     calls to trigger open-terminal / restart / shutdown;
//   * an index tap that injects the floating action bar into the web UI.
//
// Nothing here ships native code: it is the pure-JS (Backend A) half of the
// dual-track architecture.

import { readFileSync } from 'node:fs'
import { ControlClient } from './client.js'

export const name = 'dshl-control'
export const inject = ['webServer']
const ROUTES = {
  state: '/dshl-control/state',
  openTerminal: '/dshl-control/open-terminal',
  restart: '/dshl-control/restart',
  shutdown: '/dshl-control/shutdown',
  ui: '/dshl-control/ui.js',
}

function sendJson(res, status, value) {
  const body = JSON.stringify(value)
  res.statusCode = status
  res.setHeader('content-type', 'application/json; charset=utf-8')
  res.setHeader('cache-control', 'no-store')
  res.setHeader('x-content-type-options', 'nosniff')
  res.end(body)
}

// The control actions are local-only: accept loopback requests whose Host
// header names the local webserver (the dsh page itself is same-origin).
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

export function apply(ctx) {
  const endpoint = process.env.DSHL_CONTROL_URL
  if (!endpoint) {
    ctx.logger.warn('[dshl-control] DSHL_CONTROL_URL is not set; bridge disabled')
    return
  }
  let client
  try {
    client = new ControlClient(endpoint, ctx.logger)
  } catch (cause) {
    ctx.logger.warn(`[dshl-control] invalid DSHL_CONTROL_URL: ${cause instanceof Error ? cause.message : cause}`)
    return
  }
  ctx.effect(() => () => client.dispose())

  // Desktop-contract service: a future community-market bundle picks these up
  // via `ctx.inject(['desktopActions'], ...)` exactly like the desktop app.
  ctx.provide('desktopActions', {
    openTerminal() {
      void client.request('open-terminal', {})
    },
    requestRestart() {
      return client.request('restart', {})
    },
  })

  const disposers = [
    ctx.webServer.register({ kind: 'exact', path: ROUTES.state, handler: async (_req, res) => {
      if (!requestAllowed(_req)) {
        sendJson(res, 403, { error: 'dshl control authority rejected' })
        return
      }
      try {
        const result = await client.request('ping', {})
        sendJson(res, 200, { connected: true, version: result?.version })
      } catch (cause) {
        sendJson(res, 200, {
          connected: false,
          error: cause instanceof Error ? cause.message : String(cause),
        })
      }
    }}),
    ctx.webServer.register({ kind: 'exact', path: ROUTES.openTerminal, handler: async (req, res) => {
      if (!asPost(req, res)) return
      try {
        await client.request('open-terminal', {})
        sendJson(res, 200, { ok: true })
      } catch (cause) {
        sendJson(res, 502, { error: cause instanceof Error ? cause.message : String(cause) })
      }
    }}),
    ctx.webServer.register({ kind: 'exact', path: ROUTES.restart, handler: async (req, res) => {
      if (!asPost(req, res)) return
      // Acknowledge before asking the launcher to stop dsh: this HTTP server
      // dies with dsh, so awaiting the launcher reply would cut the response
      // off mid-flight. The pipe request is still delivered; the UI treats a
      // dropped connection as success.
      sendJson(res, 200, { ok: true })
      void client.request('restart', {})
    }}),
    ctx.webServer.register({ kind: 'exact', path: ROUTES.shutdown, handler: async (req, res) => {
      if (!asPost(req, res)) return
      sendJson(res, 200, { ok: true })
      void client.request('shutdown', {})
    }}),
    ctx.webServer.register({ kind: 'exact', path: ROUTES.ui, handler: async (_req, res) => {
      res.statusCode = 200
      res.setHeader('content-type', 'text/javascript; charset=utf-8')
      res.setHeader('cache-control', 'no-store')
      res.setHeader('x-content-type-options', 'nosniff')
      res.end(uiScript())
    }}),
  ]
  ctx.effect(() => () => {
    for (const dispose of disposers) dispose()
  })

  // Inject the floating action bar into the web UI. `tapIndex` runs against
  // the SPA's index.html served by the frontend-static fallback owner.
  ctx.webServer.tapIndex((html) =>
    html.replace('</body>', `<script src="${ROUTES.ui}" defer></script></body>`),
  )

  ctx.logger.info(`[dshl-control] bridge active at ${endpoint}`)
}

let cachedUiScript
function uiScript() {
  if (cachedUiScript === undefined) {
    cachedUiScript = readFileSync(new URL('./ui.js', import.meta.url), 'utf8')
  }
  return cachedUiScript
}