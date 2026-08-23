// Service Definition for the desktop-backend seam (harness 三角色模式:
// Service Definition / Provider / Consumer).
//
//   Providers: @dshl/native — local kernel via napi DLL (FULL tier)
//              @dshl/pipe   — remote control pipe  (REMOTE tier)
//   Consumer:  @dshl/control — folds whichever providers are present into
//              the `nativeCapabilities` service.
//
// The contract lives beside the CONSUMER because both providers are OPTIONAL
// (native needs an installed .node; pipe needs DSHL_CONTROL_URL) — neither
// provider can own this file without dragging the other into a dependency,
// and the docs' "不要预防性拆分" rule argues against a fourth package until
// drift actually hurts. If the two tiers keep diverging, promote this file to
// its own @dshl/backend-definition package.

/**
 * Capability groups each tier MUST expose (absent GROUPS are legal
 * feature-detection; present groups must carry every listed method).
 */
export const TIERS = {
  native: {
    actions: ['openTerminal', 'openPath', 'openUrl', 'ping', 'platformInfo'],
    window: ['show', 'hide'],
    tray: ['show', 'hide', 'setIcon'],
    supervisor: ['shutdown', 'restart', 'launch'],
    terminal: ['spawn', 'list', 'kill', 'resize', 'write', 'endpoint'],
    status: [],
  },
  // Remote tier intentionally ships a subset: no window/tray/terminal (a
  // remote launcher cannot drive a WebView in another process), openPath/
  // openUrl degrade to always-false, ping/platformInfo answer locally.
  pipe: {
    actions: ['openTerminal', 'openPath', 'openUrl', 'ping', 'platformInfo'],
    supervisor: ['shutdown', 'restart', 'launch'],
    status: [],
  },
}

/** Documented keys of `LaunchOptions` (napi `launch()`), camelCase. */
export const LAUNCH_OPTION_KEYS = [
  'config',
  'debug',
  'enableSingleInstance',
  'enableControlPipe',
  'installSignalHandler',
]

/**
 * Validate a backend against its tier. Returns a list of drift problems;
 * an empty list means conformant. Warn-only by design: a missing method
 * degrades one route to 501, it should not take the whole bridge down.
 */
export function checkBackend(tier, backend) {
  const spec = TIERS[tier]
  if (!spec) return [`unknown backend tier "${tier}"`]
  if (!backend || typeof backend !== 'object') return [`${tier}: backend is not an object`]
  const problems = []
  for (const [group, methods] of Object.entries(spec)) {
    const obj = backend[group]
    if (obj === null || obj === undefined) continue // absent group = feature-detect
    for (const m of methods) {
      if (typeof obj[m] !== 'function') problems.push(`${tier}.${group}.${m} missing or not a function`)
    }
  }
  return problems
}

/**
 * Explicit resolve step for `launch()` options (显式优于隐式): pick only the
 * documented keys so snake_case mistakes or unknown fields cannot silently
 * fall through napi's key-dropping deserialization.
 */
export function resolveLaunchOptions(raw) {
  const out = {}
  if (!raw || typeof raw !== 'object') return out
  for (const k of LAUNCH_OPTION_KEYS) {
    if (raw[k] !== undefined) out[k] = raw[k]
  }
  return out
}
