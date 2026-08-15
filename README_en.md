> 🌐 [简体中文](./README.md) | **English**

# DSHL — DeepSeek Harness web launcher

DSHL is a small, native launcher that boots the **DeepSeek Harness web UI**
(`dsh web`) inside a browser, using [webui.me](https://webui.me) as the
startup-UI wrapper. It checks the runtime, installs `@deepseek-ai/dsh` if
needed, boots it on an ephemeral port, and routes the browser to it.

Everything is configurable through **`dshl.toml`**, and the launcher is built
on **dependency-free native async** (`std::future`) — no tokio, no async
runtime.

## Highlights

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

| Tool   | Version        | Required?                              |
|--------|----------------|----------------------------------------|
| Node   | `>= 24.15.0`   | **always** (installs `26` if missing)  |
| Bun    | `>= 1.3.14`    | only when `pm = "bun"` / `exector = "bunx"` |
| fnm    | any            | preferred Node version manager         |
| cargo  | any            | used to `cargo install fnm` as fallback |
| nvm    | any            | last managed fallback                  |

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
5. the Linux system-wide config (lowest priority, installed by the distro
   package): `/etc/dshl/dshl.toml`.

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
close-to-tray = false # true = close the window fully into the tray (dsh keeps running); tray icon rebuilds it; Windows/Linux
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
  accidental). The **Quit** menu item goes through the same graceful shutdown
  path as Ctrl+C (SIGINT stops dsh and saves its session).
- **Linux** — WebKitGTK windows cannot be intercepted, so after a close the
  launcher keeps running **without a window**; the tray icon
  (libayatana-appindicator3, loaded at runtime; without the library the
  feature degrades to close-to-exit) offers **Restore window** (rebuilds and
  re-navigates to dsh) and **Quit**.

Closing the window during startup (dsh not ready yet) still exits directly;
macOS has no tray support yet.

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
  - `browser` — external browser (Chrome/Edge/Firefox…), detected by tracking the
    browser window process and polling for its exit.
- dsh exits on its own → the launcher exits too.
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
  platform.rs    OS/arch/shell/paths/process helpers + dark mode / window theme
                + system tray (Windows/Linux), dshl single-instance mutex,
                screen-size & DPI probing
  version.rs     semver parse/compare
  process.rs     async child (streaming) + hidden-console spawn + job object
  wskeep.rs      keep-alive WebSocket (keeps the WebView window open)
  probe.rs       tool detection (node/bun/fnm/cargo/nvm/dsh)
  install.rs     installers + the fallback chain
  progress.rs    shared status state (UI-agnostic)
  flow/          the five startup flows
  ui/            webui.me window, vfs, bindings
assets/          startup page (index.html / app.js / styles.css)
               + full-resolution icon sources (dsh-black.svg / dsh-white.svg)
packing/       platform-specific installers + generated raster icons
  windows/       dshl.nsi + dsh.ico / dsh-white.ico
  linux/         build-deb.sh + 256/512px dsh*.png
  macos/         build-dmg.sh + 1024px dsh*.png (.icns built at package time)
```

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
```

## License

MIT.
