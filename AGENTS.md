# Repository Guidelines

## Project Overview

DSHL（DeepSeek Harness Launcher）——一个 webui.me 包装启动器：检查运行环境 → 安装/解析 `@deepseek-ai/dsh` → 启动 `dsh web` → 路由浏览器到它的 URL。根 crate 名是 **`dshl-core`**（源码在仓库根 `src/`，纯 lib）。同一内核两条交付轨道：

- **Track A 安装器**：`crates/dshl`（exe；打包产物 NSIS/deb/dmg，由 `release.yml` 发布）。
- **Track B 插件轨**：`crates/dshl-native`（napi-rs cdylib `.node`）+ `plugins/*` JS 包（`release-native.yml` 发 6 个 `@dshl/native-<platform>-<arch>` 子包；`release-plugins.yml` 发聚合包 `@dshl/native`、`@dshl/pipe`、`@dshl/control`；发布步骤缺 `NPM_TOKEN` secret 时自跳过——no-op 不报错）。

助手铁律：

- **用中文回复**。
- **不要动 git**：不 commit/add/stage、不改 .git 状态。用户手动管理暂存区以便自行审查。
- **改 `t!()` 消息必须同步 `locales/en.yml` 与 `locales/zh-CN.yml`**（rust-i18n，fallback = zh-CN；缺失键退化为中文而非空串）。
- **Windows 系统 API 一律走 `windows-rs 0.62`**（按模块开 feature），不写手写 `#[link] extern "system"` FFI。唯一例外：`crates/dshl-native` 的 napi-rs。
- dsh 的安装/更新**绝不 `-g`**（不污染用户环境）：装进 dshl 缓存，运行时把 bin 目录注入 dsh 进程 PATH。
- 镜像（mirror）永远临时生效（env / CLI 参数），从不写入全局配置。

## Architecture & Data Flow

**入口链**（A/B 两轨仅入口不同，内核同一份）：

- Track A：`crates/dshl/src/main.rs` → `dshl_cli::run_cli()`。壳只做三件事：转发 windows_subsystem 属性、调 run_cli（其内部从不 `process::exit`）、把 `RunOutcome`（HelpPrinted/VersionPrinted/ArgsError/AlreadyRunning）映射为退出码。
- Track B：`crates/dshl-native` 的 `#[napi] launch(...)` → `dshl_cli::run_with_options(...)`。
- 共用参数层 `crates/dshl-cli`：run / control / signal / handle 子命令。

**启动管线**（`src/flow/`，顺序执行）：`system`（OS/arch 检测）→ `runtime_env`（node/bun/pnpm/fnm 探测与就绪）→ `mirror_check`（镜像策略解析）→ `prepare`（安装/解析 dsh，`prepare::run()` 返回 `Command`）→ `launch`（捕获 stdout 里的 URL 并转入监督）。

**dsh 来源策略**（`src/config.rs` `DshMode`）：`global`（严格全局，缺失报错）/ `hybrid`（默认：全局优先，缺失或版本不符落缓存）/ `private`（恒装缓存、不碰全局）。缓存位置 `flow/prepare.rs::dsh_dir()` = `<cache>/dshl`，dsh 是其中的 node module（`node_modules/@deepseek-ai/dsh`），以 `node <bin 入口>` 直接运行（从 package.json 解析 bin）。包管理器可配 npm / bun / pnpm。

**控制面**（`src/control.rs`）：`DSHL_CONTROL_URL=dshl://<token>@127.0.0.1:<port>` 注入 dsh 进程环境；方法 `ping | shutdown | switch-profile | open-terminal | restart`。插件轨 `open-terminal` 优先本地 addon、回落管道；restart/shutdown 对空 client 有防护。

**进程监督**（`src/process/`）：dsh 是受监督子进程，stdout/stderr 逐行进 `<cache>/dshl/dsh.log`；优雅停止（Windows 隐藏控制台 + `GenerateConsoleCtrlEvent` 发 Ctrl+C，Unix SIGTERM），**从不自动强杀**；强杀由 Job Object / PDEATHSIG 兜底。崩溃恢复：5s 倒计时自动重启。

