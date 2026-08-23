# @dshl/control

> The dshl control bridge — the top-level abstraction plugin that folds the
> native (dshl-native addon) and pipe (legacy control pipe) backends into a
> unified `nativeCapabilities` Cordis service, exposes HTTP routes, and hosts
> the `desktopPlugins` market contract (plugin-guard).

## Overview

`@dshl/control` is a [dsh](https://github.com/hibays/dsh-launcher) plugin. It
is **the only plugin the dsh profile needs to declare** to pick up dshl
launcher capabilities. The actual backends are provided by two sibling
plugins, both declared as `optionalDependencies`:

- [`@dshl/native`](../dshl-native) — loads the dshl-native napi-rs addon
  (.node DLL) which links the FULL dshl kernel (webui window, tray icon,
  setup pipeline, supervisor loop, OS actions, embedded PTY terminal), and
  registers it as the `dshlNativeBackend` service (always registered; the
  descriptor carries `backend: null` plus a `loadError` when the addon fails
  to load).
- [`@dshl/pipe`](../dshl-pipe) — connects to an already-running dshl launcher
  over the legacy control pipe (`DSHL_CONTROL_URL`) and registers the
  `dshlPipeBackend` service (only registered when the URL is set and valid).

When a sibling plugin is simply not installed, its service never exists and
the bridge falls back gracefully: native takes priority, pipe is the remote
fallback, and neither → capability fields are null and routes return 501.

## Architecture

The plugin follows the harness three-role pattern:

| Role | Package | What it does |
| --- | --- | --- |
| Service Definition | `src/backend-contract.js` (this package) | Declares the `TIERS` capability contract each backend tier MUST satisfy |
| Provider | [`@dshl/native`](../dshl-native) | FULL tier: window / tray / supervisor / terminal / actions / status |
| Provider | [`@dshl/pipe`](../dshl-pipe) | REMOTE tier: supervisor / actions / status only |
| Consumer | this plugin (`@dshl/control`) | Folds whichever providers are present into the unified `nativeCapabilities` service |

### The `ctx.get` optional-consumption idiom

At `apply(ctx)` time the consumer resolves both providers opportunistically:

```js
nativeBackend = ctx.get('dshlNativeBackend') ?? null
pipeBackend   = ctx.get('dshlPipeBackend') ?? null
```

A provider that is not loaded simply resolves to `undefined`. Never
`require()` a sibling here — bypassing the container splits lifecycle
ownership and freezes the binding at module-eval time.

### The TIERS contract and drift warnings

`src/backend-contract.js` exports `TIERS`, the capability groups each tier
MUST expose. An absent group is legal feature-detection; a **present** group
must carry every listed method:

- `native`: `actions` (openTerminal, openPath, openUrl, ping, platformInfo),
  `window` (show, hide), `tray` (show, hide, setIcon),
  `supervisor` (shutdown, restart, launch), `terminal` (spawn, list, kill,
  resize, write, endpoint), `status`.
- `pipe`: intentionally a subset — `actions` and `supervisor` only. A remote
  pipe cannot drive a WebView in another process, so window / tray / terminal
  are excluded; `openPath`/`openUrl` degrade to always-false and
  `ping`/`platformInfo` answer locally.

`checkBackend(tier, backend)` validates a backend against its tier and returns
a list of drift problems. Drift is **warn-only by design**: `apply()` logs
`backend contract drift (<tier>): …` via `ctx.logger.warn` and keeps going. A
missing method degrades one route to 501; it should not take the whole bridge
down. If the two tiers keep diverging, the contract file should be promoted to
its own `@dshl/backend-definition` package.

The same module also exports `LAUNCH_OPTION_KEYS` and
`resolveLaunchOptions(raw)`: an explicit resolve step that passes only the
documented napi `LaunchOptions` keys (`config`, `debug`,
`enableSingleInstance`, `enableControlPipe`, `installSignalHandler`), so
snake_case mistakes or unknown fields cannot silently fall through napi's
key-dropping deserialization.

## Responsibilities retained by this top-level plugin

- Folding the two backends into the unified `nativeCapabilities`,
  `desktopActions`, `dshlPluginGuard`, and `desktopPlugins` Cordis services.
- HTTP routes under `/dshl-control/*` (state, window, tray, supervisor,
  terminal, plugin-guard, ui-script injection, vendored xterm assets).
- The floating UI action bar injection (`/dshl-control/ui.js`, appended to the
  index page via `webServer.tapIndex`).
- The `desktopPlugins` + `dshlPluginGuard` market contract (crash rollback,
  disable preview, mark-healthy/failed).

## HTTP API

All routes are served on the dsh web server under `/dshl-control`. Two shared
gates apply unless noted otherwise:

- **Loopback authority** (`requestAllowed`): the TCP peer must be
  `127.0.0.1`/`::1` AND the `Host` header must resolve to
  `127.0.0.1`/`localhost`/`::1`; anything else gets **403**.
- **POST gate** (`asPost`): non-POST gets **405**, then the loopback check
  runs (**403** on failure). JSON bodies are capped at 64 KiB.

Native-only routes return **501** when `caps.backend !== 'native'`
(`requireNative`). Backend call failures surface as **502**.

| Route | Method | Gate | Typical statuses |
| --- | --- | --- | --- |
| `/dshl-control/state` | any | loopback | 200 (status snapshot + `guard` block); 403 |
| `/dshl-control/open-terminal` | POST | POST gate | 200; 501; 502 |
| `/dshl-control/shutdown` | POST | POST gate | 200 immediately, host exit deferred; 403/405 |
| `/dshl-control/restart` | POST | POST gate | 200 immediately; rejection logged; 403/405 |
| `/dshl-control/window/show` | POST | POST gate + native | 200; **409 `{ok:false, code:'booting'}`** when the kernel reports the window was not shown because the launcher is still starting; 501 |
| `/dshl-control/window/hide` | POST | POST gate + native | 200; 501 |
| `/dshl-control/window/navigate` | POST | POST gate + native | 200; 400 (`url` missing or invalid); 501 |
| `/dshl-control/tray/show` · `/hide` | POST | POST gate + native | 200; 501 |
| `/dshl-control/tray/icon` | POST | POST gate + native | 200 (`dark` boolean); 400 (bad JSON); 501 |
| `/dshl-control/launch` | POST | POST gate + native | 200 (napi launch result); 400 (bad JSON / launch threw); 501 |
| `/dshl-control/ui.js` | GET | none | 200 (action-bar script) |
| `/dshl-control/assets/xterm/xterm.mjs` · `addon-fit.mjs` · `xterm.css` | GET | none | 200 (vendored, in-memory cached; fixed whitelist, no path traversal surface) |
| `/dshl-control/plugins/*` (guard) | see below | prefix handler, below | see below |

Notes on specific routes:

- **`window/show` during startup**: the machine-readable `409 code:'booting'`
  lets the UI localize honestly instead of flashing success for a click that
  did nothing.
- **`launch` whitelist**: the raw body goes through
  `resolveLaunchOptions()` — only the five documented `LaunchOptions` keys are
  forwarded to the backend; everything else is dropped before it can reach
  napi.
- **Terminal routes** (`/terminal/spawn`, `/list`, `/kill`, `/resize`,
  `/write`, `/endpoint`) are registered **only when the native terminal
  capability exists**; they additionally re-check the gate and return 501 as a
  defensive fallback. `kill`/`resize`/`write` validate their arguments
  (**400**) and map "unknown session" to **404**; `list` and `endpoint` use
  the loopback gate only, and `endpoint` returns **503** until a PTY WebSocket
  session has been spawned.

### Plugin-guard routes (`/dshl-control/plugins/…`, prefix match)

One prefix handler serves the whole subtree. Ordering inside the handler:

1. Loopback authority FIRST — nothing derived from the unauthenticated
   request (not even URL decoding) runs before the check; failure → **403**.
2. Path segments are `decodeURIComponent`-decoded; malformed percent-encoding
   → **400** `malformed URL encoding`.
3. Non-GET/POST → **405**.

| Sub-path | Method | Result |
| --- | --- | --- |
| `` (empty) or `list` | GET | 200 `{bundles}` (id, packageName, status `active`/`disabled`/`protected`, mutable, disabledReason, disabledAt); unknown → 404 |
| `disabled` | GET | 200 `{disabled: string[], count}` |
| `rollback` | GET | 200 crash/rollback state (`nextStartupRollbackInfo()`) |
| `mark-healthy` | POST | 200 (body `{bundles?: string[]}`); bad JSON → 400 |
| `mark-failed` | POST | 200 (body `{report?: string}`, report truncated to 4000 chars); bad JSON → 400 |
| `:name/disable` | POST | 200 (body `{reason?: string}`, default `manual`); bad JSON → 400 |
| `:name/enable` | POST | 200 (`hadEntry` reports whether an entry existed); bad JSON → 400 |
| anything else | — | 404 |

`@dshl/control` itself is protected: disabling it is refused (status
`protected`, `mutable: false`).

## Plugin guard

`src/plugin-guard.js` implements an independent disable list + crash rollback
tracker. It persists two files under `$DSH_HOME/.dshl/` (or `~/.dsh/.dshl/`
as fallback):

- `disabled.json` — disabled plugins map (`pkg -> {reason, disabledAt}`)
- `launch-state.json` — crash tracking: healthy flag, last healthy bundles
  snapshot, consecutive-crash counter, rollback analysis

Constants: `WINDOW_MS = 30s` (renderer health-report window),
`GRACE_MS = 10s` (recent start without healthy ≠ crash),
`AUTO_DISABLE_THRESHOLD = 3`.

### Lifecycle

- **`beginStartup`** runs inside `apply()`. It stamps `startedAt`,
  `healthy=false`, then analyses the *previous* run. A previous run counts as
  a crash only when it was neither marked healthy nor ended gracefully AND its
  `startedAt` is older than `GRACE_MS`. Bundles present now but missing from
  `lastHealthyBundles` are flagged *suspicious*.
- **Auto-disable**: when `consecutiveCrashes >= 3` AND there is at least one
  suspicious bundle, the suspicious bundles are written into `disabled.json`
  with reason `crash-<N>x` and the launch is flagged as a rollback launch
  (logged as a warning at startup).
- **Automatic `markHealthy`**: `ui.js` calls `POST plugins/mark-healthy` once,
  **60 seconds after the widget loads**, fetching the live bundle list first
  so the snapshot is accurate. (It used to be 3 s; the 60 s stability bar
  matters — a process that dies at t=10 s must still count as a failed start,
  and marking healthy too early would wipe that crash signal.) `markHealthy`
  commits `healthy=true`, stores the bundle snapshot, and resets the crash
  counter. Failure of the auto-call is non-fatal; the manual button still
  works.
- **`markShutdown` (graceful exit)**: the plugin's `ctx.effect` dispose hook
  calls `guard.markShutdown()`, setting `gracefulExit=true`. The next
  `beginStartup` therefore does NOT count the run as a crash — even when the
  renderer never reached `markHealthy` (e.g. the user closed the window early).
  Normal teardown is never a crash.
- **`markFailed`** records an explicit renderer failure (sets `healthy=false`)
  but deliberately does NOT auto-disable; detection is deferred to the next
  `beginStartup` to avoid false positives. The `WINDOW_MS` deadline timer is
  likewise a soft no-op: a silent renderer alone never disables anything.

### Enforcement caveat

Nothing in the dsh plugin loader reads `disabled.json` today — the bundle
patch mechanism can only INSERT a plugin, not filter others out. The disable
list is persisted and exposed (services, HTTP routes, overlay UI); it is
bookkeeping + visibility, **not** a load-time block. Claims are phrased as
"recorded", never "won't load on next boot", until a consumer in the loader
actually exists.

## Install

Install as a dsh bundle. The package declares
`dsh.bundle.patch: ./cordis.patch.yml`, so installing it into a profile
inserts the `@dshl/control` row automatically; equivalently, apply the patch
by hand:

```bash
dsh --profile <name> --patch ./node_modules/@dshl/control/cordis.patch.yml
```

Runtime requirements and packaging facts (from `package.json`):

- `engines.node >= 22` (the consumer uses container-based provider discovery;
  Node 22+ is the supported baseline).
- `peerDependencies`: `@deepseek-ai/cordis >= 4.0.0` — the plugin is a Cordis
  worker and expects the host to supply it.
- `optionalDependencies`: `@dshl/native ^0.1.0` and `@dshl/pipe ^0.1.0`.
  npm/pnpm installs whichever siblings are available; both are optional and
  either may be absent (see Overview for the resulting degradation).
- For a plugin-only deployment (no standalone launcher exe) you typically want
  `@dshl/native` (and its platform-specific `.node` subpackage). For a
  launcher-spawned deployment you want `@dshl/pipe` (with `DSHL_CONTROL_URL`
  exported by the launcher; note the URL embeds a bearer token and is never
  logged verbatim).
- Published files (`files` field): `src`, `assets` (vendored xterm.js so the
  embedded terminal works offline), `cordis.patch.yml`, `LICENSE`,
  `README.md`.

## License

MIT © hibays
