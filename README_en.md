# DSHL — DeepSeek Harness web launcher

> [简体中文](./README.md) | **English**

![_](docs/screenshot_1.webp)

DSHL is a small, native launcher written in **Rust** that boots the
**DeepSeek Harness web UI** (`dsh web`) inside a browser, using
[webui.me](https://webui.me) as the startup-UI wrapper. It checks the runtime,
installs `@deepseek-ai/dsh` if needed, boots it on an ephemeral port, and
routes the browser to it.

Everything is configurable through **`dshl.toml`**, and the launcher is built
on **dependency-free native async** (`std::future`) — no tokio, no async
runtime.

## Highlights

- **Single-file portable binary** — written in Rust, ships as **one
  platform-native executable**: no installer, no bundled runtime, no GUI
  toolkit. Copy it anywhere (a USB stick works) and run; a `dshl.toml` next
  to it (or in the platform config dir) keeps the setup portable too.
- **Runs in the browser** — the launcher's own UI needs no desktop framework:
  the startup page is served by an embedded local web server (webui.me) and
  opens in your system browser or an embedded WebView. The only real
  dependency on the machine is Node.js (which dsh itself needs anyway).
- **webui.me wrapper** — a lightweight startup page (progress, logs, config)
  that navigates to dsh once it is up (the window is kept and can be brought
  back at any time via the tray icon).
- **Five explicit startup flows**:
  1. check system environment & architecture,
  2. check the runtime (node/bun, with a fallback chain),
  3. decide domestic mirrors (`auto-mirror`),
  4. prepare dsh (`install` vs `x` mode),
  5. launch `dsh web` and capture its URL.
- **Fallback chain** for Node.js (`node` is always required):
  `fnm` → `cargo install fnm` → `nvm` → auto-install `fnm` into `~/.cache/bin`
  → (if everything fails) an in-UI prompt with the
  [fnm install guide](https://www.fnmnode.com/zh-cn/guide/install).
- **Platform-aware** — Windows (PowerShell), Linux, macOS (bash); dsh is
  spawned in a **hidden console + new process group**
  (`CREATE_NEW_CONSOLE | CREATE_NEW_PROCESS_GROUP`, `setsid` on Unix), path
  and shell differences.
  The Windows **release** binary is a GUI app (no console window), and when
  dsh is installed it runs the `dsh` command (`dsh.exe` / `dsh.cmd` / `dsh.sh`)
  directly in a hidden console so no stray `node` console pops up.
- **App logo & icons** — `assets/` holds only the two full-resolution vector
  sources: `dsh-black.svg` (black whale, **dark-mode aware** — flips to white
  automatically on dark themes) and `dsh-white.svg` (forced-white night
  variant). Everything else is generated from them via ffmpeg into
  `packing/<platform>/`: `packing/windows/dsh.ico` (16/32/48/64/128/256)
  embedded into the Windows `.exe`, `dsh-white.ico` for the white
  window/taskbar/**tray** icon on dark systems (DPI-aware: the closest source
  size is selected for the current scaling), and the png files used by the
  NSIS/deb/dmg packages (see **Icons** below).
- **Fully configurable** launch: package manager, runner, version, flags,
  mirrors.

## Requirements

| Tool   | Version        | Required?                                   |
|--------|----------------|---------------------------------------------|
| Node   | `>= 24.15.0`   | **always** (installs `26` if missing)       |
| Bun    | `>= 1.3.14`    | only when `pm = "bun"` / `exector = "bunx"` |
| fnm    | any            | preferred Node version manager              |
| cargo  | any            | used to `cargo install fnm` as fallback     |
| nvm    | any            | last managed fallback                       |

## Install / build

```sh
cargo build --release
```

The binary is `target/release/dshl` (`.exe` on Windows). The first build
fetches the prebuilt WebUI static library for your platform automatically.

Pre-built binaries for Windows / Linux / macOS are attached to every tagged
release (see `.github/workflows/release.yml`).

## Usage

```sh
# with a config file next to the executable or in the config dir
./dshl

# explicit config
./dshl --config ./dshl.toml
```

### Config file search & generation

The launcher looks for `dshl.toml` in the following order, **first match wins**:

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
and uses it — so there is always a real file to edit, and the **打开配置**
button on the startup page opens exactly that file. A config that fails to
parse falls back to the built-in defaults with an error shown on the startup
page.

## Debug logging

`./dshl --debug` (or `-v`, or setting `DSHL_LOG=1`) mirrors the full runtime
timeline — every flow step and process output line — to **stderr** with a
monotonic timestamp. Debug builds keep a console, so `cargo r -- --debug`
shows it live; release builds stay console-free.

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
mode        = "install"   # install | x
pm          = "npm"       # npm | bun | pnpm  (install mode)
exector     = "npx"       # npx | bunx | pnpx (x mode)
version     = "latest"    # "latest" = no suffix, else @deepseek-ai/dsh@<version>
auto-update = true        # keep @deepseek-ai/dsh up-to-date (both modes)
single-instance = false   # true = refuse to start dsh while another dsh is running (no dual-writer)

[ui]
mode    = "webview"   # webview | browser (a *preference*, always falls back)
close-to-tray = false # true = close the window fully into the tray (dsh keeps running); tray icon rebuilds it; Windows/Linux/macOS
single-instance = false # true = only one dshl instance; a second one activates the existing one (focus or restore)
```

An empty mirror address means that mirror is **not used**. Mirrors are applied
temporarily (environment variables / CLI flags) and are never written to any
global config.

### Modes

- `install` — check the installed `dsh` (and version); if missing/wrong,
  install `@deepseek-ai/dsh` with the configured `pm`
  (`bun add -g --ignore-scripts`, `npm i -g`, `pnpm add -g`), then run `dsh`.
- `x` — run directly via `npx` / `bunx` / `pnpx` (npx/pnpx use `--yes`). The
  target is the **bare `dsh` name** (`dsh@<version>` when pinned): `bunx dsh`
  resolves the already-installed `dsh` command (e.g. bun's global
  `~/.bun/bin/dsh`) without a registry round-trip, avoiding the
  "Resolving dependencies" stall of `bunx @deepseek-ai/dsh`; any download
  goes through the configured npm mirror. If the runner fails to start and
  `dsh` is installed, the launcher falls back to running `dsh` directly.

### Auto-update

`auto-update` (default `true`) keeps `@deepseek-ai/dsh` current when no version
is pinned (`version = "latest"`):

- **install mode** — on each launch the launcher queries the registry for the
  latest version and reinstalls if the installed one is older (capped at 5s;
  skipped silently when offline).
- **x mode** — the runner fetches the latest by default; with
  `auto-update = false` npx/pnpx use `--prefer-offline` (cached copy).

With `auto-update = false`, the launcher only installs dsh if it is missing and
never updates it afterwards. A pinned `version` (`1.2.3`) is always respected
regardless of `auto-update`.

### Single-instance for dsh (optional)

By default every dshl process manages its own dsh, so multiple instances may
coexist. With `[dsh] single-instance = true`, the launcher refuses to start
dsh when another dsh process is already running on the machine (whether
started manually or by another dshl) and shows an error instead — two
processes appending to the same session log corrupt it permanently
("corrupt session log: seq gap"). The check runs after the stale-process
cleanup, so a previous dsh of our own that exited cleanly is not a conflict.

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
  `SwitchToThisWindow`, so it works even after the second instance exited
  and its foreground grant was revoked; Linux has no generic focus API, so
  only the tray restore applies).

Note: since the second instance exits right away, re-run dshl from the
launcher/shortcut to bring the window back (equivalent to **double-clicking
the tray icon**).

## How it launches dsh

`dsh` is spawned as a **supervised child** (stdout/stderr streamed line-by-line
to `~/.cache/dshl/dsh.log` while the `http://127.0.0.1:<port>` line is captured).
The launcher then routes the startup window to that URL and **stays alive as a
supervisor**, so shutting down is always clean:

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
  **5-second countdown**
  before auto-restarting (立即重启 / 取消). On timeout (or 立即重启) the full
  launch pipeline runs again and jumps back to dsh; 取消 keeps the startup page
  for a manual 重试 or 退出. If the window is in the tray it is re-created
  first so the countdown is visible.
- Ctrl+C / SIGTERM → kills dsh, then exits.
- dsh is stopped **gracefully** (Ctrl+C on Windows via a hidden console +
  `GenerateConsoleCtrlEvent`, SIGTERM on Unix) and given up to 10s (retry
  path) / 30s (exit path) to exit on its own — dsh itself responds within
  ~5s. It is **never force-killed automatically**: when the grace period
  expires, the launch is cancelled and the user must explicitly confirm the
  **force-kill stale process** button on the startup page (`taskkill /F /T` /
  `SIGKILL`), which prevents two processes writing the same session log.
- force-killed launcher (Windows `TerminateProcess`) → dsh is reaped by a
  kill-on-close **Job Object** (Linux uses `PR_SET_PDEATHSIG`).
- closing the startup window before launch, or the **退出** button → stops dsh
  gracefully (Ctrl+C/SIGTERM) and exits.

## Project layout

```
src/
  main.rs        binary entry point
  lib.rs         module registry
  runtime.rs     minimal native-async executor (no tokio)
  config.rs      dshl.toml model + discovery
  mirror.rs      mirror resolution (temporary, never persisted)
  platform/      OS primitives (loosely coupled — see Architecture below)
    mod.rs       facade: re-exports the submodules; callers still write crate::platform::…
    detect.rs    OS/arch/shell detection
    paths.rs     directory discovery + executable lookup (which/tool/known_tool_dirs)
    actions.rs   OS actions (open a file in the file manager)
    process.rs   process liveness / tree-kill / process discovery
    dpi.rs       DPI awareness & scaling (Win32 + X11 dlopen)
    theme.rs     dark-mode detection + window theming (Win32)
    window.rs    Win32 window geometry / focus / discovery
    single_instance.rs  dshl single-instance mutex (lock file + activation signal)
  tray/          system tray (one implementation per OS, shared 6-function interface)
    windows.rs   hidden message window + Shell_NotifyIconW (all via windows-rs)
    linux.rs     libayatana-appindicator3 loaded at runtime via dlopen (no hard dep)
    macos.rs     tray-icon (AppKit backend, NSImage template icon)
  version.rs     semver parse/compare
  process/       process helpers (loosely coupled)
    mod.rs       facade: re-exports (run / with_env / AsyncChild / Output / …)
    capture.rs   synchronous capture (run) + command prep (with_env / prepare_spawn)
    child.rs     AsyncChild: streaming line queue + reaper thread + waker + graceful stop
    win_proc.rs  (Windows) CreateProcessW hidden console + Ctrl+C graceful stop (windows-rs)
    win_job.rs   (Windows) kill-on-close Job Object (windows-rs)
  wskeep.rs      keep-alive WebSocket (keeps the WebView window open)
  probe.rs       tool detection (node/bun/fnm/cargo/nvm/dsh)
  install/       runtime installers + fallback chains (one file per runtime)
    mod.rs       facade: constants (NODE_MIN/BUN_MIN/…) + re-exports
    node.rs      ensure_node + fnm→cargo→nvm→fnm auto-install fallback chain
    bun.rs       ensure_bun + direct-download→official-script→npm fallback chain
    pnpm.rs      ensure_pnpm + global-bin-dir resolution
    download.rs  zip download/extract + fnm binary download + github proxy prefix
    stream.rs    run_streaming (stream command output into the progress log)
    runtime.rs   Runtime model (PATH prefix)
  progress.rs    shared status state (UI-agnostic)
  flow/          the five startup flows
  ui/            webui.me startup window (split by responsibility — see Architecture)
    mod.rs       facade: setup / launch_flow / run_loop / request_shutdown
    state.rs     all shared atomic state (modules coordinate only through it)
    vfs.rs       virtual file handler (embedded startup page)
    bindings.rs  functions bound to the page (get_state / retry / force_kill_stale / …)
    window.rs    window lifecycle: create/close/geometry persistence/theme/handle tracking/tray restore
    launch.rs    launch flow: stale cleanup → config → flow::run → navigate → supervise
    supervisor.rs main event loop + window-gone detection + tray/single-instance requests
assets/          startup page (index.html / app.js / styles.css)
               + full-resolution icon sources (dsh-black.svg / dsh-white.svg)
packing/       platform-specific installers + generated raster icons
  windows/       dshl.nsi + dsh.ico / dsh-white.ico
  linux/         build-deb.sh + 256/512px dsh*.png
  macos/         build-dmg.sh + 1024px dsh*.png (.icns built at package time)
               + tray-black.rgba (32×32 menu-bar template icon, raw RGBA)
```

### Architecture (loose coupling)

- **`platform/` does OS primitives only** — detection, paths, processes, DPI,
  theme, window helpers, single-instance mutex. One concern per submodule,
  `mod.rs` is a thin facade (re-exports), so every `crate::platform::…` call
  site stayed unchanged.
- **All Windows APIs go through `windows-rs 0.62`** (the `windows` crate) —
  no hand-written `#[link] extern "system"` FFI blocks remain. Features are
  enabled per module (`Win32_UI_WindowsAndMessaging`, `Win32_UI_HiDpi`,
  `Win32_UI_Shell`, `Win32_Graphics_Dwm`, `Win32_System_Registry`,
  `Win32_System_Threading`, …).
- **`tray/` is decoupled from the UI** — the three platform implementations
  share one 6-function interface (`start` / `hide_to_tray` /
  `quit_requested` / `restore_requested` / `set_icon` / `shutdown`); events
  surface as atomic flags that `ui/supervisor.rs` polls, with no platform
  details leaking in. On macOS, AppKit requires the status item to be created
  on the main thread, so `start()` only records intent and the icon is built
  on the next main-thread poll (see the design notes at the top of
  `tray/macos.rs`).
- **`ui/` is split by responsibility** — all shared state lives in
  `state.rs` (modules never reach into each other's privates), `window.rs`
  owns the window itself, `launch.rs` the launch flow, `supervisor.rs` the
  event loop, `bindings.rs` is the page's only entry point, and `mod.rs` is
  a re-export facade.
- **`process/` is split by responsibility** — `capture.rs` for synchronous
  capture and command prep, `child.rs` for `AsyncChild`'s streaming line
  queue and graceful stop, and the Windows-only hidden-console spawn/Ctrl+C
  (`win_proc.rs`) and kill-on-close Job Object (`win_job.rs`) live in their
  own files; `mod.rs` re-exports the public API unchanged.
- **`install/` is split by runtime** — one file per installer (node / bun /
  pnpm), shared zip download, fnm binary download and github-proxy prefix in
  `download.rs`, `Runtime` model and `run_streaming` each standalone,
  `mod.rs` re-exports the public API unchanged.

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

## License

MIT.