- **UI 分层**（`src/ui/`）：`state.rs` 集中共享状态；`window` / `launch` / `supervisor` / `bindings` / `crash` / `exit` / `vfs` / `geometry`（窗口几何持久化：`<cache>/dshl/window-state.json` + webui 硬限 clamp）各司其职；webui.me 保活 WebSocket 在 `src/wskeep.rs`。

**内嵌终端**（`src/pty/`）：portable-pty + tokio-tungstenite 内嵌独立 WS 服务，页面端 xterm.js（token 经 URL query 校验）。

## Key Directories

| 目录 | 用途 |
|---|---|
| `src/` | dshl-core 内核（见 Architecture） |
| `src/platform/` | OS 原语：detect / paths / process / dpi / theme / window / single_instance / actions |
| `src/tray/` | 托盘图标，每 OS 一个实现（windows / macos / linux），统一 7 函数契约（start / hide_to_tray / quit_requested / restore_requested / open_url_requested / set_icon / shutdown）+ `is_started` 查询 |
| `src/install/` | node/bun/pnpm 安装管线（fnm 兜底链在 `node.rs` / `download.rs` 内）+ download + stream 输出泵 |
| `src/ui/geometry.rs` | 窗口几何持久化（WebView 与外置浏览器共用一份 `window-state.json`，物理像素 + webui 硬限 clamp） |
| `src/testutil.rs` | 测试专用助手：按 OS 选 shell 的 `shell()`（Windows `%COMSPEC%`/cmd，Unix `sh`），供子进程类测试共用 |
| `src/version.rs`, `src/probe.rs`, `src/mirror.rs` | 版本解析与预发布比较、工具探测（`Tool`）、镜像决策 |
| `crates/dshl/` | Track A exe 壳；`build.rs` 用 winresource 嵌 Windows 图标 |
| `crates/dshl-cli/` | CLI 参数层（两轨共用） |
| `crates/dshl-native/` | napi-rs cdylib（crate-type 仅 cdylib） |
| `plugins/dshl-native` | 加载 `.node` addon → Cordis 服务 `dshlNativeBackend` |
| `plugins/dshl-pipe` | 连接运行中 dshl 的控制管道（DSHL_CONTROL_URL）→ `dshlPipeBackend` |
| `plugins/dshl-control` | 顶层聚合：native/pipe 折叠成统一 `nativeCapabilities` 服务 + HTTP 路由 + plugin-guard |
| `plugins/dshl-control/src/backend-contract.js` | native / pipe 两档 backend 的能力契约（`TIERS` 清单；消费方折叠进 `nativeCapabilities` 前校验） |
| `assets/` | 前端三件套 `index.html` / `app.js` / `styles.css`（+ 字体、svg） |
| `packing/` | `windows/dshl.nsi`、`macos/build-dmg.sh`、`linux/build-deb.sh` |
| `.github/workflows/` | `ci.yml`、`release.yml`、`release-native.yml`、`release-plugins.yml` |

各插件包带 `cordis.patch.yml`，供 dsh bundle 时打补丁接线。

## Development Commands

```sh
cargo build --workspace                        # 产出 target/debug/dshl.exe（沙箱用；改完源码必须先重 build）
cargo clippy --workspace --all-targets -- -D warnings   # CI 同款门禁
cargo fmt --all -- --check
cargo test --workspace --locked
cargo test -p dshl-core --lib install::stream  # 跑单个测试模块
node plugins/dshl-native/scripts/build-native.mjs [--release]  # 本地 .node → plugins/dshl-native/native/（gitignored）
npm run check                                  # node --check 全部插件 JS
npm pack --workspaces --dry-run                # JS 包打包校验

# 上述门禁的脚本化封装（CI ci.yml 直接调用，本地与 CI 单源）：
scripts/gate.ps1 [-Rust|-Js]                   # Windows（gate.bat 转发壳）
scripts/gate.sh [--rust|--js]                  # Linux/macOS 等价实现
# 打包与发布（镜像 release*.yml 的单机子集）：
scripts/package.ps1 [--NoInstaller]              # Track A 本机全流程（Windows）
scripts/package.sh all|stage|portable|nsis|deb|dmg  # Track A 分步子命令（release.yml 同款调用）
scripts/publish.ps1|publish.sh -Version x.y.z [-DryRun]  # Track B npm 发布（native→pipe→control）
```

