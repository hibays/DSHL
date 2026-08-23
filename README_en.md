# DSHL — DeepSeek Harness launcher

<p align="center">
  <a href="./README.md">简体中文</a> | <strong>English</strong>
</p>

[![Release](https://img.shields.io/github/v/release/hibays/DSHL?style=flat-square&logo=github)](https://github.com/hibays/DSHL/releases)
[![License](https://img.shields.io/github/license/hibays/DSHL?style=flat-square)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-4176e6?style=flat-square)]()

![_](docs/screenshot_1.webp)

DSHL is a small, native launcher written in **Rust** that boots the
**DeepSeek Harness WebUI** (`dsh web`) inside a browser, using
[webui.me](https://webui.me) as the startup-UI wrapper. It checks the runtime,
installs `@deepseek-ai/dsh` if needed, boots it on an ephemeral port, and
routes the browser to it. Everything is configurable through **`dshl.toml`**,
and the pipeline runs on a **tokio multi-thread runtime** (thinly wrapped by
`src/runtime.rs`: `block_on` / `spawn`) with true-async process I/O, timers
and the keep-alive WebSocket.

## Dual-track distribution (one Rust kernel)

The kernel is the workspace root crate **`dshl-core`** (`src/`); the two
distribution tracks differ only in their entry point:

- **Track A (installer)** — the platform-native `dshl` executable
  (`crates/dshl`, entry `main.rs → dshl_cli::run_cli()`).
- **Track B (plugin)** — the same kernel compiled as a napi-rs cdylib
  (`crates/dshl-native`, `#[napi] launch(...) → dshl_cli::run_with_options(...)`)
  shipped as a `.node` native addon installed into dsh as a
  [Cordis](https://cordis.js.org) plugin, running in-process via FFI; published
  to npm as `@dshl/native` (aggregating six platform subpackages), `@dshl/pipe`
  and `@dshl/control` (see **Plugin track** below).

Both tracks share the **`crates/dshl-cli`** entry layer (`RunOptions` /
`RunOutcome` / `RunHandle`): Track A parses CLI flags and runs the same
pipeline, Track B skips CLI parsing entirely and drives the pipeline on a
managed background thread so the Node event loop stays alive.

## Highlights

- **Single-file portable binary (Track A)** — ships as **one platform-native
  executable**: no installer, no bundled runtime, no GUI toolkit. Copy it
  anywhere (a USB stick works) and run; a `dshl.toml` next to it (or in the
  platform config dir) keeps the setup portable too.
- **Runs in the browser** — the launcher's own UI needs no desktop framework:
  the startup page is served by an embedded local web server (webui.me) and
  opens in your system browser or an embedded WebView. The only real
  dependency on the machine is Node.js (which dsh itself needs anyway).
- **Five explicit startup flows** (`src/flow/`, one per startup-page step):
  1. `system` — check OS environment & architecture,
  2. `runtime_env` — check the runtime (node/bun, with a fallback chain),
  3. `mirror_check` — domestic-mirror decision (`auto-mirror`),
  4. `prepare` — make dsh available (global/hybrid/private) and build the
     launch command,
  5. `launch` — spawn `dsh web` and capture its URL.
- **Fallback chain** for Node.js (node is always required, min 24.15.0;
  missing node installs `26` via fnm): `fnm` → `cargo install fnm` → `nvm` →
  auto-install fnm into `~/.cache/bin` → (if everything fails) an in-UI prompt
  with the [fnm install guide](https://www.fnmnode.com/zh-cn/guide/install).
- **Embedded terminal (PTY + WebSocket + xterm.js)** — the Rust side owns the
  PTY master/slave pair via `portable-pty` (ConPTY on Windows,
  openpty/forkpty on Unix), injects PATH/env/cwd, and exposes each session
  through a self-hosted WebSocket server (bound to `127.0.0.1:0`, authenticated
  by a 64-hex-char token) as pre-signed URLs
  `ws://127.0.0.1:<port>/_pty/<id>?token=...`; xterm.js ships as vendored
  static assets inside `@dshl/control` (whitelisted routes, no CDN). See
  `src/pty/` (`spawn/list/resize/write/kill/server_endpoint`).
- **Control plane** — the launcher exposes a newline-delimited JSON-RPC
  endpoint on a loopback TCP socket, giving the supervised dsh process native
  capabilities (shutdown/restart/switch-profile/open-terminal/ping); a random
  per-launch token is handed to dsh via the `DSHL_CONTROL_URL` env var. See
  **Control plane** below.
- **Window geometry memory** — a single `<cache>/dshl/window-state.json`
  stores `{x,y,width,height}` (physical pixels) shared by the WebView window
  and the external browser window alike; every value is clamped before being
  handed to webui (its C core silently drops out-of-range values), and browser
  mode converts through the DPI scale. See `src/ui/geometry.rs`.
- **System tray** — three implementations behind one shared 6-function
  interface: Windows (hidden message window + Shell_NotifyIconW, all via
  windows-rs), Linux (libayatana-appindicator3 dlopen'd at runtime), macOS
  (tray-icon's AppKit backend); events surface as atomic flags polled by
  `ui/supervisor.rs`.
- **Contract validation (plugin track)** — `@dshl/control` validates backends
  against `backend-contract.js` tiers (native full set / pipe subset) with
  warn-only drift detection, and whitelists `launch()` options through
  `resolveLaunchOptions`; the guard component does crash tracking and rollback
  bookkeeping (see **Plugin track**).
- **Platform-aware** — Windows (PowerShell), Linux, macOS (bash); dsh is
  spawned in a hidden console + new process group
  (`CREATE_NEW_CONSOLE | CREATE_NEW_PROCESS_GROUP`, `setsid` on Unix). The
  Windows **release** binary is a GUI app (no console window); when dsh is
  installed it runs the `dsh` command (`dsh.exe`/`dsh.cmd`/`dsh.sh`) directly
  in a hidden console so no stray `node` console pops up.
- **App logo & icons** — `assets/` holds only the two full-resolution vector
  sources: `dsh-black.svg` (black whale, **dark-mode aware** — flips to white
  automatically on dark themes) and `dsh-white.svg` (forced-white night
  variant). Everything else is generated from them via ffmpeg into
  `packing/<platform>/` (see **Icons** below).
- **Fully configurable** launch: package manager, version, flags, mirrors.

## Requirements

| Tool  | Version      | Required?                              |
|-------|--------------|----------------------------------------|
| Node  | `>= 24.15.0` | **always** (installed via the assembly chain below when missing) |
| Nub   | npm latest   | default (`pm = "nub"`); fetched through the npm mirror into dshl's cache |
| Bun   | `>= 1.3.14`  | only when `pm = "bun"`                 |
| fnm   | any          | preferred Node manager when no mirror is configured |
| nub   | npm latest   | preferred when a mirror is configured (also acts as the pm) |
| cargo | any          | used to `cargo install fnm` as fallback|
| nvm   | any          | last managed fallback                  |

## Runtime assembly chain

Step 2 of the pipeline probes node / bun / pnpm / nub / fnm / cargo / nvm
concurrently and logs each tool's version and path — transparency only; the
real assembly happens on top of the probe results and always follows one
first principle: **an existing tool that already satisfies the requirement is
used as-is, never reinstalled**.

node is the single hard prerequisite (min 24.15.0). When a satisfying install
is probed, its directory becomes the runtime directory and nothing is
downloaded. Only a missing or outdated node enters assembly, and the path
taken is decided by two things: whether the configured package manager is
nub, and whether mirrors are enabled.

With mirrors on and pm="nub", assembly starts at the npm mirror: @nubjs/nub is
installed into dshl's own cache (same mechanism as pnpm; the registry comes
from mirrors.npm, so it is mirrorable by construction), and nub then provides
node itself - `nub node install` pulls the dist through NODEJS_ORG_MIRROR
(sourced from mirrors.nodejs_release) and `nub node which` resolves the
binary directory, which becomes this run's runtime directory. Any failure in
the chain falls through immediately to fnm (the preferred version manager
without mirrors) -> cargo install fnm -> nvm -> auto-install into
~/.cache/bin, ending with an in-UI link to the fnm install guide.

Package managers follow the same use-what-exists/install-into-cache pattern:
a missing bun is downloaded via GitHub mirror with official-script and npm
fallbacks; pnpm and nub land via `npm install --prefix <cache>/dshl/<name>`
with their node_modules/.bin injected into PATH. The mirror layer cross-cuts
every network step above: mirrors.npm feeds npm/bun/pnpm/nub registries and
installs alike, mirrors.nodejs_release feeds fnm/nvm/nub Node dist downloads,
and everything is injected as temporary env or flags - never written to any
global config file.

## Build

```sh
cargo build --release          # the whole workspace
cargo build --release -p dshl  # Track A binary only
```

The binary is `target/release/dshl` (`.exe` on Windows). The first build
compiles the WebUI C library for your platform automatically (webui-rs is a
git source dependency — no prebuilt library needed).

### Gates (fmt / clippy / test / JS checks)

The gate logic has a single source: `scripts/gate.sh` (POSIX) /
`scripts/gate.ps1` (PowerShell) / `scripts/gate.bat` (cmd forwarder). CI and
local developers run the exact same command:

```sh
scripts/gate.sh          # everything: cargo fmt --check + clippy -D warnings + test --workspace --locked + npm run check + npm pack dry-run
scripts/gate.sh --rust   # Rust gates only
scripts/gate.sh --js     # npm run check (node --check syntax) + npm pack --workspaces --dry-run
```

PowerShell equivalents: `./scripts/gate.ps1 [-Rust|-Js]`.

### Packaging (Track A)

The packaging steps also have a single source: `scripts/package.sh` (called
per-step by CI, end-to-end locally):

```sh
bash scripts/package.sh all                          # current host: build + portable zip + platform installer
bash scripts/package.sh stage --bin PATH_TO_BIN      # assemble stage/ (binary + both READMEs)
bash scripts/package.sh portable --zip NAME.zip      # stage/* + default dshl.toml -> NAME.zip
bash scripts/package.sh nsis   --version V --artifact NAME   # stage/ -> dshl-<NAME>-setup.exe (needs NSIS)
bash scripts/package.sh deb    --version V --deb-arch amd64  # stage/dshl -> .deb (needs dpkg-deb)
bash scripts/package.sh dmg    --version V --artifact NAME   # stage/dshl -> dshl-<NAME>.dmg (needs macOS)
```

Note: **dshl.toml ships only in the portable zip**; installers carry no config
(the launcher auto-generates a fully commented template on first run). The
PowerShell equivalent is `scripts/package.ps1`.

### Publishing (npm, Track B)

```sh
node plugins/dshl-native/scripts/build-native.mjs --release  # or: npm run build:native (local host .node)
scripts/publish.sh --version 0.3.0            # sync version into the three packages + check + pack dry-run + publish
scripts/publish.sh --version 0.3.0 --dry-run  # bump + verify only, no publish
scripts/publish.sh                            # publish at current package.json versions
```

Publish order is fixed `@dshl/native → @dshl/pipe → @dshl/control` (control
lists the other two as optionalDependencies, so it must go last). The six
platform subpackages `@dshl/native-<platform>-<arch>` are NOT published
locally — they are built and published by CI only. `--provenance` requires
OIDC and is CI-only.

## CI / release workflows

| Workflow | Trigger | What it does |
|----------|---------|--------------|
| `ci.yml` (CI) | push to `main`, every PR | Rust gates on ubuntu-latest and windows-11-arm (two legs, via the gate scripts); JS job (Node 22) runs the syntax check + pack dry-run |
| `release.yml` (Release · Track A) | tag push `v*` | six matrix legs (Windows x64 / Windows ARM64 native runner / Linux x64 / Linux arm64 cross / macOS x64 / macOS arm64): `cargo build --release --locked -p dshl` → `package.sh` produces portable zip + NSIS/deb/dmg, generates a changelog and creates the GitHub Release |
| `release-native.yml` (Release · Track B · native .node) | tag push `v*` | six matrix legs each build the cdylib and publish `@dshl/native-{win32-x64-msvc, win32-arm64-msvc, darwin-x64, darwin-arm64, linux-x64-gnu, linux-arm64-gnu}` platform subpackages (`index.node` + generated package.json, `--provenance`) |
| `release-plugins.yml` (Release · Track B · npm aggregators) | `workflow_run` chained after the native workflow succeeds (and its ref is a tag) | `bump-versions.mjs` syncs the three package versions → `npm pack` dry-run → publishes `@dshl/native`, `@dshl/pipe`, `@dshl/control` in order |

All three release workflows use concurrency groups (a same-tag re-run replaces
the in-flight one). **When the `NPM_TOKEN` secret is not configured, every npm
publish step skips itself** (prints a notice and exits 0 — not a failure), so
tag runs on forks only produce Track A artifacts and never go red.

## Usage

```sh
# with a config file next to the executable or in the config dir
./dshl

# explicit config
./dshl --config ./dshl.toml
```

Full CLI surface: `-c/--config <path>`, `-d/--debug` (`-v/--verbose` is an
alias), `-V/--version`, `-h/--help`.

### Config file search & generation

The launcher looks for `dshl.toml` in the following order, **first match
wins** (`src/config.rs::load`):

1. an explicit `--config <path>` (a missing file is a hard, visible error);
2. the current working directory: `./dshl.toml`;
3. the executable directory: `<exe>/dshl.toml`;
4. the platform config directory:
   - Windows: `%APPDATA%\dshl\dshl.toml`
   - macOS: `~/Library/Application Support/dshl/dshl.toml`
   - Linux: `$XDG_CONFIG_HOME/dshl/dshl.toml` (or `~/.config/dshl/dshl.toml`
     when unset);
5. the Linux system-wide config (lowest priority): `/etc/dshl/dshl.toml`
   (the package ships without a config — place the file manually if needed).

If none exists, the launcher **generates a default template** in the platform
config directory (a compile-time copy of `dshl.example.toml`, fully commented)
and uses it — so there is always a real file to edit, and the **Open config**
button on the startup page opens exactly that file. A config that fails to
parse falls back to the built-in defaults with an error shown on the startup
page.

## Debug logging

`./dshl --debug` (or `-v`, or a non-empty `DSHL_LOG`) mirrors the full runtime
timeline — every flow step and process output line — to **stderr** with a
monotonic timestamp (`src/debug.rs`). Debug builds keep a console, so
`cargo r -- --debug` shows it live; release builds stay console-free.

## Configuration (`dshl.toml`)

```toml
# off = no mirrors, on = use non-empty mirrors (default), force = strict
auto-mirror = "on"

[mirrors]
npm            = "http://registry.npmmirror.com"     # also used by bun
cargo          = "sparse+https://rsproxy.cn/index/"  # temporary, CLI-only
nodejs-release = "https://mirrors.aliyun.com/nodejs-release/"
bun-download   = ""                                   # empty = not used
github         = ""                                   # proxy prefix, e.g. https://ghproxy.com/

[dsh]
flags       = "--profile web --host 127.0.0.1 --port 0"
mode        = "hybrid"    # global | hybrid | private
pm          = "npm"       # npm | bun | pnpm
version     = "latest"    # "latest" = no suffix, else @deepseek-ai/dsh@<version>
auto-update = true        # keep @deepseek-ai/dsh up-to-date
single-instance = false   # true = refuse to start dsh while another dsh is running (no dual-writer)

[ui]
mode    = "webview"   # webview | browser (a *preference*, always falls back)
close-to-tray = false # true = close the window fully into the tray (dsh keeps running); tray icon rebuilds it; Windows/Linux/macOS
single-instance = false # true = only one dshl instance; a second one activates the existing one (focus or restore)
```

An empty mirror address means that mirror is **not used**. Mirrors are applied
temporarily (environment variables / CLI flags) and are never written to any
global config.

### Modes (`dsh.mode`)

Where dsh comes from:

- `global` — strictly the global `dsh` on your PATH; the launcher errors out
  (instead of installing) when it is missing. For users who manage dsh
  themselves.
- `hybrid` (default) — prefer the global `dsh`; when it is missing or does not
  satisfy the pinned `version`, install `@deepseek-ai/dsh` into dshl's private
  cache and run it as `node <entry>`. Never `-g`.
- `private` — always install into dshl's cache (`<cache>/dshl/node_modules`),
  never touching your global environment or PATH.

### Auto-update

`auto-update` (default `true`) keeps `@deepseek-ai/dsh` current when no version
is pinned (`version = "latest"`):

- In `hybrid`/`private` mode, on each launch the launcher queries the registry
  for the latest version and reinstalls the cache copy when it is older (capped
  at 5s; skipped silently when offline). In `hybrid` mode the global dsh is used
  only when it is already up to date.
- `global` mode is untouched by auto-update — you own the global install.

With `auto-update = false`, the launcher only installs dsh if it is missing and
never updates it afterwards. A pinned `version` (`1.2.3`) is always respected
regardless of `auto-update`.

### Single-instance for dsh (optional)

By default every dshl process manages its own dsh, so multiple instances may
coexist. With `[dsh] single-instance = true`, the launcher refuses to start
dsh when another dsh process is already running on the machine (whether
started manually or by another dshl) and shows an error instead — two
processes appending to the same session log corrupt it permanently. The check
runs after the stale-process cleanup, so a previous dsh of our own that exited
cleanly is not a conflict.

### Close to tray (optional)

By default closing the window exits (and gracefully stops dsh). With
`close-to-tray = true`, once dsh is up, closing the window no longer exits —
dsh keeps running in the background:

- **The window is closed for real** (WebView2 / WebKitGTK processes exit,
  memory is freed); only the tray icon, the launcher and dsh stay resident.
- **The tray icon is created at startup** (not only after the first window
  close) and switches between the black/white variants with the OS theme.
- **Windows** — the tray icon reuses the window icon. Left-**double-click** or
  the **Restore** menu item **rebuilds** the window (restoring the saved
  geometry) and navigates back to dsh; a single click does nothing (anti
  accidental). The menu also has **Open dsh** (opens the dsh page in the
  system default browser) and **Quit** (same graceful shutdown path as
  Ctrl+C: SIGINT stops dsh and saves its session).
- **Linux** — WebKitGTK windows cannot be intercepted, so after a close the
  launcher keeps running **without a window**; the tray icon
  (libayatana-appindicator3, loaded at runtime; without the library the
  feature degrades to close-to-exit) offers **Restore window** (rebuilds and
  re-navigates to dsh), **Open dsh** and **Quit**.
- **macOS** — same model as Linux: after the WKWebView window closes the
  launcher keeps running without a window; a menu-bar status item
  (`tray-icon`'s AppKit backend, created on the main thread) offers
  **Restore window**, **Open dsh** and **Quit**. The icon is an **NSImage
  template** (black + alpha mask), so macOS renders it in the menu-bar colour
  in both light and dark mode automatically — no icon swapping needed; a
  single left click restores the window, right click opens the menu.

Closing the window during startup (dsh not ready yet) still exits directly.

### Window geometry memory

Regardless of the `webview` or `browser` backend, window position and size are
persisted to `<cache>/dshl/window-state.json` (physical pixels), and both
backends **share the same store** — the user's layout follows the launcher,
not the backend. Recording points: the WebView close hook, a 1 Hz sampler
while the external browser runs, plus a one-shot capture at shutdown (webui
offers no browser-side close hook). Every restored value is clamped to the
current screen and webui's hard acceptance ranges (out-of-range values are
silently dropped by webui's C core), and converted from physical to logical
pixels through the DPI scale for browser command lines.

### Single-instance for dshl (optional)

`[dsh] single-instance` guards **dsh itself**; `[ui] single-instance`
guards the **dshl launcher**: when enabled, only one dshl process may run on
the machine (lock file + kernel file lock — released automatically on crash,
so stale locks are impossible). A second dshl does not create a new window;
it **activates the existing instance** and exits:

- existing instance in tray / windowless state → brings the window back
  (rebuilds it and navigates back to dsh);
- existing instance window visible → focuses it once (Windows multi-level:
  `SetForegroundWindow` → input-queue attach → synthesized Alt key →
  `SwitchToThisWindow`; Linux has no generic focus API, so only the tray
  restore applies).

Note: since the second instance exits right away, re-run dshl from the
launcher/shortcut to bring the window back (equivalent to **double-clicking
the tray icon**).

## How it launches dsh

`dsh` is spawned as a **supervised child** (stdout/stderr streamed line-by-line
to `<cache>/dshl/dsh.log` while the `http://127.0.0.1:<port>` line is
captured). The launcher then routes the startup window to that URL and **stays
alive as a supervisor**, so shutting down is always clean:

- **closing the window that shows dsh** → by default dsh is stopped and the
  launcher exits; with `close-to-tray` enabled (and dsh already up) it goes
  to the **tray** instead and dsh keeps running. This works for both window
  backends:
  - `webview` — embedded WebView (WebView2 / WKWebView / WebKitGTK). The launcher
    holds a keep-alive WebSocket to its own webui server (`multi_client`) so the
    window stays open after it navigates to dsh, and detects the close via
    webui's `set_close_handler_wv` (plus the window handle on Windows). The
    process is made DPI-aware (`PerMonitorV2`) so the WebView is crisp on
    high-DPI displays.
  - `browser` — external browser (Chrome/Edge/Firefox…). No keep-alive is
    needed: webui's server-timeout path tries to terminate the *external*
    browser process, but that lookup is best-effort and does not fire on
    modern Windows/Edge, so the browser stays open on its own after
    navigating to dsh. The launcher detects the close by tracking the browser
    window process and polling for its exit, and (with `close-to-tray`) hands
    it over to the tray — the tray re-opens the browser on restore.
- dsh exits **cleanly** (exit 0, e.g. its graceful Ctrl+C shutdown) → the
  launcher exits too.
- after a successful launch, dsh exits **unexpectedly** (non-zero exit code /
  killed by a signal) → **crash recovery**: the window jumps back to the
  startup page and shows a "dsh exited unexpectedly (exit N)" banner with a
  **5-second countdown** before auto-restarting (Restart now / Cancel). On
  timeout (or Restart now) the full launch pipeline runs again and jumps back
  to dsh; Cancel keeps the startup page for a manual Retry or Quit. If the
  window is in the tray it is re-created first so the countdown is visible.
- Ctrl+C / SIGTERM → kills dsh, then exits.
- dsh is stopped **gracefully** (Ctrl+C on Windows via a hidden console +
  `GenerateConsoleCtrlEvent`, SIGTERM on Unix) and given up to 10s (retry
  path) / 30s (exit path) to exit on its own. It is **never force-killed
  automatically**: when the grace period expires, the launch is cancelled and
  the user must explicitly confirm the **force-kill stale process** button on
  the startup page (`taskkill /F /T` / `SIGKILL`), which prevents two
  processes writing the same session log.
- force-killed launcher (Windows `TerminateProcess`) → dsh is reaped by a
  kill-on-close **Job Object** (Linux uses `PR_SET_PDEATHSIG`).
- closing the startup window before launch, or the **Quit** button → stops dsh
  gracefully (Ctrl+C/SIGTERM) and exits.

## Control plane

`src/control.rs` implements the Rust side of the `@dshl/control` plugin
contract: a newline-delimited JSON endpoint on a loopback TCP socket that
exposes the launcher's native capabilities to the supervised dsh process.

- **Handshake**: the client first sends
  `{"type":"hello","token":"<per-launch token>"}` (5s timeout), then
  request/response frames are exchanged (64 KiB max frame).
- **Token**: minted from the platform CSPRNG each launch (UUID v4, 122 bits of
  entropy) and handed to dsh via the `DSHL_CONTROL_URL` env var (format
  `dshl://<token>@127.0.0.1:<port>`); logs carry the port only, never the
  token.
- **Method set**: `ping` (pong + version), `shutdown` (requests a graceful
  shutdown), `switch-profile` (persists the pending profile and triggers a
  restart, effective on next boot), `open-terminal` (opens a terminal with the
  augmented PATH of the most recent dsh launch), `restart` (runs the full
  launch pipeline again); unknown methods return an error frame.
- **Enablement**: on by default for the CLI entry
  (`RunOptions.enable_control_pipe` can turn it off, letting an embedding host
  decide).

## Plugin track (npm packages)

The three packages under `plugins/` form Track B (the monorepo is managed by
the root `package.json` workspaces, Node ≥ 22 required):

- **`@dshl/native`** — the napi loader. Prefers a locally built host `.node`
  (`npm run build:native`); otherwise resolves one of the six CI-published
  `@dshl/native-<platform>-<arch>` subpackages (optionalDependencies) by
  platform.
- **`@dshl/pipe`** — the remote control-pipe backend: reads `DSHL_CONTROL_URL`
  and connects to a running dshl control plane (REMOTE tier — no window /
  tray / terminal capabilities).
- **`@dshl/control`** — the aggregating consumer. It acquires backends through
  Cordis's optional seam (`ctx.get('dshlNativeBackend') ?? null` — an absent
  provider simply resolves to undefined; never `require()`, which would bypass
  the container), folds whichever providers are present into the
  `nativeCapabilities` service, and registers a set of **loopback-only** HTTP
  routes (remoteAddress and Host header both checked) on dsh's web server:
  status, window show/hide/navigate, tray, launch, guard management and
  terminal control.
  - `window/show` returns **409 `{ code: 'booting' }`** while the launcher is
    still starting, so the UI can localize honestly instead of flashing
    success for a click that did nothing;
  - `/dshl-control/launch` whitelists options through `resolveLaunchOptions` —
    only the documented `LaunchOptions` keys (`config/debug/
    enableSingleInstance/enableControlPipe/installSignalHandler`) pass
    through, so snake_case mistakes cannot be silently swallowed by napi;
  - xterm.js assets are served via a **fixed whitelist** (vendored under
    `assets/xterm`, no CDN dependency, no path-traversal surface).
- **backend-contract.js (contract validation)** — defines the capability
  groups each tier (native / pipe) MUST expose; `checkBackend(tier, backend)`
  performs warn-only drift detection (a missing method just degrades one
  route to 501, it does not take the whole bridge down).
- **plugin-guard.js (disable guard + crash rollback)** — persists
  `disabled.json` and `launch-state.json` under `$DSH_HOME/.dshl/` (fallback
  `~/.dsh/.dshl/`): `beginStartup` records startedAt and sets healthy=false;
  the renderer must call `markHealthy` within a 30s window (HTTP
  `POST /dshl-control/plugins/mark-healthy`); the plugin's dispose hook
  **automatically calls `markShutdown`** — a normal teardown never counts as a
  crash. On the next startup, if the previous run was neither healthy nor a
  graceful exit (beyond the 10s grace), a consecutive crash is counted and any
  bundle present now but missing from the last healthy snapshot is flagged as
  suspicious; after 3 consecutive crashes the suspicious bundles are recorded
  into disabled.json (reason `crash-3x`).
  Honest caveat: nothing in the dsh plugin loader reads disabled.json today —
  the disable list is **bookkeeping + visibility** (exposed via services and
  HTTP routes), not a load-time block.

## Project layout

The tree below mirrors the actual `git ls-files` output:

```
Cargo.toml             workspace root (dshl-core kernel + three member crates)
package.json           monorepo: workspaces + scripts for the three @dshl/* npm packages
dshl.example.toml      fully commented default config template (compile-time embedded, auto-generated when missing)
assets/                startup page (index.html / app.js / styles.css)
                       + full-resolution icon vector sources (dsh-black.svg / dsh-white.svg)
locales/               i18n translations (en.yml / zh-CN.yml, compile-time embedded, zh-CN fallback)
docs/                  screenshot
packing/               platform installer assets + generated raster icons
  windows/               dshl.nsi + dsh.ico / dsh-white.ico
  linux/                 build-deb.sh + dsh*.png (256/512px)
  macos/                 build-dmg.sh + dsh*.png + tray-black.rgba (32×32 menu-bar template, raw RGBA)
scripts/               gate.{sh,ps1,bat} gates; package.{sh,ps1} packaging; publish.{sh,ps1} npm publishing; bump-versions.mjs
.github/workflows/     ci.yml; release.yml (Track A); release-native.yml; release-plugins.yml

src/                   dshl-core kernel (lib.rs registers modules)
  lib.rs                module registry + DSH_CHILD global
  runtime.rs            thin wrapper over the tokio multi-thread runtime (block_on / spawn)
  config.rs             dshl.toml model + discovery + default template generation
  control.rs            control plane (loopback TCP NDJSON RPC, see above)
  error.rs              tiny error type (Error(String) + bail! macro + Result alias)
  i18n.rs               locale detection + t! translation init
  mirror.rs             mirror resolution (temporary, never persisted)
  probe.rs              tool detection (node/bun/fnm/cargo/nvm/dsh)
  progress.rs           shared status state (UI-agnostic)
  version.rs            semver parse/compare
  wskeep.rs             keep-alive WebSocket (keeps the WebView window open)
  debug.rs              stderr timeline logging (--debug / DSHL_LOG)
  testutil.rs           test helpers (per-platform shell command construction)
  flow/                 the five startup flows (system / runtime_env / mirror_check / prepare / launch)
  install/              runtime installers + fallback chains (node/bun/pnpm/download/stream/runtime)
  platform/             OS primitives (detect/paths/actions/process/dpi/theme/window/single_instance)
  process/              process helpers (capture/child/win_proc/win_job)
  pty/                  embedded PTY service + self-hosted WS server (server/session/types)
  tray/                 system tray (windows/linux/macos, shared 6-function interface)
  ui/                   webui.me startup window (state/bindings/window/launch/supervisor/vfs/exit/crash/geometry/assets)

crates/
  dshl/                 Track A binary (main.rs + build.rs embedding the .exe icon resource)
  dshl-cli/             shared entry layer (options/handle/run/signal/control shims)
  dshl-native/          Track B napi-rs cdylib (kernel/platform/pty/tray/window/supervisor/types)

plugins/
  dshl-native/          @dshl/native — napi loader + build-native.mjs
  dshl-pipe/            @dshl/pipe — remote control-pipe backend (client/index)
  dshl-control/         @dshl/control — aggregating consumer (index/ui/plugin-guard/backend-contract
                        + vendored xterm.js assets)
```

### Architecture (loose coupling)

- **`platform/` does OS primitives only** — detection, paths, processes, DPI,
  theme, window helpers, single-instance mutex. One concern per submodule,
  `mod.rs` is a thin facade (re-exports), so every `crate::platform::…` call
  site stays unchanged.
- **All Windows APIs go through `windows-rs 0.62`** — no hand-written
  `#[link] extern "system"` FFI blocks remain; features are enabled per
  module.
- **`tray/` is decoupled from the UI** — the three platform implementations
  share one 6-function interface (`start` / `hide_to_tray` /
  `quit_requested` / `restore_requested` / `set_icon` / `shutdown`); events
  surface as atomic flags that `ui/supervisor.rs` polls, with no platform
  details leaking in. On macOS, AppKit requires the status item to be created
  on the main thread, so `start()` only records intent and the icon is built
  on the next main-thread poll.
- **`ui/` is split by responsibility** — all shared state lives in
  `state.rs` (modules never reach into each other's privates), `window.rs`
  owns the window itself (including geometry persistence), `launch.rs` the
  launch flow, `supervisor.rs` the event loop, `bindings.rs` is the page's
  only entry point, and `mod.rs` is a re-export facade (`setup` /
  `launch_flow` / `run_loop` / `request_shutdown`, …).
- **`process/` is split by responsibility** — `capture.rs` for synchronous
  capture and command prep, `child.rs` for `AsyncChild`'s streaming line
  queue and graceful stop, and the Windows-only hidden-console spawn/Ctrl+C
  (`win_proc.rs`) and kill-on-close Job Object (`win_job.rs`) live in their
  own files.
- **`install/` is split by runtime** — one file per installer (node / bun /
  pnpm), shared zip download, fnm binary download and github-proxy prefix in
  `download.rs`, `Runtime` model and `run_streaming` each standalone.
- **Error handling** — `error.rs` provides the dependency-free
  `Error(String)`, the `bail!` macro and a `Result` alias, used uniformly
  across modules.
- **i18n** — `rust-i18n` embeds `locales/` at compile time; the `t!` macro is
  available crate-wide with zh-CN as the fallback language; the default locale
  comes from OS UI-language detection (`sys-locale`).
- **Test infrastructure** — `testutil.rs` provides
  `shell(win_cmd, unix_cmd)`, which builds a subprocess test `Command` using
  `%COMSPEC% /c` on Windows or `sh -c` on Unix, replacing the formerly
  copy-pasted COMSPEC lookups (which mis-selected under WSL interop).

## Icons

`assets/` keeps only the two **full-resolution vector sources**
(1024×1024 declared size — vector rendering, crisp at every scale):

- `dsh-black.svg` — black whale with a built-in `prefers-color-scheme: dark`
  rule, so the page/tab mark turns white on dark themes automatically.
- `dsh-white.svg` — forced-white night variant (dark docks / window / tray icons).

All raster icons are generated from these with ffmpeg (librsvg) and committed
under `packing/<platform>/`:

```sh
# Windows multi-size .ico (16/32/48/64/128/256) — black default / white night
ffmpeg -hide_banner -y -i assets/dsh-black.svg \
  -filter_complex "split=6[a][b][c][d][e][f];[a]scale=16:16:flags=lanczos[b];\
[b]scale=32:32:flags=lanczos[c];[c]scale=48:48:flags=lanczos[d];\
[d]scale=64:64:flags=lanczos[e];[e]scale=128:128:flags=lanczos[f];\
[f]scale=256:256:flags=lanczos[g]" \
  -map "[b]" -map "[c]" -map "[d]" -map "[e]" -map "[f]" -map "[g]" \
  -c:v bmp packing/windows/dsh.ico
# white variant: use assets/dsh-white.svg as input → packing/windows/dsh-white.ico

# Linux PNGs (256/512) and macOS PNG (1024; .icns is built from it at package
# time with sips + iconutil):
ffmpeg -hide_banner -y -i assets/dsh-black.svg -vf "scale=256:256:flags=lanczos" -frames:v 1 packing/linux/dsh.png
ffmpeg -hide_banner -y -i assets/dsh-black.svg -vf "scale=512:512:flags=lanczos" -frames:v 1 packing/linux/dsh-512.png
ffmpeg -hide_banner -y -i assets/dsh-black.svg -frames:v 1 packing/macos/dsh.png
# white variant: input assets/dsh-white.svg → dsh-white*.png outputs

# macOS menu-bar template icon (32×32 raw RGBA, embedded at compile time;
# black + alpha mask — the system renders it in the menu-bar colour):
ffmpeg -hide_banner -y -i assets/dsh-black.svg -pix_fmt rgba -f rawvideo -s 32x32 packing/macos/tray-black.rgba
```

> **⚠️ Disclaimer**  
> This tool (DSHL) is a **community third-party launcher** and is **not affiliated with, endorsed by, or recognized by DeepSeek** in any direct or indirect way.  
> All trademarks mentioned in this project (including "DeepSeek", "DeepSeek Harness", etc.) belong to their respective legal owners.

## License

MIT.
