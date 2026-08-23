// @dshl/pipe — the dshl pipe backend dsh plugin.
//
// Connects to a running dshl launcher over the control pipe
// (DSHL_CONTROL_URL env var) and exposes a subset of the native capability
// set (supervisor + actions + status) as the `dshlPipeBackend` Cordis service.
//
// This backend is the legacy/remote fallback: it's used when a launcher exe
// was started above the dsh process and set DSHL_CONTROL_URL before this
// plugin loaded. It does NOT expose window/tray/terminal controls (those
// are native-only — the pipe dispatches open-terminal/restart/shutdown
// remotely but cannot drive a WebView in another process).

export const name = 'dshl-pipe'

import { ControlClient } from './client.js'

const endpoint = process.env.DSHL_CONTROL_URL
let client = null
if (endpoint) {
  try {
    client = new ControlClient(endpoint, null)
  } catch {
    client = null
  }
}

const hasPipe = client !== null

function buildActions() {
  if (!hasPipe) return null
  return {
    openTerminal: async () => {
      await client.request('open-terminal', {})
      return true
    },
    // Remote pipe doesn't expose open-path/open-url. Route them to
    // always-false for UX-contract parity; UI falls back gracefully.
    openPath: () => false,
    openUrl: () => false,
    ping: async () => client.request('ping', {}),
    platformInfo: () => ({ os: process.platform, arch: process.arch, shell: 'bash' }),
  }
}

function buildSupervisor() {
  if (!hasPipe) return null
  return {
    shutdown: async () => {
      await client.request('shutdown', {})
      return { ok: true, native: false }
    },
    restart: async () => {
      await client.request('restart', {})
      return { ok: true, native: false }
    },
    launch: async () => ({ ok: false, error: 'launch not supported over pipe' }),
  }
}

function buildStatus() {
  if (!hasPipe) return null
  return async () => {
    let pipeOk = false
    let pipeVersion = null
    let pipeError = null
    try {
      const r = await client.request('ping', {})
      pipeOk = true
      pipeVersion = r?.version ?? null
    } catch (cause) {
      pipeError = cause instanceof Error ? cause.message : String(cause)
    }
    return {
      backend: 'pipe',
      version: pipeVersion,
      connected: pipeOk,
      pipeError,
      // pipe backend is only reachable when the launcher actually spawned dsh
      // — so window/tray are assumed present on the exe side but unknown from
      // here (501 semantics on direct controls).
      launched: pipeOk,
      kernelRunning: pipeOk,
      windowVisible: null,
      trayVisible: null,
    }
  }
}

/**
 * The pipe backend object, or null when DSHL_CONTROL_URL is unset / invalid.
 * Shape: { backend, version, supervisor, actions, status, client }
 */
export const backend = hasPipe
  ? {
      backend: 'pipe',
      version: null,
      supervisor: buildSupervisor(),
      actions: buildActions(),
      status: buildStatus(),
      client,
    }
  : null

export const inject = []

export function apply(ctx) {
  if (backend) {
    ctx.provide('dshlPipeBackend', backend)
    ctx.effect(() => () => client.dispose())
  }
  // The URL embeds the control bearer token (`dshl://<token>@host:port`);
  // never log it verbatim — logs land on disk.
  const shown = hasPipe ? endpoint.replace(/\/\/[^@]*@/, '//***@') : ''
  ctx.logger.info(`[dshl-pipe] ${hasPipe ? `connected to ${shown}` : 'no DSHL_CONTROL_URL; inactive'}`)
}