- `cargo clippy` / `cargo test` **不会更新 `target/debug/dshl.exe`**——跑沙箱前先 `cargo build --workspace`。
- `webui` 是 **git 依赖**（fork `hibays/rust-webui`，build script 拉 C 库），首次构建需联网。
- Windows 本地沙箱循环（本机辅助脚本，未入库）：设置 `DSHL_CACHE` / `BUN_INSTALL` / `NPM_CONFIG_PREFIX` 等环境变量，把沙箱 bin 前插 PATH 后前台跑 `dshl -d`。它是**前台阻塞进程**：用 `Start-Process -WindowStyle Hidden` 分离启动，再轮询 `stderr.log`（启动时间线）与 `cache\dshl\dsh.log`（dsh web URL），不要同步等待。沙箱 `dshl.toml` 控制 `mode` / `version`：PATH 前缀自带 dsh rc.6 时可测 hybrid 命中，改 mode 可测 hybrid 兜底 vs private 缓存安装。

## Code Conventions & Common Patterns

- **错误处理**：零依赖自制类型 `error::Error(pub String)` + `type Result<T>` + `bail!` 宏（`src/error.rs`）。不用 anyhow/thiserror；面向用户的消息走 i18n 键而非裸字符串拼接。
- **i18n**：`#[macro_use] extern crate rust_i18n` 使 `t!("key", var = val)` 全 crate 可用；键按域前缀分组（`tray.*`、`page.*`、`flow.steps.*`、`flow.prepare.*`、`ui.*`…）；`i18n!("locales", fallback = "zh-CN")` 在 `src/lib.rs` 装载。新增/修改键必须同时落 `locales/en.yml` 与 `locales/zh-CN.yml`。
- **异步**：全局多线程 tokio runtime 的薄封装在 `src/runtime.rs`（`block_on` / `spawn`）。业务代码经它进入异步；测试里也用 `crate::runtime::block_on(...)` 而非 `#[tokio::test]`。
- **平台分支**：按 OS 拆子模块（`platform/`、`tray/{windows,macos,linux}`、`process/win_{proc,job}.rs`），条件依赖放 `[target.'cfg(...)'.dependencies]`；Windows API 只经 windows-rs 0.62 feature 门控。
- **状态管理**：跨模块可变状态用全局静态（如 `lib.rs` 的 `DSH_CHILD: LazyLock<Mutex<Option<Arc<AsyncChild>>>>`）；UI 态集中在 `ui/state.rs`，不做散落的 Arc<Mutex> 字段传递。
- **结构约定**：一模块一职责，小写单文件或 `目录 + mod.rs`；测试内联在文件尾 `#[cfg(test)] mod tests`；文档注释解释「为什么」并常含平台坑位说明。
- **前端**（改 `assets/` 前必读 `DESIGN.md`）：瑞士国际风格——唯一超蓝 `#2f6fe4`（hover `#2258c4`）、全部方角（rounded none）无阴影、mono 字体仅用于日志/数据、light/dark 两套自写色板、禁 emoji 与图标字体。

## Important Files

- `src/lib.rs` — crate 模块地图、i18n 装载、`DSH_CHILD` 全局态。
- `src/config.rs` — `Config` / `MirrorMode(off|on|force)` / `DshMode(global|hybrid|private)` / `Pm(npm|bun|pnpm)` / `Ui`；配置面见 `dshl.example.toml`（每字段可选、每次启动重读）。
- `src/flow/prepare.rs` — `dsh_dir()` / `install_dsh()` / 版本匹配与更新决策（dsh 缓存布局的事实来源）。
- `src/control.rs` — 控制面协议与分发（含大部分单元测试）。
- `src/runtime.rs`、`src/error.rs`、`src/i18n.rs` — 三大横切基础设施（异步、错误、本地化）。
- `Cargo.toml`（workspace 根：members、依赖、feature、release profile）、`crates/*/Cargo.toml`。
- `package.json`（npm workspace 根：`plugins/*`、scripts、engines node>=22）。
- `.github/workflows/ci.yml` — 门禁定义（fmt/clippy/test/node --check/pack dry-run）。

