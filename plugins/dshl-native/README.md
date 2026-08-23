# @dshl/native

> The dshl native backend — a dsh plugin that loads the dshl-native napi-rs
> addon (.node DLL) and provides the full kernel capability set (window,
> tray, supervisor, terminal, OS actions) as the `dshlNativeBackend` Cordis
> service.

## Overview

`@dshl/native` is a [dsh](https://github.com/hibays/dsh-launcher) plugin. It
loads the per-platform `.node` cdylib built from
[`crates/dshl-native`](https://github.com/hibays/dsh-launcher/tree/main/crates/dshl-native)
via [napi-rs](https://napi.rs). The DLL links the FULL dshl kernel — the
same `dshl-core` rlib the installer binary (`dshl.exe`) uses — so this
backend is NOT a subset of the installer track: it runs the same kernel
inside the hosting dsh/Node process via FFI.

Dual track = two ENTRY POINTS into the same Rust code:

| Track | Entry | Used when |
|-------|-------|-----------|
| A (installer exe) | `dshl.exe` → `dshl_core::run_cli()` | Standalone launcher |
| B (plugin DLL) | `addon.launch(opts)` → `dshl_core::run_with_options()` | Plugin-only deployment |

## Addon resolution order

`src/loader.js` resolves the `.node` binary in this order (never throws — a
present-but-unloadable addon degrades to `null` plus `lastLoadError()`):

1. The per-platform `@dshl/native-*` package from the table below, matched
   against `process.platform` / `process.arch` (npm/pnpm only installs the
   one that matches the current host).
2. Fallback: a repo-local build at
   `native/dshl-native.<platform>-<arch>.node` next to this plugin
   (gitignored, produced by `scripts/build-native.mjs`).

A `MODULE_NOT_FOUND` for the subpackage is treated as the expected miss and
falls through silently; any other subpackage load failure (truncated
download, wrong Node ABI, missing VC runtime) is recorded in
`lastLoadError()` so the failure stays diagnosable instead of looking like
an absent install.

## Service registration

`apply(ctx)` ALWAYS calls `ctx.provide('dshlNativeBackend', …)` — even when
the `.node` failed to load. Consumers discover the service opportunistically
via `ctx.get('dshlNativeBackend')`; there is no hard dependency, so the
container registration is the only discovery path and it must always exist.

- Addon loaded: the descriptor is `{ backend: 'native', version, window,
  tray, supervisor, terminal, actions, status, isKernelRunning }`.
- Addon unavailable: the same keys are registered with `backend: null` and
  null sub-objects, `isKernelRunning: () => false`, plus a `loadError` field
  carrying the error from `lastLoadError()`. Consumers should feature-detect
  with a truthy check on `backend` (and can surface `loadError` for logging)
  rather than assuming the service is absent.

The module also re-exports `lastLoadError()` so hosts can log WHY the
backend is null without going through the container.

## Plugin exports

| Export | Value / shape |
|--------|---------------|
| `name` | `'dshl-native'` |
| `inject` | `[]` — the plugin declares no Cordis injections; it only provides |
| `backend` | The native backend object, or `null` when the addon is unavailable |
| `lastLoadError` | `() => Error \| null` — why `backend` is null, if it is |
| `apply(ctx)` | Registers `dshlNativeBackend` (see above) and logs load status |

## Capability surface

- `version` — kernel version string (from `ping()`)
- `window` — show / hide / navigate / isVisible
- `tray` — show / hide / setIcon / isVisible
- `supervisor` — shutdown / restart / launch
- `terminal` — spawn / list / kill / resize / write / endpoint (xterm.js
  PTY over WebSocket)
- `actions` — openTerminal / openPath / openUrl / ping / platformInfo
- `status` — synchronous launcher status snapshot
- `isKernelRunning()` — boolean

## camelCase mapping

napi-derive exposes snake_case Rust symbols and struct fields to JS as
camelCase. `src/index.js` wraps them 1:1; the Rust side lives in
`crates/dshl-native/src/types.rs`.

| Rust (types.rs) | JS |
|-----------------|----|
| `enable_single_instance` (`LaunchOptions`) | `enableSingleInstance` |
| `enable_control_pipe` (`LaunchOptions`) | `enableControlPipe` |
| `install_signal_handler` (`LaunchOptions`) | `installSignalHandler` |
| `kernel_running` (`LaunchStatus`) | `kernelRunning` |
| `window_visible` (`LaunchStatus`) | `windowVisible` |
| `tray_visible` (`LaunchStatus`) | `trayVisible` |
| `prepend_path` (`TerminalSpawnOptions`) | `prependPath` |
| `ws_url` (`TerminalSpawnResult`) | `wsUrl` |
| `started_at_ms` (`TerminalSessionInfo`) | `startedAtMs` |
| `url_prefix` (`TerminalServerInfo`) | `urlPrefix` |

Addon functions follow the same rule (`windowShow`, `traySetIcon`,
`terminalSpawn`, `terminalWsEndpoint`, `launchStatus`, `platformInfo`, …);
fields without underscores (`config`, `debug`, `cwd`, `shell`, `cols`,
`rows`, `id`, `pid`, `host`, `port`, `token`) keep their names.

Note: `prependPath` MUST be passed camelCase to `terminalSpawn` — a
snake_case key is silently dropped by napi and the shell boots without the
dsh runtime PATH prefix.

## Per-platform packages

This package declares the six napi-rs subpackages as `optionalDependencies`
(see resolution order above — the subpackage is tried FIRST, the local
build is the fallback):

| Platform | Subpackage |
|----------|------------|
| Windows x64 | `@dshl/native-win32-x64-msvc` |
| Windows arm64 | `@dshl/native-win32-arm64-msvc` |
| macOS x64 | `@dshl/native-darwin-x64` |
| macOS arm64 | `@dshl/native-darwin-arm64` |
| Linux x64 | `@dshl/native-linux-x64-gnu` |
| Linux arm64 | `@dshl/native-linux-arm64-gnu` |

## Local development

For repo-local builds (no published subpackage), run:

```bash
node scripts/build-native.mjs           # debug
node scripts/build-native.mjs --release # release
```

The script runs `cargo build -p dshl-native` (debug or `--release`) at the
repo root, then copies the cdylib — `target/{debug,release}/dshl_native.dll`
on Windows, `libdshl_native.dylib` on macOS, `libdshl_native.so` on Linux —
to `native/dshl-native.<platform>-<arch>.node` beside this plugin
(gitignored, and excluded from npm publishes by `.npmignore`/`files`).
That filename is exactly what `src/loader.js` looks up in its fallback
step.

## License

MIT © hibays
