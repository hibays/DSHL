# DSHL — DeepSeek Harness web 启动器

> **简体中文** | [English](./README_en.md)

![_](docs/screenshot_1.webp)

DSHL 是一个轻量的原生启动器，以 [webui.me](https://webui.me) 作为启动 UI 包装，
在浏览器里启动 **DeepSeek Harness Web UI**（`dsh web`）。它会检查运行环境，必要时
安装 `@deepseek-ai/dsh`，在临时端口启动它，并把浏览器路由过去。

所有行为都通过 **`dshl.toml`** 配置；启动器基于**无依赖的原生 async**
（`std::future`）——无 tokio、无异步运行时。

## 亮点

- **Rust 编写 · 单文件便携** — 原生编译为**一个平台可执行文件**：无安装器、
  无捆绑运行时、无 GUI 框架。拷到哪都能跑（U 盘也行），`dshl.toml` 放在可执行
  文件旁（或平台配置目录）即可随身携带配置。
- **只需要浏览器** — 启动器自己的界面不依赖任何桌面框架：启动页由内嵌的本地
  web 服务器（webui.me）提供，用系统浏览器或内嵌 WebView 打开；机器上唯一
  的硬依赖是 Node.js（而 dsh 本身就需要它）。
- **webui.me 包装** — 轻量启动页（进度、日志、配置），dsh 启动后导航到 dsh（窗口保留，配合托盘可随时唤回）。
- **五个明确的启动流程**：
  1. 检查系统环境与架构
  2. 检查运行环境（node/bun，带回退链）
  3. 决定国内镜像（`auto-mirror`）
  4. 准备 dsh（`install` 或 `x` 模式）
  5. 启动 `dsh web` 并捕获其 URL
- **Node.js 回退链**（`node` 始终必需）：
  `fnm` → `cargo install fnm` → `nvm` → 自动安装 `fnm` 到 `~/.cache/bin`
  →（全部失败时）在 UI 中给出
  [fnm 安装指南](https://www.fnmnode.com/zh-cn/guide/install) 提示。
- **平台适配** — Windows（PowerShell）、Linux、macOS（bash）；dsh 以**隐藏控制台 +
  新进程组**启动（`CREATE_NEW_CONSOLE | CREATE_NEW_PROCESS_GROUP`，Unix 用
  `setsid`），路径与 shell 差异。Windows **release** 二进制是 GUI 程序（无控制台
  窗口），dsh 已安装时直接执行 `dsh`（`dsh.exe` / `dsh.cmd` / `dsh.sh`）并以隐藏
  控制台启动，不会弹出多余的 node 控制台。
- **应用图标** — `assets/` 下只有两份**全分辨率矢量源**：`dsh-black.svg`（黑色鲸鱼，
  同时**适配深色模式**——深色主题下自动变白）与 `dsh-white.svg`（白色夜间变体）；
  其余全部由 ffmpeg 从这两个源生成后放入 `packing/<平台>/`：`packing/windows/dsh.ico`
  （16/32/48/64/128/256 多尺寸）作为 Windows `.exe` 图标资源，`dsh-white.ico` 用于
  深色系统下窗口/任务栏/**托盘图标**的白色夜间变体（DPI 感知：按缩放比例选择最合适的源尺寸），Linux/macOS 的 png 供 deb/dmg 打包（见下文「图标」）。
- **完全可配置**的启动：包管理器、运行器、版本、flags、镜像。

## 要求

| 工具   | 版本          | 是否必需                                   |
|--------|---------------|--------------------------------------------|
| Node   | `>= 24.15.0`  | **始终**（缺失时安装 `26`）                |
| Bun    | `>= 1.3.14`   | 仅当 `pm = "bun"` / `exector = "bunx"` 时  |
| fnm    | 任意          | 首选 Node 版本管理器                       |
| cargo  | 任意          | 用于回退执行 `cargo install fnm`           |
| nvm    | 任意          | 最后的受管回退                             |

## 安装 / 构建

```sh
cargo build --release
```

二进制位于 `target/release/dshl`（Windows 为 `.exe`）。首次构建会自动下载对应
平台的预编译 WebUI 静态库。

每个打 tag 的 release 都会附带 Windows / Linux / macOS 的预编译二进制（见
`.github/workflows/release.yml`）。

## 使用

```sh
# 使用可执行文件旁或配置目录里的配置文件
./dshl

# 指定配置文件
./dshl --config ./dshl.toml
```

### 配置文件的搜索与生成

启动器按以下顺序查找 `dshl.toml`，**命中即用、不再继续**：

1. `--config <path>` 显式指定（文件不存在时直接报错提示）；
2. 当前工作目录 `./dshl.toml`；
3. 可执行文件所在目录 `<exe>/dshl.toml`；
4. 平台配置目录：
   - Windows：`%APPDATA%\dshl\dshl.toml`
   - macOS：`~/Library/Application Support/dshl/dshl.toml`
   - Linux：`$XDG_CONFIG_HOME/dshl/dshl.toml`（未设置时 `~/.config/dshl/dshl.toml`）；
5. Linux 系统级配置（最低优先级）：`/etc/dshl/dshl.toml`（发行包不携带配置，需手动放置）。

全部未命中时，启动器在**平台配置目录生成默认模板**（`dshl.example.toml` 的编译期
嵌入副本，注释齐全）并使用它——因此始终存在一个真实文件可编辑，启动页的
**打开配置**按钮打开的就是它。解析失败时使用内置默认值并在启动页提示错误。

## 调试日志

`./dshl --debug`（或 `-v`、或设置 `DSHL_LOG=1`）会把完整运行时时间线——每个流程
步骤和进程输出行——带单调时间戳镜像到 **stderr**。debug 构建保留控制台，所以
`cargo r -- --debug` 能实时看到；release 构建无控制台。

## 配置（`dshl.toml`）

```toml
# off = 不用镜像，on = 使用非空镜像（默认），force = 强制
auto-mirror = "on"

[mirrors]
npm            = "http://registry.npmmirror.com"     # bun 也走 npm registry
cargo          = "sparse+https://rsproxy.cn/index/"  # 临时使用，仅 CLI
nodejs-release = "https://mirrors.aliyun.com/nodejs-release/"
bun-download   = ""                                   # 空 = 不使用
github         = ""                                   # 代理前缀，如 https://ghproxy.com/

[dsh]
flags       = "--profile web --host 127.0.0.1 --port 0"
mode        = "install"   # install | x
pm          = "npm"       # npm | bun | pnpm  （install 模式）
exector     = "npx"       # npx | bunx | pnpx（x 模式）
version     = "latest"    # "latest" = 无后缀，否则 @deepseek-ai/dsh@<version>
auto-update = true        # 自动更新 @deepseek-ai/dsh（两种模式都生效）
single-instance = false   # true = 检测到其他 dsh 实例在运行时拒绝启动（防双写）

[ui]
mode    = "webview"   # webview | browser（是*偏好*，始终会回退）
close-to-tray = false # true = 关闭窗口时彻底关窗进托盘（dsh 后台运行），托盘图标重建窗口；Windows/Linux/macOS
single-instance = false # true = 只允许一个 dshl 实例；第二个实例改为激活已有实例（聚焦或唤出）
```

镜像地址为空表示**不使用**该镜像。镜像始终临时生效（环境变量 / CLI 标志），
从不写入任何全局配置。

### 模式

- `install` — 检查已安装的 `dsh`（及版本）；缺失或不匹配时用配置的 `pm` 安装
  `@deepseek-ai/dsh`（`bun add -g --ignore-scripts`、`npm i -g`、`pnpm add -g`），
  然后运行 `dsh`。
- `x` — 直接用 `npx` / `bunx` / `pnpx` 运行（npx/pnpx 使用 `--yes`）。目标使用
  **裸名 `dsh`**（锁定版本时为 `dsh@<version>`）：`bunx dsh` 会直接解析已安装的
  `dsh` 命令（如 bun 全局安装的 `~/.bun/bin/dsh`），不经过注册表，避免在
  `bunx @deepseek-ai/dsh` 下出现的 "Resolving dependencies" 卡死；需要下载时走
  配置的 npm 镜像。runner 启动失败且本机已安装 dsh 时，回退为直接执行 `dsh`。

### 自动更新

`auto-update`（默认 `true`）在未锁定版本（`version = "latest"`）时让
`@deepseek-ai/dsh` 保持最新：

- **install 模式** — 每次启动查询 registry 的最新版本，若已安装版本更旧则重装
  （最多 5 秒；离线时静默跳过）。
- **x 模式** — 运行器默认拉取最新；`auto-update = false` 时 npx/pnpx 使用
  `--prefer-offline`（缓存副本）。

`auto-update = false` 时，启动器仅在 dsh 缺失时安装，之后不再更新。锁定的
`version`（如 `1.2.3`）始终优先，不受 `auto-update` 影响。

### 单实例（可选）

默认每个 dshl 进程各自管理一个 dsh，允许多实例并存。把 `single-instance`
设为 `true` 后，启动前会检测机器上是否已有 dsh 在运行（手动启动或由其他
dshl 启动的都会命中）；检测到则**拒绝启动**并提示，防止两个进程写同一会话
日志造成永久损坏（"corrupt session log: seq gap"）。该检查发生在残留进程
清理之后，因此自己的旧 dsh 正常退出不算冲突。

### 关闭到托盘（可选）

默认关闭窗口即退出（并优雅停止 dsh）。把 `close-to-tray` 设为 `true` 后，
dsh 已启动时关闭窗口不再退出，dsh 继续在后台运行：

- **窗口会彻底关闭**（WebView2 / WebKitGTK 进程退出，释放内存），只有
  托盘图标、启动器和 dsh 常驻；
- **托盘图标在启动时即生成**（不等第一次关窗），并随系统深浅色主题切换黑白变体；
- **Windows**：托盘图标复用窗口图标。左键**双击**或菜单「恢复窗口」**重建**
  窗口（恢复上次的几何位置）并重新导航到 dsh；单击无动作（防误触）；右键
  菜单含「打开 dsh」（系统默认浏览器打开 dsh 页面）、「退出」（与 Ctrl+C
  相同的优雅关闭路径，SIGINT 停止 dsh 并保存会话）。
- **Linux**：WebKitGTK 窗口无法拦截关闭，关闭后启动器**无窗口继续运行**，
  托盘图标（libayatana-appindicator3，运行时动态加载；桌面没有该库时
  自动降级为关闭即退出）提供「恢复窗口」（重建窗口并重新导航到 dsh）、
  「打开 dsh」和「退出」。
- **macOS**：与 Linux 相同，WKWebView 窗口关闭后启动器无窗口继续运行，
  状态栏图标（`tray-icon` 的 AppKit 后端，主线程创建）提供「恢复窗口」、
  「打开 dsh」和「退出」。图标使用 **NSImage 模板**（黑色 + alpha 遮罩），
  系统自动按菜单栏深浅色渲染，无需手动切换图标变体；左键单击直接恢复窗口，
  右键弹出菜单。

启动阶段（dsh 尚未就绪）关闭窗口仍然直接退出。

### 单实例（dshl 互斥，可选）

`[dsh] single-instance` 限制的是 **dsh 本身**；`[ui] single-instance` 限制
的是 **dshl 启动器**：开启后同一台机器只允许一个 dshl 进程（锁文件 + 内核
文件锁，进程崩溃自动释放，无陈旧锁问题）。第二个 dshl 启动时不会创建新
窗口，而是**激活已有实例**后退出：

- 已有实例在托盘/无窗口状态 → 唤出窗口（重建并导航回 dsh）；
- 已有实例窗口可见 → 聚焦一次（Windows 多级策略：`SetForegroundWindow` →
  输入队列关联 → 模拟 Alt 键 → `SwitchToThisWindow`，即使第二实例已退出、
  授权失效也能把窗口带到前台；Linux 无通用窗口聚焦 API，仅托盘唤出）。

注意：由于第二个实例直接退出，请从启动器/快捷方式再次运行 dshl 来唤出
窗口（效果等同于"双击托盘图标"）。

## 如何启动 dsh

`dsh` 作为**受监督的子进程**启动（stdout/stderr 逐行流式写入
`~/.cache/dshl/dsh.log`，同时捕获 `http://127.0.0.1:<port>` 行）。然后启动器把
启动窗口路由到该 URL，并**作为 supervisor 常驻**，因此关闭始终干净：

- **关闭显示 dsh 的窗口** → 默认杀掉 dsh 并退出启动器；`close-to-tray`
  启用时（dsh 已启动）则**进托盘**，dsh 继续后台运行。两种窗口后端都支持：
  - `webview` — 内嵌 WebView（WebView2 / WKWebView / WebKitGTK）。启动器向自己的
    webui 服务端保持一个保活 WebSocket（`multi_client`），使窗口在跳转到 dsh 后仍
    保持打开；通过 webui 的 `set_close_handler_wv`（Windows 上还有窗口句柄）检测
    关闭。进程已设为 DPI 感知（`PerMonitorV2`），高分屏下 WebView 清晰不模糊。
  - `browser` — 外部浏览器（Chrome/Edge/Firefox…），通过跟踪浏览器窗口进程并轮询
    其退出来检测。
- dsh **正常退出**（exit 0，例如 Ctrl+C 后自存的优雅关闭）→ 启动器也随之退出。
- dsh **成功启动后意外退出**（非零退出码 / 被信号杀死）→ **崩溃恢复**：窗口跳回启动页，
  显示「dsh 意外退出（exit N）」横幅并**倒计时 5 秒**自动重启（立即重启 / 取消）；
  超时或点「立即重启」即重新走一遍完整启动流程并跳回 dsh 页；点「取消」则停在
  启动页，可手动「重试」或退出。窗口在托盘时崩溃也会先唤出窗口再倒计时。
- Ctrl+C / SIGTERM → 杀掉 dsh 后退出。
- dsh 会被**优雅停止**（Windows 通过隐藏控制台 + `GenerateConsoleCtrlEvent` 发
  Ctrl+C，Unix 发 SIGTERM），等待其自行退出（重试路径 10 秒 / 退出路径 30 秒
  宽限，dsh 自身最多 5 秒即响应）。**从不自动强杀**：宽限超时后本次启动取消，
  由用户在启动页确认后才会「强制结束残留进程」（`taskkill /F /T` / `SIGKILL`），
  防止双进程写同一 session 日志造成永久损坏。
- 启动器被强杀（Windows `TerminateProcess`）→ dsh 由 kill-on-close
  **Job Object** 回收（Linux 用 `PR_SET_PDEATHSIG`）。
- 启动前关闭启动窗口，或点 **退出** 按钮 → 优雅停止 dsh（Ctrl+C/SIGTERM）后退出。

## 项目结构

```
src/
  main.rs        二进制入口
  lib.rs         模块注册表
  runtime.rs     极简原生 async 执行器（无 tokio）
  config.rs      dshl.toml 模型 + 发现
  mirror.rs      镜像解析（临时，从不持久化）
  platform/      OS 原语（松耦合拆分，见下方「架构说明」）
    mod.rs       门面：re-export 各子模块，调用方仍写 crate::platform::…
    detect.rs    OS/架构/shell 探测
    paths.rs     目录发现 + 可执行文件查找（which/tool/known_tool_dirs）
    actions.rs   OS 动作（文件管理器打开文件）
    process.rs   进程存活/树杀/进程发现
    dpi.rs       DPI 感知与缩放（Win32 + X11 dlopen）
    theme.rs     深色模式检测 + 窗口主题（Win32）
    window.rs    Win32 窗口几何/聚焦/发现
    single_instance.rs  dshl 单实例互斥（锁文件 + 激活信号）
  tray/          系统托盘（每个平台一个实现，共用 6 函数接口）
    windows.rs   隐藏消息窗口 + Shell_NotifyIconW（全部走 windows-rs）
    linux.rs     libayatana-appindicator3 运行时 dlopen（无硬依赖）
    macos.rs     tray-icon（AppKit 后端，NSImage 模板图标）
  version.rs     语义化版本解析/比较
  process/       进程助手（松耦合拆分）
    mod.rs       门面：re-export（run / with_env / AsyncChild / Output …）
    capture.rs   同步捕获（run）+ 命令准备（with_env / prepare_spawn）
    child.rs     AsyncChild：流式行队列 + 收尸线程 + waker + 优雅停止
    win_proc.rs  (Windows) CreateProcessW 隐藏控制台 + Ctrl+C 优雅停（windows-rs）
    win_job.rs   (Windows) kill-on-close Job Object（windows-rs）
  wskeep.rs      保活 WebSocket（让 WebView 窗口保持打开）
  probe.rs       工具探测（node/bun/fnm/cargo/nvm/dsh）
  install/       运行时安装器 + 回退链（按运行时拆分）
    mod.rs       门面：常量（NODE_MIN/BUN_MIN…）+ re-export
    node.rs      ensure_node + fnm→cargo→nvm→fnm 自动安装 回退链
    bun.rs       ensure_bun + 直连下载→官方脚本→npm 回退链
    pnpm.rs      ensure_pnpm + 全局 bin 目录解析
    download.rs  zip 下载/解压 + fnm 二进制下载 + github 代理前缀
    stream.rs    run_streaming（命令输出流式进进度日志）
    runtime.rs   Runtime 模型（PATH 前缀）
  progress.rs    共享状态（与 UI 解耦）
  flow/          五个启动流程
  ui/            webui.me 启动窗口（按职责拆分，见下方「架构说明」）
    mod.rs       门面：setup / launch_flow / run_loop / request_shutdown
    state.rs     全部共享原子状态（模块间只经它协调）
    vfs.rs       虚拟文件处理器（内嵌启动页）
    bindings.rs  页面绑定函数（get_state / retry / force_kill_stale …）
    window.rs    窗口生命周期：创建/关闭/几何持久化/主题/句柄追踪/托盘恢复
    launch.rs    启动流程：残留清理 → 配置 → flow::run → 导航 → 监督
    supervisor.rs 主事件循环 + 窗口消失检测 + 托盘/单实例请求
assets/          启动页（index.html / app.js / styles.css）
                + 全分辨率图标源（dsh-black.svg / dsh-white.svg）
packing/       平台特化安装包与生成的光栅图标（ffmpeg 产物）
  windows/       dshl.nsi + dsh.ico / dsh-white.ico
  linux/         build-deb.sh + 256/512px 的 dsh*.png
  macos/         build-dmg.sh + 1024px dsh*.png（打包时生成 .icns）
                + tray-black.rgba（32×32 状态栏模板图标，原始 RGBA）
```

### 架构说明（松耦合设计）

- **`platform/` 只做 OS 原语**：检测、路径、进程、DPI、主题、窗口助手、单实例
  互斥。每个子模块一个职责，`mod.rs` 是薄门面（re-export），因此其余代码的
  `crate::platform::…` 调用完全不变。
- **Windows API 全部走 `windows-rs 0.62`**（`windows` crate）：不再有任何手写
  `#[link] extern "system"` FFI 块。需要哪个模块就开哪个 feature
  （`Win32_UI_WindowsAndMessaging`、`Win32_UI_HiDpi`、`Win32_UI_Shell`、
  `Win32_Graphics_Dwm`、`Win32_System_Registry`、`Win32_System_Threading` …）。
- **`tray/` 与 UI 解耦**：三套平台实现共用同一个 6 函数接口（`start` /
  `hide_to_tray` / `quit_requested` / `restore_requested` / `set_icon` /
  `shutdown`），事件都以原子标志的形式暴露，`ui/supervisor.rs` 只轮询标志、
  不感知平台细节。macOS 因 AppKit 必须主线程创建状态栏图标，`start()` 只记
  意图，真正创建推迟到主线程轮询时（见 `tray/macos.rs` 顶部设计说明）。
- **`ui/` 按职责拆分**：全部共享状态集中在 `state.rs`（模块间不互相摸内部），
  `window.rs` 管窗口本身，`launch.rs` 管启动流程，`supervisor.rs` 管事件循环，
  `bindings.rs` 是页面的唯一入口，`mod.rs` 只做 re-export 门面。
- **`process/` 按职责拆分**：`capture.rs` 管同步捕获与命令准备，`child.rs` 管
  `AsyncChild` 流式行队列与优雅停止，Windows 专属的隐藏控制台 spawn /
  Ctrl+C（`win_proc.rs`）和 kill-on-close Job Object（`win_job.rs`）单独成文件，
  `mod.rs` 只 re-export 公共 API，外部调用零改动。
- **`install/` 按运行时拆分**：node / bun / pnpm 三个安装器各占一个文件，
  公共的 zip 下载、fnm 二进制下载、github 代理前缀在 `download.rs`，
  `Runtime` 模型与 `run_streaming` 各自独立，`mod.rs` 只 re-export 公共 API
  （`ensure_node` / `ensure_bun` / `ensure_pnpm` / `Runtime` / `run_streaming`），
  外部调用零改动。

## 图标

`assets/` 下只有两份**全分辨率矢量源**（1024×1024 声明尺寸，向量渲染，缩放不糊）：

- `dsh-black.svg` — 黑色鲸鱼；自带 `prefers-color-scheme: dark` 适配，页面/标签页
  图标在深色主题下自动变白。
- `dsh-white.svg` — 强制白色的夜间变体（深色任务栏/窗口/托盘图标）。

其余所有图标都由 ffmpeg（librsvg）从这两个源生成，生成后放入 `packing/<平台>/`：

```sh
# Windows 多尺寸 .ico（16/32/48/64/128/256）——黑色默认 / 白色夜间
ffmpeg -hide_banner -y -i assets/dsh-black.svg \
  -filter_complex "split=6[a][b][c][d][e][f];[a]scale=16:16:flags=lanczos[b];\
[b]scale=32:32:flags=lanczos[c];[c]scale=48:48:flags=lanczos[d];\
[d]scale=64:64:flags=lanczos[e];[e]scale=128:128:flags=lanczos[f];\
[f]scale=256:256:flags=lanczos[g]" \
  -map "[b]" -map "[c]" -map "[d]" -map "[e]" -map "[f]" -map "[g]" \
  -c:v bmp packing/windows/dsh.ico
# 白色变体：输入换 assets/dsh-white.svg → packing/windows/dsh-white.ico

# Linux PNG（256/512px）与 macOS PNG（1024px，打包时用 sips+iconutil 生成 .icns）
ffmpeg -hide_banner -y -i assets/dsh-black.svg -vf "scale=256:256:flags=lanczos" -frames:v 1 packing/linux/dsh.png
ffmpeg -hide_banner -y -i assets/dsh-black.svg -vf "scale=512:512:flags=lanczos" -frames:v 1 packing/linux/dsh-512.png
ffmpeg -hide_banner -y -i assets/dsh-black.svg -frames:v 1 packing/macos/dsh.png
# 白色变体把输入换为 assets/dsh-white.svg，输出 dsh-white*.png

# macOS 状态栏模板图标（32×32 原始 RGBA，编译期嵌入；黑色 + alpha 遮罩，
# 系统自动按菜单栏深浅色渲染）
ffmpeg -hide_banner -y -i assets/dsh-black.svg -pix_fmt rgba -f rawvideo -s 32x32 packing/macos/tray-black.rgba
```

## License

MIT.