## Runtime/Tooling Preferences

- Rust：edition 2024、resolver 3、workspace 统一版本（当前 0.2.0）；release profile `opt-level="z"` + lto + strip。
- tokio 仅开实际用到的 feature（rt-multi-thread/macros/process/time/io-util/sync/net）——加异步能力时先扩 feature 再用。
- JS 侧：Node >=22、全 ESM（`"type": "module"`）、npm workspaces（`plugins/*`）；验证与打包走根 `package.json` scripts（`check` / `pack:dry` / `publish:all` / `build:native`）。
- 环境隔离原则：dsh 装进 dshl 缓存并注入子进程 PATH，绝不全局安装；镜像只经 env / CLI 参数临时注入（npm/cargo/nodejs-release/bun-download/github 五路，见 `dshl.example.toml`）。
- CI 矩阵：Rust 在 ubuntu-latest + windows-11-arm（aarch64-pc-windows-msvc，抓 ARM64 回归）双跑；Linux 需 gtk-3 / webkit2gtk-4.1 / ayatana-appindicator 系统包；JS job 单独跑语法检查与 pack 校验。

## Testing & QA

- **形态**：全部为源码文件尾内联 `#[test]`（无独立 `tests/` 目录、无 `#[tokio::test]`）；异步断言统一 `crate::runtime::block_on(async { ... })`。
- **分布**：`control.rs`（方法分发、hello token 校验、profile flags 注入、线上 roundtrip）、`version.rs`（解析/排序/预发布）、`flow/launch.rs`（`stream_until_url` 时序与崩溃路径、`supervise` 零退出清理）、`flow/prepare.rs`（`split_args` 引号处理、版本决策）、`install/bun.rs`（镜像 URL 决策）、`install/stream.rs`、`process/child.rs`（管道 drain、孙进程占住写端）、`platform/process.rs`、`pty/mod.rs`（token 生成、URL query 解析）、`pty/server.rs`（WS 帧分类：text 永非控制帧、binary JSON 为控制、其余为 shell 输入）、`ui/geometry.rs`（persist/load roundtrip、退化尺寸拒绝、webui 硬限 clamp）。
- **时序测试预算**：`install/stream.rs::verbose_output_drain_completes_promptly` 是 AsyncChild `next_line` lost-wakeup 竞态回归测试，5s 预算以「无逐行延迟的生产者」为前提——powershell 分支已去掉 Start-Sleep（windows-11-arm 上每拍 ~35ms 会爆预算）。**不要重新引入逐行 sleep 或大幅加行数**；`flow/launch.rs` 多个 8s 上限断言同理勿放宽。
- **编写约束**：临时目录一律 `std::env::temp_dir()`，不写死平台路径；不引入依赖网络 / 真实 bun / 硬编码沙箱路径的测试（历史教训：bun add 回归测试因 Windows junction 复制 `PermissionDenied` 改成通用命令）。子进程类测试 Windows 用 `%COMSPEC%`/cmd，非 Windows 用 `sh`；`flow/launch.rs` 测试统一经 `src/testutil.rs::shell()` 取平台命令。
- **JS 侧无测试框架**：验证链就是 `npm run check` + `npm pack --workspaces --dry-run`。
- 沙箱 stderr 里的中文经部分工具读取会显示 GBK 乱码——纯显示问题，不影响判断。

## Agent 工作流

- `.agents/skills/`：可复用流程（代码审查检查点、发布演练），agent 遇到对应任务
  应主动加载对应 `SKILL.md`。
- `.agents/notes/`：设计决策记录（ADR）——锁中毒策略边界、CLI_LOCK 生命周期、
  可选 backend seam 等已权衡方案的动机与验证方式；与 note 冲突先当设计讨论。
- `plugins/AGENTS.md`：JS 插件三角色约定（可选服务 ctx.get 习语、契约同步、双语 T()）。
