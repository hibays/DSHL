# DSHL 审查报告（五项专题）

- 日期：2026-09-22
- 审查对象：`G:\Projects\dsh-launcher`，HEAD `a0ae6d1`（2026-09-12），工作区干净
- 审查方式：源码通读 + 本仓门禁实跑 + **对真实 registry / 真实缓存 / 真实运行进程取证** + 一个针对性的 Rust 语义实验
- 纪律：只读。未修改任何仓库文件（本报告除外），未执行任何 git 写操作

## 0. 证据等级说明

| 标记 | 含义 |
|---|---|
| 【实测】 | 本次在真机上跑出来的结果（命令与输出见附录 A） |
| 【代码】 | 源码可直接指认 file:line |
| 【推断】 | 由代码语义 + 外部事实推导，未端到端跑通 |
| 【推测】 | 明确未验证，仅作方向提示 |

门禁实跑（本机 Windows）：`cargo clippy --workspace --all-targets -- -D warnings` 退出 0；`cargo test --workspace --locked` **63 passed / 0 failed**。→ **仓库当前是绿的**，下述问题全部是"设计/覆盖"层面的，不是编译或既有测试失败。

---

## 1. 结论速览

| # | 问题 | 严重度 | 类型 | 证据 |
|---|---|---|---|---|
| B1 | 默认 `pm = "nub"` 在**没有全局 nub** 的机器上必然失败：registry URL 少了 `@` | **P0 阻断** | 拼串 bug | `src/install/nub.rs:106`；实测 npmjs 405 / npmmirror 422 |
| B2 | 即使下载成功，**缓存里的 nub 也无法启动**：Windows 走 `nub.cmd`（缓存里根本没有 `.cmd`），Unix 主包 JS shim 覆盖原生二进制且 `require("../platform.js")` 缺失 | **P0 阻断** | 解析/布局 bug | `nub.rs:122-128`、`platform/paths.rs:83-91`、`prepare.rs:349-355`；主包 tarball 实测 |
| B3 | 版本比较策略不一致会让缓存被**静默降级**，而降级正好撞上运行中 dsh 的文件占用 → **每次启动都重试、每次都失败** | **P0 高** | 策略不一致 | `prepare.rs:183`（`>=`）vs `269-279`（`!=`）；本机缓存 0.1.6-alpha.2 vs registry latest 0.1.5-rc.2【实测】 |
| B4 | 「被 lock」的真正机制不是 dshl 加锁，而是 **Windows 文件占用 + 单实例静默退出 + 安装无进度无自愈** 三者叠加 | **P1 高** | 诊断 | `win_job.rs`、`single_instance.rs:47`、`main.rs:31-35`、`prepare.rs:555-563`；占用实测复现 |
| B5 | 半成品缓存**无自愈**：`cache_needs_install` 只看 `package.json` 里的版本串，入口缺失时永久 `entry_missing` | **P1 高** | 判定过弱 | `prepare.rs:255-259/269-279/555-563` |
| B6 | `version` 不支持 dist-tag / 版本区间：`version = "next"` 永远拿不到更新（且不报错） | P2 中 | 能力缺口 | `prepare.rs:107-118/269-279` |
| B7 | 更新检查是"尽力而为 + 3s + 静默"：失败即当作 latest，用户看不到任何状态；长驻启动器永不复查 | P2 中 | 可观测性 | `prepare.rs:370-401/459-465`；`control.rs:256-295` 无 update 方法 |
| B8 | 每次启动对同一个 dsh 外壳跑**两次** `--version`（probe + stale-shim gate），纯启动开销 | P2 中 | 性能 | `prepare.rs:121-126` + `409-420`、`502-516` |
| B9 | 前端配置面板的 `auto-mirror` 一行**恒为空**（serde 序列化成 `auto-mirror`，前端读 `auto_mirror`） | P2 低 | 前端 bug | `config.rs:196`、`ui/launch.rs:39`、`assets/app.js:100` |
| B10 | 文档漂移：AGENTS.md 的缓存路径、`prepare.rs:366` 的 "5-second cap"、README 对 nub 的能力声明 | P3 低 | 契约 | `AGENTS.md:29`、`prepare.rs:203/366`、`README.md:88` |
| B11 | macOS 既无 Job Object 也无 PDEATHSIG → 关掉启动器后**安装子进程可继续存活**（Windows/Linux 都有兜底） | P2 中（仅 macOS） | 平台缺口 | `process/capture.rs:38-56`（`not(any(windows, linux))` 分支是空操作） |
| B12 | **版本三源不一致**：最新 tag `v0.2.22`、Cargo workspace `0.2.0`、npm 包 `0.1.0`；`dshl --version` 与 `ping` 报的都是 0.2.0 | P3 低 | 发布卫生 | 【实测】`git describe` = `v0.2.22`、`target/debug/dshl.exe --version` = `dshl 0.2.0`；`main.rs:24`、`src/control.rs:260`、`crates/dshl-native/src/platform.rs:18`；`scripts/bump-versions.mjs` 只改 JS 侧 |
| B13 | **探测类子进程其实没有超时**：`probe.rs:44-73` 直接 `run_async`，而 AGENTS.md/README 承诺 "probe 30s"；一个挂死的 `--version` 会让启动管线永久停在"探测中" | P1 中-高 | 契约未实现 | `probe.rs:44-73`、`process/capture.rs:90-97`；承诺见 `AGENTS.md:117`、`README.md:133-134` |
| B14 | **`platform::tool()` 的 `.cmd` 兜底只对 npm 风格 shim 成立**：工具只有 dshl 缓存里那一份时，`bun/nub` 会被解析成不存在的 `bun.cmd`/`nub.cmd` | P1 高 | 解析 bug | `paths.rs:83-91` + `prepare.rs:332/350`；`.cmd` 缺失已实测（附 A.2） |
| B15 | **`MirrorMode::Force` 是空实现**：`MirrorConfig::forced()`（`mirror.rs:46-48`）全仓无调用者；`On` 承诺的"失败回退原始源"也不存在；bun 的官方脚本路线在 Force 下仍直连 `bun.sh` | P2 中 | 配置项无实效 | `mirror.rs:46-48`（grep 实证无调用者）、`config.rs:13-14/17-18`、`bun.rs:109-127` |
| B16 | **`probe::nvm()` 的 Windows 分支绕过退出码门**（正是 skill 点名的"挖版本号"回归类）：非零退出仍 `found=true` 并从 stdout 取版本 | P2 中 | 回归风险 | `probe.rs:141-160` vs `probe.rs:83-102` 与其回归测试 |

一句话总结：**"dsh 版本检查"的策略不一致（B3/B6/B7）与"本地安装被 lock"的真实成因（B1/B2/B4/B5）是同一处代码区域的两个面** —— 都在 `flow/prepare.rs` + `install/` 这条"解析版本 → 决定装不装 → 调 pm 装"的短链路上。

---

## 2. 专题一：dsh 版本检查的模式

### 2.1 现状链路（一次启动内的完整决策）

```
ui/launch.rs:36        config::load()           —— 配置每次启动重读
  ↓
flow/prepare.rs:454-470 目标版本解析
  ├ 固定版本                     → target = config.dsh.version（零网络）
  ├ latest + auto-update=true    → query_latest_version()（3s 有界，失败 → "latest"）
  └ latest + auto-update=false   → target = "latest"
  ↓ spec = "@deepseek-ai/dsh[@<target>]"
flow/prepare.rs:473-490 来源决策（config.dsh.mode）
  ├ private → 直接走缓存
  ├ global  → probe 全局，缺失即报错
  └ hybrid  → hybrid_use_global()（141-201）：
        全局缺失            → 缓存
        固定版本            → 全局必须版本**完全相等**（==）
        latest + 不更新     → 用全局现状
        latest + auto-update → 全局 `installed >= latest` 才用，否则落缓存
  ↓
flow/prepare.rs:502-516 stale-shim 闸门：global_program_usable() 再跑一次 `dsh --version`（15s）
  ↓
flow/prepare.rs:529-552 缓存判定：cached_dsh_version(package.json) → cache_needs_install()
  ↓
flow/prepare.rs:284-360 install_dsh()：按 pm 拼命令，装进 <cache>/dshl/dsh
  ↓
flow/prepare.rs:555-563 package_entry()：从 package.json 的 bin 解析入口（严格，无兜底）
  ↓
flow/prepare.rs:564-568 以 `node <entry> <flags>` 启动
```

### 2.2 问题

#### V1（=B3）缓存与全局的版本比较策略不一致 → 静默降级

- 全局：`installed >= latest`（`prepare.rs:182-190`）——**永不降级**。
- 缓存：`*v != wanted`（`prepare.rs:273-278`）——**只要不相等就重装**，包括"缓存比 registry 的 latest 更新"。

【实测】本机当前状态正好命中：registry（原站与 npmmirror 一致）`dist-tags = {latest: 0.1.5-rc.2, alpha: 0.1.6-alpha.2, next: 0.1.5-rc.2}`，而用户缓存里装的是 **0.1.6-alpha.2**（`~/.cache/dshl/dsh/node_modules/@deepseek-ai/dsh/package.json`）。用户 `dshl.toml` 是 `version = "latest"` + `auto-update = true` → 目标 `0.1.5-rc.2` → `cache_needs_install(0.1.6-alpha.2, "0.1.5-rc.2") = true` → **每次启动都判定要安装，并执行一次"降级"重装**。

要紧的是**失败不会改变判定输入**：降级安装失败后缓存仍是 0.1.6-alpha.2，下次启动仍然判定"需要安装" → **同一个冲突每次启动复现**。这就是"安装/更新被 lock"里"反复被锁"的那一半。

#### V2（=B6）dist-tag / 区间不被支持，而且失败是静默的

`version = "next"`（或 `alpha`）时：
- `dsh_version_ok()`（`prepare.rs:107-118`）解析不出 `next` → `return true`——**任何**全局 dsh 都被放行；
- `cache_needs_install()`（`prepare.rs:273-277`）解析不出 target → `return false`——**永远保留缓存**；
- 结果：空缓存时能装上 `@deepseek-ai/dsh@next` 那一次解析到的版本，之后**再也不会更新**，因为 tag 无法与具体版本比较。

这对"启动器式选版本"是硬伤：PCL/HMCL 式的渠道（稳定/预览）选择天然是 dist-tag 语义。

#### V3（=B7）更新检查不可观测、不可按需

- `query_latest_version`（`prepare.rs:370-401`）：3s 超时 + 任何非零退出 → `None` → `target = "latest"` → 有缓存就不动。**用户看不到"检查更新失败"**，也看不到"当前已是最新/有更新"。
- 只在启动时检查一次；托盘常驻数天的启动器不会复查。
- 控制面（`control.rs:256-295`）只有 `ping | shutdown | switch-profile | open-terminal | restart`，**没有 `check-update` / `update` / `list-versions`**，所以现有的 dsh 内插件也无法触发更新检查。

#### V4（=B8）对同一外壳重复探测

`probe_global()`（`prepare.rs:121-126`）已经跑过一次 `dsh --version`；随后 `global_program_usable()`（`prepare.rs:409-420`）对**同一个 `which("dsh")` 结果**再跑一次同样的命令。stale-shim 闸门本身有 ADR 依据（交付物 `2026-08-23-tray-open-navigate-and-stale-shim-gate.md`），但"重复一次 Node 冷启动"是可以消除的：probe 成功即证明可执行，只在 probe 异常（`found=true, version=None`）时复核即可。

#### V5 其它小项

- `prepare.rs:366` 的注释写 "5-second cap"，代码是 `Duration::from_secs(3)`（`:392`）——注释漂移。
- `probe.rs:91` 把 stdout+stderr 拼成 `raw`（成功时宽松是有意的），但 `FullVersion::parse` 对这种拼接串会取**第一个**版本三元组；当前 dsh 只在 stdout 打印版本，暂无风险【推断】。
- `dsh_dir()` 明确不做版本隔离（`prepare.rs:203-212` 注释："there is deliberately no version isolation — one dsh kernel per machine is enough"）。这条决定了"选版本/回滚"只能靠**重装**实现（详见专题五）。

### 2.3 建议（按收益排序）

1. **统一比较策略**：目标来自 registry `latest` 时用 `installed >= target`（不降级）；显式 pin 时保留 `==`。降级必须是显式动作（用户点了"切到该版本"），不能由 `latest` 自动带来。
2. **支持 dist-tag**：把 `version` 先解析成具体版本号再比较（例如查询 `dist-tags` 或对 tag 调用 `<pm> view @deepseek-ai/dsh@<tag> version`），并把"tag → 已解析版本"落进状态文件，避免每次启动都要比较不可比的东西。
3. **引入机器管理的状态文件** `<cache>/dshl/dsh-state.json`：`{installed_version, resolved_from, checked_at, source(global|cache), entry_ok}`。它一次解决三件事：V3 的可观测性、B5 的自愈判定（`entry_ok=false` 即强制重装）、以及专题五"当前版本"免查询展示。
4. **按需检查 + 后台复查**：把 `check-update` 做成控制面方法（`control.rs` 加一个 method 即可，协议与分发都是现成的），UI 上给"检查更新"按钮；启动路径保持"不阻塞、不强制"。
5. **合并重复探测**（V4）。
6. 文档：`prepare.rs:366` 的 5s → 3s。

---

## 3. 专题二：为什么"本地安装/更新会被 lock"

### 3.1 先排除一个误会：dshl 代码里**没有任何安装锁**

【代码】全仓文件锁只有一个使用点：`platform/single_instance.rs:47`（`<cache>/dshl/instance.lock`，保护"启动器单实例"）。`install_dsh()`（`prepare.rs:284-360`）不取锁、不写 lockfile、不检查"这个缓存是不是正在被运行"。锁相关的机制清单：

| 机制 | 位置 | 持有范围 | 与安装的关系 |
|---|---|---|---|
| `CLI_LOCK`（进程内 Mutex） | `crates/dshl-cli/src/run.rs:36`，获取 `:74`/`:236`，释放 `:144`/`:246` | 只有 `ui::setup` 那一段 | **无关**：安装发生在 `ui::launch_flow`（`:145`/`:247`）之后 |
| `try_kernel_lock` + 10s 有界等待 | `crates/dshl-cli/src/control.rs:24-36` | napi `window_show` | 无关（ADR 明示设计） |
| `<cache>/dshl/instance.lock` | `single_instance.rs:20-22/35-57` | Track A 终身（`run.rs:118-119` `mem::forget`） | **间接相关**：它决定"第二个启动器能不能起来" |
| 单实例文件锁的失败路径 | `run.rs:117-126` → `crates/dshl/src/main.rs:31-35` | —— | **退出码 0、零输出**（见 3.4） |
| `[dsh] single-instance`（进程扫描） | `ui/launch.rs:104-110`、`platform/process.rs:158-197` | 每次 launch 开头一次 | 默认 `false`（`config.rs:110`）→ **默认不拦** |
| `FLOW_RUNNING` | `ui/state.rs`、`ui/launch.rs:18-20` | 单进程一次管线 | 同进程串行，跨进程无效 |
| Job Object（Windows） | `process/win_job.rs:22/42`，在 `child.rs:156`/`:209` 赋值 | dshl 进程终身 | 启动器退出 → **kill-on-close**：安装中途被杀 |
| PDEATHSIG（Linux） | `process/capture.rs:44-51/79-86` | 子进程终身 | 同上 |
| macOS | `capture.rs:52-55` 是空分支 | 无 | 安装子进程可存活到天荒地老 |
| pm 自身锁/store | 由 npm/bun/pnpm/nub 决定；dshl 不隔离 `npm_config_cache`/store | 外部 | 跨进程争用发生在 dshl 视野之外【推测】 |

### 3.2 真正会 lock 的东西：Windows 文件占用（本机已复现）

【实测】本机正在跑的两个进程都来自 dshl 缓存，且是"用户手动从缓存启动"的形态：

```
PID 39012  "C:\Users\Frees\.cache\dshl\dsh\node_modules\.bin\dsh.exe" --profile dsh-tui
PID 34372  node "C:\Users\Frees\.cache\dshl\dsh\node_modules\@deepseek-ai\dsh\lib\bin.js" --profile dsh-tui
PID 30480  node "...\node_modules\@deepseek-ai\dsh-subprocess-local\lib\runner.js" -- ...
```

对缓存里被映射的文件做**独占写打开**全部失败（"being used by another process"）：至少包括
`node_modules/.bin/dsh.exe` 与 `node_modules/@koromix/koffi-win32-x64/win32_x64/koffi.node`（原生插件，`.node` 被 loader 映射后不可覆盖）。

而 `~/.cache/dshl/dsh/package.json` 里带着 bun 写下的 `trustedDependencies: ["@deepseek-ai/dsh-subprocess-local", "@google/genai", "koffi", "protobufjs"]` 与 `.bin/*.bunx`——【实测】证明**这台机器上 dsh 是用 `bun add` 装进缓存的**（也说明 dsh 依赖树里确实有需要安装脚本的原生依赖）。

结论链：**缓存被运行中的 dsh 占住 → 任何"原地覆盖/删除再解包"式更新都会失败或留下半删除的 `node_modules`**。而 dshl 在动手之前完全不知道这件事（无预检、无占用提示、无重试策略）。

### 3.3 五个具体场景

**A. dsh 正在运行 + 触发更新 —— 高风险（本机就是这个状态）**
`ui/launch.rs:104` 只在 `[dsh] single-instance = true` 时才拦，而默认 `false`（`config.rs:110`）。于是：判定要装（V1 的降级判定会让它**每次都判定要装**）→ `bun add` 往正被执行的树里写 → 共享冲突 → `run_streaming` 报 `stream.exit_failed`（`locales/zh-CN.yml:117`）→ 缓存不变 → 下次启动再来一遍。

**B. 两个 dshl 同时安装 —— 默认不会发生，但一关单实例就是裸奔**
`[ui] single-instance` 默认 `true`（`config.rs:186`）→ 第二个进程在 `run.rs:117-126` 就退出，进不了安装。一旦设为 `false`（配置注释明确支持多实例），**跨进程零互斥**：两个 pm 同时抽同一个 `node_modules` = 静默损坏（bun 自己的 shim 里就带着 "corrupted node_modules" 的诊断串）。更细的共享点还有：nub/bun 的暂存目录 `.stage`（`nub.rs:111`、`bun.rs:164`）与 `pkg.tgz` 文件名（`download.rs:237`）都是固定名 → 两个进程会互相 `remove_dir_all`、并用 `curl -C -` 写同一个 partial 文件。

**C. 已有实例时再启动 CLI —— 功能上安全，体验上是"被锁死"**
挡路的是 `instance.lock`，不是 `CLI_LOCK`。失败路径是：`notify_activate()` 写 `<cache>/dshl/activate` → 睡 500ms → `RunOutcome::AlreadyRunning` → `main.rs:31-35` **退出 0、不打印任何东西**。release 构建还带 `windows_subsystem = "windows"`（`main.rs:10`），双击 exe 时连控制台都没有——用户视角就是"点了没反应/被锁住了"。已有实例会在 50ms 循环里 `poll_activate()`（`supervisor.rs:212`）把自己唤到前台，但**第二个实例本身没有任何反馈**。

**D. 上次安装被打断 —— 半成品缓存无自愈（B5）**
关窗/Ctrl+C 时 `tokio::select!`（`ui/launch.rs:122-134`）丢弃 flow future；`AsyncChild` **没有 `Drop`**（`process/child.rs` 全文无 `impl Drop`）→ 安装进程继续写，直到启动器进程真正退出被 Job/PDEATHSIG 击杀 —— **正好死在写盘中途**。下次启动只看 `package.json` 的版本串：
- package.json 已写、入口未写 → `cache_needs_install = false` → `package_entry` 失败 → `flow.prepare.entry_missing`（`prepare.rs:555-563`）→ 重试按钮（`bindings.rs:27-37`）走同一判定 → **永久卡死，只能手删 `<cache>/dshl/dsh`**。
- 若 registry 查询超时/关闭（`target = "latest"`）→ `Some(_) => false`，**任何能解析的缓存版本都放过** → 坏树永不修复。

**E. 镜像下的差异 —— 主要是感知**
dshl 只注入 registry 环境变量（`mirror.rs:51-61`：`npm_config_registry` / `NPM_CONFIG_REGISTRY` / `BUN_CONFIG_REGISTRY`），不隔离任何缓存/store → 多实例共享 `~/.npm/_cacache`、bun 的全局 install cache、pnpm store。加上"安装类子进程不设超时"（ADR 明示），镜像卡顿就是**无限期等待**，而 npm 在非 TTY 下不打进度 → 页面停在一行日志上不动。

### 3.4 "用户感觉被 lock"的根因排序

1. **第二次启动被单实例锁静默拒绝**（exit 0 + 零输出）——字面意义上的"被 lock"，且没有任何可解释信息。
2. **运行中的 dsh 占住缓存 → 更新写不进去，且失败后每次启动重试**（3.2 + V1）。
3. **安装阶段零进度 + 无超时**：`prepare.rs:316` 一行日志之后可能长时间零输出，前端 250ms 轮询照跑但内容不变（视觉卡死）。
4. **中断后的坏缓存无自愈**（D）。
5. **并发安装无跨进程保护**（B，仅在关掉单实例时）。

### 3.5 建议

**不要动（属于有意设计，改之前先改 ADR）**：CLI_LOCK 收窄到 setup；`window_show` 的 10s + HTTP 409 `booting`；锁中毒边界；安装/下载类子进程**不设超时**（要补的是进度与心跳，不是超时）；`[ui] single-instance` 的"激活而非新开"语义；`[dsh] single-instance` 的硬拒绝语义。

**建议方向**（前 4 条是本次最值得做的）：
1. **单实例拒绝要有输出**：CLI 侧打印一行 i18n 提示（或返回一个明确的 `RunOutcome`/退出码），让"被锁"可解释。这是成本最低、体验提升最大的一条。
2. **安装前的占用预检**：复用 `platform::dsh_instance_running()`（`platform/process.rs:158-197`）的思路，判断"是否有 dsh 从 `dsh_dir()` 运行"；命中就明确提示"请先关闭正在运行的 dsh"，而不是把 Windows 的共享冲突原文抛给用户。**注意**：这与 `[dsh] single-instance` 不同——后者是"拒绝启动"，前者只需要"拒绝在原地重装"。
3. **安装心跳**：`run_streaming`（`install/stream.rs:10-36`）在无输出时周期性写一行"仍在安装（已 N 秒）+ 最后一行输出"，把"卡死"变成"在跑"。**只加日志，不动超时策略。**
4. **坏缓存自愈**：把 `cache_needs_install` 的判定从"版本串"升级为"版本 + 入口可用性"（`package_entry` 已经在手边，`prepare.rs:237-251`）；入口缺失即强制重装。再加一个显式的"清理缓存并重装"入口（现在完全没有）。
5. **原子安装（可选但性价比高）**：装进 `<cache>/dshl/dsh.new`，成功后替换目录，失败保留旧树。一次解决 A/B/D 半成品问题。Windows 上"替换目录"要先处理被占用的删除（`remove_dir_all` 会被 delete-pending 卡住）【推测】，所以它最好与第 2 条配合。
6. **允许多实例时给安装加跨进程互斥**（`dsh_dir()` 上的 lockfile + 超时提示），否则在配置注释里明确警告。
7. **macOS 兜底**：`capture.rs:52-55` 目前是空操作，建议至少对安装类子进程做"父进程退出即 kill"的补偿（或在文档中写明）。

---

## 4. 专题三：逐个审查所有包管理器

### 4.1 矩阵

| | npm | bun | pnpm | nub（默认） |
|---|---|---|---|---|
| dsh 安装命令 | `npm install --prefix <dir> --no-save <spec>`（`prepare.rs:324-330`） | `bun add --cwd <dir> <spec>`（`:331-338`） | `pnpm add --dir <dir> <spec>`（`:339-346`） | `nub add <spec>` + `current_dir(<dir>)`（`:349-355`） |
| 自身如何获得 | 跟随 node（`ensure_node`） | `@oven/bun-<plat>` registry tarball → 官方脚本 → `npm install --prefix`（`install/bun.rs:91-145`） | `npm install --prefix <cache>/dshl/pnpm`（`install/pnpm.rs:64-76`） | `@nubjs/nub` + `@nubjs/nub-<plat>` registry tarball（`install/nub.rs:104-136`） |
| 镜像 env | `npm_config_registry` / `NPM_CONFIG_REGISTRY` / `BUN_CONFIG_REGISTRY`（`mirror.rs:51-61`） | 同左 | 同左 | 同左 + `NODEJS_ORG_MIRROR`（`mirror.rs:66-78`） |
| 版本查询工具 | `npm`（`prepare.rs:380`） | `npm`（`:380`） | `pnpm`（`:383`） | `nub`（`:382`，**`nub view` 未证实存在**） |
| 入口解析 | 统一读 `node_modules/@deepseek-ai/dsh/package.json` 的 `bin` → `node <entry>`（`prepare.rs:237-251/564-568`） | 同左 | 同左（注意 pnpm 用 symlink/junction） | 同左 |
| 失败行为 | `run_streaming` 非零 → 硬错（无回退） | 同左 | 同左 | 同左（但**前面已经注定失败**） |
| 依赖安装脚本策略 | 引擎审批 | **执行**（npm 默认跑 script） | **默认拦截**（需 `trustedDependencies`，本机 manifest 可见） | 需 `approve-builds` |
| 改写 manifest / 留 lockfile | 否（`--no-save`） | 否（`--no-save`，是否仍写 lockfile【推测】） | 是（会写 dependencies + `trustedDependencies`） | 是 |
| Windows "只有缓存一份"时能否被发现 | **否**（`.cmd` 兜底 → 文件不存在） | 是 | **否**（同左） | 是（但缓存判定查 `.exe` → 见 F3） |
| 测试覆盖 | 无 | 仅 `oven_package_for()` 平台映射单测（`bun.rs:209-224`） | 无 | 无 |
| 本机验证 | 未直接验证（PATH 语义已实测，见附 A.2） | **实机在跑**：缓存 `package.json` 有 `trustedDependencies` + `.bin/*.bunx`（bun 签名） | 未验证 | **实机不可用**（见 4.2） |

### 4.2 nub（默认 pm）—— 两个独立缺陷，都足以让它完全不可用

**N1：registry URL 少了 `@`（阻断级）**

```rust
// src/install/nub.rs:106
let latest_json = http_get_text(&format!("{base}/{}%2Fnub/latest", "nubjs")).await?;
```
拼出来是 `{base}/nubjs%2Fnub/latest`。npm registry 的 scoped 包元数据端点需要 `@`：

| URL | 实测状态码 |
|---|---|
| `https://registry.npmjs.org/nubjs%2Fnub/latest` | **405** |
| `https://registry.npmjs.org/@nubjs%2Fnub/latest` | 200 |
| `http://registry.npmmirror.com/nubjs%2Fnub/latest`（默认镜像） | **422** |
| `https://registry.npmmirror.com/@nubjs%2Fnub/latest` | 200 |

`http_download` 用 `curl -fL`（`download.rs:167-172`），HTTP 4xx 直接非零退出 → `http_get_text` 返回 Err → `ensure_nub` 记负缓存（`nub.rs:149-152`）→ **本会话不再尝试**。对比 `bun.rs:159` 的实现（`pkg.replace('/', "%2F")`，带了 `@`）——同一份代码里两种写法，nub 那条是错的。

**N2：就算下载成功，缓存里的 nub 也启动不了（阻断级）**

`ensure_nub` 的组装顺序（`nub.rs:122-128`）是"先平台包 bin，再主包 bin 覆盖"：

```
<cache>/dshl/nub/bin/  ← 平台包 bin/：nub.exe、busybox.exe、nub-launcher-win32-x64.exe【实测 tarball 清单】
                       ← 主包 bin/：nub、nubr、nubx（Node shim）、launch.js   ← 覆盖写入
```

- **Windows**：`platform::tool("nub")`（`paths.rs:83-91`）= `which("nub")`。`which` 只搜 `PATH` + `known_tool_dirs()`（`paths.rs:130-175`），**不含 `<cache>/dshl/nub/bin`** → 找不到 → 回退成 `PathBuf::from("nub.cmd")`。而缓存里没有 `nub.cmd`（`.cmd` 是 npm 全局安装时才生成的，见主包 `launch.js` 自述）→ `Command::new("nub.cmd")` 直接 "program not found"（**PATH 会生效但文件不存在**，见附 A.2 实验）。→ `install_dsh` 必然失败。
- **Unix**：主包的 `bin/nub` 是 `#!/usr/bin/env node` 的 shim，**覆盖掉平台包的同一个文件名**；它 `require("./launch.js")`（同目录，已复制）→ `launch.js:44` 又 `require("../platform.js")` —— `<cache>/dshl/nub/platform.js` **从未被复制**（只复制了 `bin/`）→ MODULE_NOT_FOUND。

**N3：其它 nub 相关小项**
- `nub.rs:87-100`：同一段"缓存命中直接返回"的代码写了两遍，第二遍（`:97-100`）在两行之间没有任何文件系统写入，**不可达**。
- 平台包映射没有覆盖 musl（`@nubjs/nub-linux-x64-musl` 存在但 `nub.rs:40-64` 不映射）→ Alpine 会拉 glibc 二进制。
- 主包与平台包的 `latest` 实测**可以不一致**（2026-09-22：主包 0.9.3、`@nubjs/nub-win32-x64` 0.9.4），而 `nub.rs:114-118` 用主包版本去拉平台包——只要那个组合没发布就是 404。
- 代码注释多处引用 "0.7.5"，而本机全局 nub 是 **0.8.3**、上游 npm latest 已是 **0.9.3**；`NUB_VERSION_MARKER` 只写不校验（`nub.rs:31/156-160`）→ **没有最低版本门**。
- 【实测】本机 `nub 0.8.3 --help`：`view` **存在**（Inspect dependencies 分组的 `view / search / bin / root / query / check / sbom`），`nub node which` 与 `nub node install` **也存在**（`nub node — manage Node versions`）。所以 `prepare.rs:381-382` 的 `nub view` 与 `nub.rs:217-228` 的 `nub node install|which` **命令形态是对的**——nub 的问题只有 N1（下载 URL）与 N2（装配/解析），不涉及子命令。

> 为什么这个 P0 能活到今天：`dshl.example.toml` 把默认改成了 `pm = "nub"`，但**已存在的 `dshl.toml` 不会被迁移**（`config.rs:281-296` 只在"一个配置都找不到"时才写模板）。本机就是活证据：用户配置里还留着上一代模板的 `bun-download = ""` 且 `pm = "bun"`【实测】。也就是说 **nub 路径只有全新安装的用户才会走到**，而作者本机走的始终是 bun。

### 4.3 其余三个 pm 的注意点

- **npm**：命令正确（`--prefix` + `--no-save`）。风险点只有一个：`platform::tool("npm")` 依赖 `which`，而"dshl 自己装的 node"（fnm → `<cache>/dshl/fnm/node-versions/...`）不在 `known_tool_dirs()` 里 → 回退成裸 `npm.cmd` → **靠子进程 PATH 生效**才找得到。这条链路已实测成立（附 A.2），但它建立在"Rust 在 Windows 上用子进程 env 的 PATH 解析程序名"这一行为上，建议加一条回归测试锁住（见 4.5）。
- **bun**：本机实机在用，工作正常。注意 `bun add` 会往缓存 `<dir>/package.json` 写 dependencies（本机就多了一条用户手动加的 `@deepseek-harness-tui/dsh-tui`）——这是**用户态可写**的文件，dshl 从不清理它，也从不读它（只读 node_modules 里的 package.json）。
- **pnpm**：命令形态正确（`pnpm add --dir`），但有两个真实风险：
  1. `node_modules/@deepseek-ai/dsh` 是 **symlink/junction**，`package_entry` 用 `is_file()` 穿透（Windows 上一般成立），但仓库历史教训明确写过"bun add 回归测试因 Windows junction 复制 `PermissionDenied`"（AGENTS.md 测试约束）——**这条路径没有回归保护**。
  2. dsh 依赖树含需安装脚本的原生依赖（bun 写的 `trustedDependencies` 就是证据：`koffi`、`protobufjs`、`@google/genai`、`@deepseek-ai/dsh-subprocess-local`）。**pnpm 10+ 默认阻止依赖的 build script** → 用 pnpm 装出来的树**可能缺少 koffi 的原生绑定**，表现为 dsh 运行时才炸【推断，需实测】。若确认，`pnpm add` 需要加 `--config.onlyBuiltDependencies` 或 `--allow-build`。

### 4.4 一致性缺口（四个 pm 之间）

| 项 | 现状 | 影响 |
|---|---|---|
| pm 二进制解析 | `install_dsh` 用 `platform::tool(pm)`（只搜父进程 PATH + 已知目录），而 node 用 `which_in("node", runtime.path_prefix())`（`prepare.rs:564-565`） | **正是 N2 的根因**。统一成 `which_in(pm, &runtime.path_prefix())` 即可让缓存里的 pm 被发现 |
| 临时文件 | `http_get_text` 用 `temp_dir()/dshl-get-{pid}`（`download.rs:186`） | 同进程并发调用会互相覆盖（当前调用是串行的，属潜在坑） |
| 平台覆盖 | bun 有完整平台矩阵单测；nub 手写三分支且漏 musl；npm/pnpm 无需 | nub 的映射最脆弱却没有测试 |
| 失败回退 | bun 本体安装有三层回退；pnpm/nub 本体只有一层 | `pm = pnpm` 的机器若 npm 装不动 pnpm 就没有 Plan B |
| 版本查询 | npm/bun → `npm view`；pnpm → `pnpm view`；nub → `nub view` | 三种语义/三种失败模式，且都静默 |

### 4.5 建议

1. **立即修 N1**（一行：`format!("{base}/@nubjs%2Fnub/latest")`），并给 `package_tgz_url`/元数据端点加一条**离线单测**（纯拼串断言，锁住 `@` 与 `%2F` 的约定）。
2. **修 N2**：统一用 `which_in(pm, runtime.path_prefix())` 解析 pm；nub 组装时**先主包（JS）后平台包（原生）**，或干脆只复制平台包的 `bin/`（原生二进制才是要 PATH 的东西），把主包整体（含 `platform.js`）放到 `<cache>/dshl/nub/pkg/` 供 shim 解析。
3. **给 pnpm 路径补一次实测**（尤其是 koffi 原生绑定），并把 `--allow-build`/`onlyBuiltDependencies` 是否需要写进代码决定下来。
4. **加一条 PATH 解析回归测试**：在临时目录放一个假 `x.cmd`，用 `Command::new("x.cmd").env("PATH", tmp)` 断言可启动——把"Rust/Windows 使用子进程 PATH"这个隐含前提钉死（本报告附 A.2 已给出可复制的实验）。
5. **`nub view` 已实测存在**（nub 0.8.3），所以版本查询这条不用改；但 `pm = nub` 组合的自动更新依然会被 N1/N2 连带打死。

### 4.6 追加发现（pm 深度核对）

以下每条都经过源码或本机实测确认，按严重度排列。

| # | 级别 | 结论 | 证据 |
|---|---|---|---|
| F1 | **阻断** | nub 缓存在 POSIX 上**覆盖掉平台包的原生二进制**（上游包实测：`@nubjs/nub-linux-x64`/`-darwin-arm64` 的 `bin/nub` 才是原生二进制 48–59 MB，主包 `bin/nub` 只有 475 B）；且 dshl 只写 `bin/`，launcher 需要的 `node_modules` 布局从未建立 | `nub.rs:122-128`、`:180-194`（`fs::copy` 覆盖）、`:83`（检查名在 POSIX 恰为被覆盖的那个） |
| F2 | **高** | `platform::tool()`（`paths.rs:83-91`）对**原生 exe 工具**会回退成 `<name>.cmd`；一旦工具只在 dshl 缓存里（不在用户 PATH），这个回退就指向不存在的文件 → bun/nub 必然 spawn 失败。`.cmd` 兜底只对 npm 风格 shim 成立 | `paths.rs:83-91` 用于 `prepare.rs:332/350/385/565`；`.cmd` 缺失已由附 A.2 实测坐实 |
| F3 | **高** | pnpm 的缓存判定查 `pnpm.exe`（`pnpm.rs:55/78`），而缓存由 **npm** 填充成 `pnpm.cmd` → 判定恒失败 → 回落目录（`:83/138-146`）又不含缓存 `.bin` → `install_dsh` 硬失败，且**每次启动重跑一次联网 `npm install pnpm`** | `pnpm.rs:55-58/64-83/138-146` |
| F4 | 中 | bun 的 npm 回退成功判定同样查 `bun.exe`（`bun.rs:139-142`）、缓存复用检查 `:58-63` 同病 → 死回退 + 误报 | `bun.rs:53-68/131-144` |
| F5 | 中-高 | **探测类子进程其实没有超时**（=B13）：`probe.rs:44-73` 不带 timeout，与 AGENTS.md「probe 30s」的承诺不符；启动管线也没有看门狗（`ui/launch.rs:122-134` 只 race 关停标志）→ 一个挂死的 `--version` 会让启动页永久停在"探测中" | `probe.rs:44-73`、`process/capture.rs:90-97` |
| F6 | 中 | `pnpm bin -g` 是唯一无界的探测类调用（`pnpm.rs:99-105`） | 同左 |
| F7 | 中 | `probe::nvm()` 的 Windows 分支绕过 `tool_from_result`：非零退出仍 `found=true` 并从 stdout 取版本 —— 正是 skill 点名的"从崩溃输出挖版本号"回归类 | `probe.rs:141-160` vs `:83-102` 及其回归测试 `:189-203` |
| F8 | 中 | **`MirrorMode::Force` 无实现**（=B15）：`forced()`（`mirror.rs:46-48`）无调用者；`On` 承诺的"失败回退原始源"也不存在；bun 的官方脚本路线在 Force 下仍直连 `bun.sh` | grep 实证 + `config.rs:13-18`、`bun.rs:109-127` |
| F9 | 低 | 四处死代码/赘余：`nub.rs:97-100`（不可达的重复快路径）、`nub.rs:250 ensure_nub_with_node`（无调用者）、`paths.rs:179 default_pnpm_bin_dir`（被内联重复）、`MirrorConfig::forced` | 左列 |
| F10 | 低 | nub 的缓存快路径**排在全局探测之前**（`nub.rs:87-95`），与文件头注释"用户全局 nub 仍然优先"、也与 bun（`bun.rs:35` 全局优先）相反 | 左列 |
| F11 | 低 | nub 假设以 0.7.5 验证（`nub.rs` 头、`prepare.rs:347`）却默认拉 latest（≥0.9.3）；`NUB_VERSION_MARKER` 只写不校验 → 无最低版本门；平台矩阵漏 musl（上游有 `-musl` 包），Windows ARM64 上游也不支持而 CI 有 ARM 腿 | `nub.rs:31/40-64/156-160` |
| F12 | 低 | 四家 pm 的语义并不等价：`--no-save` 只有 npm 用（`prepare.rs:328`）；谁都不传 `--ignore-scripts`；postinstall 策略天然不同（npm 执行 / bun 默认拦截 / pnpm 需 approve）→ **同一个 dsh 依赖树在不同 pm 下的产物可能不同**（koffi 这类原生依赖尤其敏感） | `prepare.rs:323-356` |

**文档即契约偏差（一并要改）**：
- `AGENTS.md:29/101`、`README.md:88-91`（nub 行重复、缺 pnpm）与 `Pm` 的四值不符；
- `README.md:253` / `README_en.md:291` 的 `# npm | bun | pnpm` 漏了默认的 nub；
- `README.md:121` / `README_en.md:143` 把 dsh 的安装描述成"bun=@oven 平台包直连 registry"——那是 **bun 本体**的装法（`bun.rs:151-183`），dsh 走的是 `bun add --cwd`（`prepare.rs:331-338`）；
- **"probe 30s" 这个承诺在代码里不存在**（`AGENTS.md:117`、`README.md:133-134`、ADR 网络策略表都写了）；
- `README.md:283-284` 的"最多 5 秒"↔ 代码 3 秒（`prepare.rs:392`）；
- `locales/zh-CN.yml:114`/`en.yml:114` 的"未找到 nub，开始通过 npm 安装"与 `nub.rs:1-7` 的"从不 spawn npm"矛盾（用户可见文案）；
- `dshl.example.toml:26-27`（"nub installs … from npm"）与 `install/mod.rs:3` 的顺序描述也与实现不符。

**测试覆盖事实**：`install/` 下**四种 pm 的安装路径全部没有测试**（只有 `bun.rs:209-224` 的平台映射纯函数单测与 `stream.rs` 的排水测试）；`prepare.rs` 的测试只覆盖纯决策函数（`split_args` / `dsh_version_ok` / `cache_needs_install` / `cached_dsh_version`），`install_dsh()` 本身不可测（它直接拼 `Command`）。CI（`ci.yml`）不触发任何真实安装。**"默认 pm 的缓存自举从未被跑过"因此是系统性的，而不是偶然的。**

---

## 5. 专题四：有没有更先进的模式／配置方法

### 5.1 现状（准确描述）

- **配置模型**：单一 `dshl.toml`（`config.rs:192-201`），全字段可选、带默认；**每次启动重读**（`ui/launch.rs:36`）。
- **发现顺序**：`--config` → `./dshl.toml` → exe 目录 → 平台配置目录（Linux 再兜 `/etc/dshl/dshl.toml`）；**先命中者胜，不合并**（`config.rs:232-246`）。
- **写入**：只在"一个配置都找不到"时写一次注释模板（`config.rs:281-296`）；**从不修改已存在的配置**。
- **错误**：解析失败 → 回退内置默认 + 把原始错误字符串丢给 UI（`config.rs:257-262`、`ui/launch.rs:56-58`）。
- **覆盖层**：CLI 只有 `-c/--config` 与 `-d/--debug`（`crates/dshl-cli/src/run.rs:85-107`）；环境变量只有内部接线用的 `DSHL_CACHE`（`platform/paths.rs:30-34`）、`DSHL_LOG`（`run.rs:272-275`）、`DSHL_CONTROL_URL`（`control.rs:49`）。
- **热重载**：部分已有——`dsh.*` 每次 `launch_flow()` 重读；`ui.mode`/`ui.close_to_tray` 只在 `ui::setup` 读一次（`ui/window/setup.rs:293-295`）。
- **运行时共享态**：全局静态（`progress::STATE`、`DSH_CHILD`、`sessions`…）——与本仓架构一致，不需要动。
- **i18n**：`rust_i18n::set_locale` 运行时可切，但 `i18n::LOCALE` 是 `OnceLock`（`i18n.rs:11-25`），页面文案经 `/i18n.js` 一次性快照（`ui/vfs.rs:59-70`）→ 真正的"运行时换语言"要配套改这两处 + 重载页面。
- **可观测性**：`progress::log` 是**纯字符串**（`progress.rs:110-120`），`State` 里没有任何进度量化字段（`progress.rs:42-55`）。

### 5.2 建议（按"收益/成本"排序）

| 优先级 | 建议 | 收益 | 成本/风险 |
|---|---|---|---|
| ★★★ | **状态文件层** `<cache>/dshl/state.json` + 沿用 `pending-*` 意图文件（`control.rs:302-327` 已是现成习语）：机器管理的状态与用户手写的配置分离 | 一次解决：安装状态可观测/自愈判定、当前版本展示、"改源/改版本"不污染用户注释 | 低：无新依赖，纯 Rust 侧读写；要定 schema 与失败降级 |
| ★★★ | **配置写回改用 `toml_edit`**（仅在"用户显式点保存"时） | 让"设置页"成为可能且不毁注释 | 中：新增一个依赖（体积可接受，`toml_edit` 很小），需原子写 + 备份 |
| ★★☆ | **CLI 覆盖参数**：`--version <v|tag>`、`--pm <pm>`、`--mirror <url>`、`--mode <global|hybrid|private>`、`--dry-run`（打印五步决策不执行） | 脚本化/CI/沙箱循环立刻受益；`--dry-run` 让"版本检查决策"可自诊断 | 低：`RunOptions` 结构已存在（`crates/dshl-cli/src/options.rs`），只是加字段与优先级（CLI > env > toml） |
| ★★☆ | **结构化事件流**：`progress::event(kind, fields)`（保留 `log` 兼容） | 进度条、依赖表、"正在安装第 N 秒"都能共用一套数据 | 低-中：改 `progress.rs` + 前端渲染；注意 250ms 轮询的增量渲染不能退化成重建 |
| ★★☆ | **配置错误定位**：`toml::de::Error` 自带 span → 渲染"第 N 行第 M 列 + 提示" | 目前用户只看到一整串英文错误 | 低 |
| ★☆☆ | **配置分层合并**（global → cwd 逐层覆盖，而非先命中者胜） | 更"现代"，支持"全局默认 + 项目覆盖" | 中：改变既有语义（用户预期"最近的赢"），需要 ADR；建议先加"显式 `include`"这种加法式能力 |
| ★☆☆ | **外部覆盖层** `DSHL_DSH__VERSION` 之类 | 容器/CI 友好 | 低收益（已有 `-c` + 环境变量即可绕开）；容易变成第二套配置真相，慎加 |
| ✗ | 配置框架/DI 容器、把内核做成常驻守护进程、多配置格式（YAML/JSON5）、远端配置 | —— | 与"单 exe / 低消耗 / 无运行时"的架构承诺冲突，**不建议** |

**一条重要的现状判断**：镜像相关设计（`mirror.rs` 4 路 env、临时生效、从不写全局）本身是**先进且克制**的，属于本仓最好的部分之一。要动它必须走 ADR，不能由 UI 需求顺手推动。

### 5.3 顺带发现：版本号有三套真相（B12）

【实测】最新 tag 是 `v0.2.22`，但 `Cargo.toml` 的 workspace version 是 `0.2.0`，而 `package.json`（根 + 三个插件）都是 `0.1.0`；编译产物 `dshl --version` 打印 `dshl 0.2.0`。

- Rust 侧版本是手改的（`Cargo.toml:17`），JS 侧版本在 CI 里由 tag 驱动（`release-plugins.yml:69-73` 调 `scripts/bump-versions.mjs`，`release-native.yml:141` 直接写 tag 版本），**提交进仓库的 JS 版本永远是旧的**。
- 消费者：`main.rs:24`（CLI `--version`）、`src/control.rs:260`（控制面 `ping` 的 `version` 字段）、`crates/dshl-native/src/platform.rs:18`。
- 影响：用户/插件看到的"启动器版本"与发布标签对不上；将来做"启动器自身检查更新"或版本页时必须先决定**唯一版本源**（建议：以 tag 为准，构建时用 `build.rs` 注入 `DSHL_VERSION` 环境变量，`--version`/`ping` 统一读它）。

### 5.4 配置面的追加发现

1. **读路径带写盘副作用**：`config::load()` 在"一个配置文件都找不到"时会**写模板**（`config.rs:281-296`）。也就是说"读配置"这个动作可能产生文件。它被调用了**最多三次**（`crates/dshl-cli/src/run.rs:112` 的单实例判定、`ui/window/setup.rs:293` 的 UI 模式、`ui/launch.rs:36` 的管线），每次都可能触发写盘与三次解析。建议拆成 `discover()`（纯读，可缓存一次快照）+ 显式 `ensure_template()`。
2. **解析失败是"整份回落默认"**：一个字段写错（例如 `pm = "nub "` 或拼错枚举值）会让整份配置退回默认值，然后**悄悄用默认 pm 继续启动**（`config.rs:257-262` + `ui/launch.rs:45/56-58` 只在页面留一行错误）。建议改为"字段级回落 + 明确告警"，并把"生效值及其来源"显示在页面上。
3. **未知键静默忽略**（`config.rs:334-345` 的测试把它固化为契约）——这对向后兼容是好事，但用户改错键名时没有任何提示；建议 section 级未知键**告警**（非致命）。
4. **一次启动最多读三次配置**（见 1），而 `ui.mode` / `close_to_tray` 只在 setup 读一次 —— 前者是纯粹的低效，后者是"改了不生效"的预期差异，建议在文档里写清哪些字段需要重启。
5. `DSHL_PTY_TRACE`（`pty/`）等诊断 env 已存在，说明"env 覆盖层"的习语在本仓是成立的——把它扩到 `pm/mode/version/mirrors` 是**低风险、高收益**的一步（也正好解决 AGENTS.md 里"沙箱循环必须手改 dshl.toml 才能切 mode/version"的痛点）。

---

## 6. 专题五：能不能做成 PCL / HMCL 那样的启动器

结论：**可行，而且比同类项目便宜得多**，因为骨架已经在了；但有三条真实约束必须先接受。

### 6.1 现状：只有一个"启动页"，没有"启动器"

- 界面是一份 95 行的单文档（`assets/index.html:35-88`）：header + 错误/崩溃横幅 + 启动流程清单 + 配置 kv + 日志 + 底部动作条。**没有导航、没有路由、没有第二个页面**。
- 前后端只有两条通道：**下行** = 前端 250ms 轮询 `get_state()`（`assets/app.js:307`）拿整个 `progress::State` 的 JSON（`ui/bindings.rs:14-16`、`progress.rs:183-185`）；**上行** = 7 个几乎零参的 binding（`ui/bindings.rs:94-102`：`get_state/exit_app/retry/force_kill_stale/open_config/restart_now/cancel_restart`），且**全仓从未使用过 webui 的取参 API**。
- 静态资源全部 `include_str!` 内嵌（`ui/assets.rs:3-17`），经内存白名单路由（`ui/vfs.rs:104-151`）；i18n 经 `/i18n.js` 快照 23 个 `PAGE_KEYS`（`ui/vfs.rs:13-37`）。
- 另有一条常被忽略的面：dsh 起来后，插件会往 dsh 页面注入一个浮动控制条（Terminal/Console/Restart/Shutdown/Guard，`plugins/dshl-control/src/index.js:698-699` + `ui.js`）。

### 6.2 能力对照

| PCL/HMCL 的能力 | DSHL 现状 | 证据 |
|---|---|---|
| 多页面/设置页 | ✗ 不支持（单文档无路由） | `index.html:35-88` |
| 多版本列表与选择 | ✗ 不支持（无 `versions` 查询；`version` 只是一个串） | `prepare.rs:370-401`；全仓无 "dist-tag/versions" 查询（grep 实证） |
| 固定版本 / 回滚 | △ 部分：能 pin 到任意版本号（写 toml），无 UI、无回滚语义、且会被 V1 影响 | `config.rs:122-128`、`prepare.rs:456-465` |
| 换源 UI | ✗ 不支持（只读展示，且那一行还是空的 → B9） | `assets/app.js:99-110`、`config.rs:196` |
| 换源机制本体 | ✓ 已支持（4 路 env，临时注入） | `mirror.rs:51-115` |
| 实例/档案 | △ 部分：`-c/--config` 已可多档案；dsh 自身 `--profile` 可切换（控制面） | `run.rs:91-98`、`control.rs:266-274/368-401` |
| 多开并存 | 默认禁止（启动器单实例） | `config.rs:187`、`run.rs:111-126` |
| 下载中心 / 进度可视化 | ✗ 无结构化进度（只有 5 步状态 + 原始日志行） | `progress.rs:23-29/42-55`、`install/stream.rs:10-36` |
| 依赖探测可视化 | ✓ 7 个工具并发探测 + 逐行报告 | `runtime_env.rs:46-82` |
| 启动按钮 + 日志 + 崩溃恢复 + 托盘 + 窗口几何记忆 | ✓ 已有 | `progress.rs`、`ui/crash.rs`、`ui/geometry.rs` |
| 离线可用 / UI 秒开 | ✓ 已有（pin 版本零网络；资源全内嵌；首帧前灌状态） | `prepare.rs:456-457`、`ui/assets.rs`、`ui/launch.rs:29-53` |

### 6.3 三条硬约束（必须接受，否则做不成"低消耗"）

1. **生效时机**：镜像在管线第 2 步就被消费（`flow/mod.rs:66`、`runtime_env.rs:132-146`），`mirror_check` 只是事后展示。所以"换源/换版本/换档案"的统一样式只能是 **写意图 → 重跑管线**，不能热改。好消息：这正好等于既有的 `request_restart()` 语义（`ui/launch.rs:240-249`）。
2. **"镜像从不落盘"是本仓铁律**（AGENTS.md:114、`mirror.rs:1-5`）。UI 换源应当**只影响本次运行**（照抄 `control.rs:302-327` 的 `pending-profile`：读一次即删）。"保存为默认"是一个**规则修订**，该由维护者显式决策，不该被 UI 需求顺手带出——若要做，也建议写进"用户自己的 `dshl.toml`"（那是用户的配置，不是工具链全局配置），并用 `toml_edit` 保注释。
3. **不要把 PCL 的"多开/多实例"照搬过来**：dsh 的会话日志不隔离会**永久损坏**（`ui/launch.rs:97-110` 的既有论证），且启动器单实例默认开启。正确类比是"一次一个活跃档案 + 快速切换"。

### 6.4 最小可行设计（不改机制，只加字段）

- **UI**：仍在**同一份嵌入文档**里做 hash 路由（`#/`、`#/versions`、`#/mirrors`、`#/profiles`、`#/deps`、`#/settings`），不引入前端构建链、不加第二个 HTML（否则要多份轮询/i18n 装配）。
- **数据**：扩 `progress::State`（`progress.rs:42-55`）而不是新通道；`get_state` 继续是唯一数据源。
- **动作**：新增 binding（`ui/bindings.rs:94-102`）如 `apply_mirror(json)` / `set_version(v)` / `list_versions()`；**前置验证项**：确认 fork 的 `webui-rs` 暴露 `Event::get_string`（upstream 有；本机 cargo git checkout 已清空无法查证）。若不可用，回落到 vfs 的 `/api/...?k=v`（`ui/vfs.rs:107` 已证明 query 能到达 Rust）。
- **版本清单**：新增一个有界的 registry 查询（`npm view @deepseek-ai/dsh versions --json`，复用 `serde_json`），**只在用户点击时调用**，保持 3s 上限；选中 → 写 `pending-version` → 重跑管线（= "元数据多版本、文件单版本"）。真正的 side-by-side 多版本要改 `dsh_dir()` 一族与 PATH 注入（`prepare.rs:213-228/576-588`），风险高，留到最后。
- **换源**：写 `pending-mirror.json` → `MirrorConfig::resolve` 仍是唯一注入源，只换值不换机制；页面上明确写"仅本次运行生效"。
- **档案**：`<cache>/dshl/profiles.json`（名字 → toml 路径）即可起步；档案级缓存隔离用现成的 `DSHL_CACHE`（`platform/paths.rs:30-34`），但它是进程启动时读的 → 切档案必须重启进程。
- **进度**：`install/download.rs:155-182` 的 `http_download` 目标路径已知 → 用 250ms 采样 `stat(dest)` 就能量化"已下载 N 字节"（顺带天然反映 `-C -` 续传）；`run_streaming` 给日志行带上 stage 标签。**注意** curl 的进度表用 `\r` 写，会被行泵灌成一堆行——要么抑制，要么别解析它。
- **风格**：遵循 `DESIGN.md`（唯一超蓝 `#2f6fe4`、全方角无阴影、mono 仅日志/数据、禁 emoji/图标字体）。新增文案必须同时落 `locales/{zh-CN,en}.yml` 与 `ui/vfs.rs:13-37` 的 `PAGE_KEYS`。

### 6.5 分阶段路线（每阶段可独立交付）

| 阶段 | 目标 | 验收标准 |
|---|---|---|
| **P0**（半天） | 现状修复：修 B9（`auto-mirror` 恒空）+ 补几行只读配置展示 | 页面配置表出现真实值；`scripts/gate.ps1` 全绿 |
| **P1**（2-3 天，关键路径） | hash 路由 + 导航 + 「镜像」页（session-only 换源） | 改源后从第 2 步重跑管线可观察；**`dshl.toml` mtime 不变**；断网 3s 内如实报错；意图文件消费后删除 |
| **P2**（2-3 天） | 版本页（列版本 / 固定 / 回 latest） | 断网点击 3s 内报错且不写缓存；pin 后启动零 registry 查询；切回 latest + auto-update 才恢复查询；`cache_needs_install` 既有单测保持绿 |
| **P3**（2-3 天） | 依赖/下载与进度可视化 | 续传时进度从已有字节继续；curl 的 `\r` 不污染日志；采样线程随子进程结束；`install/stream.rs` 的时序测试红线不动 |
| **P4**（2-3 天） | 档案切换（restart-based，不做多开） | 切换后旧 dsh 优雅退出 → 新管线生效；单实例语义不被绕过 |

**黑名单**（会破坏"低消耗 Rust 核心"的做法）：Electron/Tauri/Qt/JavaFX；打包 Node/Chromium；给 `assets/` 加构建链（vite/webpack/TS）；用 HTTP 客户端 crate 取代系统 curl（会丢断点续传语义）；把镜像写进 `.npmrc`/cargo 配置；启动路径上的无界网络调用；CDN/在线字体/图标字体/emoji；把 250ms 轮询改成 push；在 webui 回调里做阻塞工作；只改 `dsh_dir()` 做多版本而不同步 `dsh_bin_dir()`/`dsh_pkg_dir()`/PATH 注入；为版本列表放宽 3s 上限。

---

## 7. 建议的修复顺序（把"审查"变成"可执行"）

**P0（今天，1 小时内可完成，收益最大）**
1. `src/install/nub.rs:106` 补 `@` → `format!("{base}/@nubjs%2Fnub/latest")`，并加拼串单测。
2. `install_dsh`（`prepare.rs:323-356`）解析 pm 二进制改用 `platform::which_in(pm_name, &runtime.path_prefix())`（与 node 一致）；nub 组装顺序改为"平台包在后"（或只复制平台包 `bin/`、主包整体放 `pkg/`）。
3. **`.exe`/`.cmd` 判定修正**：`pnpm.rs:55/78`、`bun.rs:58-63/139-142` 的缓存/回退成功判定在 Windows 上必须同时接受 `.cmd`（或直接用 `which_in` 判定）。
4. `cache_needs_install`（`prepare.rs:269-279`）：目标来自 `latest` 时用 `>=`（不降级）；并叠加"入口可用性"检查（`package_entry` 失败即强制重装）→ 顺手解决 B3 + B5。
5. `crates/dshl/src/main.rs:31-35` 的 `AlreadyRunning` 分支打一行提示（i18n 键），别静默 exit 0。
6. 文档同步：`AGENTS.md:29` 的缓存路径、`prepare.rs:203/366` 的注释、`README.md:88/253`、`locales/*.yml:114` 的 nub 文案。

**P1（本周）**：安装前占用预检（复用 `dsh_instance_running` 思路）+ 安装心跳日志 + 显式"清理缓存并重装"入口 + （可选）原子安装；**给探测类子进程补 30s 有界**（F5/F6，把已经写进 AGENTS.md/README 的承诺兑现）+ 统一 `probe::nvm` 的退出码门（F7）；macOS 的孤儿补偿；为 F2/F3/F4 加不联网的解析回归测试（临时目录放假 `.cmd`/假 `.exe`）。

**P2（下一轮）**：状态文件 `state.json` + dist-tag 支持 + `check-update`/`update` 控制面方法 + pnpm 原生依赖实测（koffi）+ `MirrorMode::Force` 要么实现要么从配置里删掉 + 配置 `discover()` 纯读化/字段级回落（§5.4）+ 版本单一来源（B12）。

**P3**：专题五的 P0→P4 路线。

---

## 附录 A：本次实测记录

### A.1 registry 探测

```
405  https://registry.npmjs.org/nubjs%2Fnub/latest
200  https://registry.npmjs.org/@nubjs%2Fnub/latest
200  https://registry.npmjs.org/@oven%2Fbun-windows-x64/latest
200  https://registry.npmjs.org/@deepseek-ai%2Fdsh/latest
422  https://registry.npmmirror.com/nubjs%2Fnub/latest
200  https://registry.npmmirror.com/@nubjs%2Fnub/latest
```
`@deepseek-ai/dsh` 的 dist-tags（原站与 npmmirror 一致）：`{alpha: 0.1.6-alpha.2, latest: 0.1.5-rc.2, next: 0.1.5-rc.2}`；包本身无 `scripts`、无 `optionalDependencies`，依赖树含 `koffi`（原生）。

### A.2 Rust `Command` 的 PATH 解析实验（决定性）

在临时目录放一个假 `faketool.cmd` 与 `fakeexe.exe`，用 `Command::new(name).env("PATH", tmp)` 逐个试：

```
faketool.cmd: OK exit=Some(0) out="faketool-ok"
faketool:     ERR program not found      ← 无扩展名不会自动补 .cmd（除非 PATHEXT 里有且在目录中）
fakeexe:      OK exit=Some(2)
fakeexe.exe:  OK exit=Some(2)
control(no dir on PATH): ERR program not found
```
结论：**Windows 上 Rust 用子进程的 `PATH` 解析程序名**（所以 `cmd.env("PATH", augmented)` 是有效的，npm/bun/pnpm 的裸名回退能工作）；但**名字带扩展名时不会智能兜底**——这正是 `platform::tool("nub")` 回退到 `nub.cmd` 却找不到文件的原因。

### A.3 nub 包布局（tarball 实测）

```
@nubjs/nub 0.9.3（46 KB 解包）
  package/bin/nub       ← #!/usr/bin/env node  shim
  package/bin/nubr      ← shim
  package/bin/nubx      ← shim
  package/bin/launch.js ← 内含 require("../platform.js")   ← 该文件不在 bin/ 里
  package/platform.js   ← 未被 dshl 复制
  package/postinstall.js

@nubjs/nub-win32-x64（54 MB 解包，部分清单）
  package/bin/busybox.exe
  package/bin/nub-launcher-win32-x64.exe
  package/bin/nub.exe
```

### A.4 本机真实状态（决定"被 lock"的那条链）

```
~/.cache/dshl/dsh/package.json  →  dependencies: @deepseek-ai/dsh = 0.1.6-alpha.2
                                   + 用户手动加的 @deepseek-harness-tui/dsh-tui = latest
                                   + trustedDependencies（bun 签名）
~/.cache/dshl/dsh/node_modules/.bin/ → 一堆 .exe/.bunx（bun 生成的 shim）
运行中： PID 39012 .bin\dsh.exe --profile dsh-tui（占用 dsh.exe）
        PID 34372 node …\@deepseek-ai\dsh\lib\bin.js --profile dsh-tui
        PID 30480 node …\@deepseek-ai\dsh-subprocess-local\lib\runner.js
被占用： .bin/dsh.exe、@koromix/koffi-win32-x64/win32_x64/koffi.node（独占写打开失败）
用户配置：pm = "bun"（旧模板）、version = "latest"、auto-update = true、mode = "hybrid"
全局 dsh：不存在（npm/bun/pnpm 全局目录都没有）
PATH（由 dshl 注入给 dsh 的）：含 C:\Users\Frees\.cache\dshl\dsh\node_modules\.bin
```

→ 组合起来就是一条**必然触发**的失败链：`latest`(0.1.5-rc.2) ≠ 缓存(0.1.6-alpha.2) → 判定"需要安装" → 从正被执行的缓存里改文件 → 共享冲突 → 缓存不变 → 下次启动重复。

### A.5 命令形态验证（本机）

```
bun 1.4.2：`bun add --help` 里存在 `--cwd=<val>`          → prepare.rs:331-338 的命令形态成立
npm view @deepseek-ai/dsh version（--registry=npmmirror）→ 0.1.5-rc.2
                                                          → 与 prepare.rs:456-465 的目标计算一致，
                                                            也证实本机 V1 降级判定必然为 true
```

## 附录 B：未验证 / 推测清单（请勿当作结论）

1. ~~`nub view` 子命令是否存在~~ → **已实测存在**（本机 nub 0.8.3），连同 `nub node which|install` 一并确认；`pm = nub` 的问题只在 N1/N2。
2. npm/bun/nub 在 Windows 上更新 `node_modules` 时是"覆盖"还是"删除重建"，以及对应的具体错误码；重命名被映射的文件是否被允许【推测】。
3. pnpm 装出的树是否缺少 koffi 原生绑定（pnpm 10+ 默认阻止 build script）【推断，需实测】。同理：npm 执行 postinstall、bun 默认拦截 —— 同一个依赖树在不同 pm 下的产物是否真的等价【未验证】。
4. 中断安装后 tar/npm 的实际残留形态（`package.json` 与入口的写入顺序）【推测】。
5. `--no-save` 是否仍会写 `package-lock.json`【推测】。
6. fork 版 `webui-rs` 是否暴露 `Event::get_string`（专题五 P1 的前置）【未验证】。
7. `nub` 是否读取 `BUN_CONFIG_REGISTRY`（`mirror.rs:58` 把它注入了 nub/bun 共用路径）【推测】。
8. macOS 上安装子进程存活对缓存的实际破坏程度【推断】。
9. `nub` 写下的 `devEngines.packageManager` 类烙印是否会影响之后切换 pm【未验证】。

## 附录 C：看起来像 bug、其实是有意设计（改之前先改 ADR）

| 现象 | 依据 |
|---|---|
| `window_show` 10s 超时后返回 false、HTTP 409 `booting` | `.agents/notes/implemented/2026-08-23-cli-lock-scope-window-show.md` |
| 部分锁容忍中毒（`into_inner`）、部分不容忍 | `2026-08-23-lock-poison-policy.md` |
| 安装/下载类子进程不设超时 | `2026-08-23-nub-install-negative-cache.md`（网络策略表） |
| `probe` 非零退出时绝不挖版本号 | `probe.rs::tool_from_result` + 回归测试 |
| dsh 缓存不做版本隔离（`rm -rf dsh` 即清空） | `prepare.rs:203-212` 注释 |
| 从不强杀 dsh（只 Ctrl+C/SIGTERM，30s 等待） | `ui/launch.rs:216-225`、`AGENTS.md` |
| 第二个 dshl 只做"激活"不新开 | `single_instance.rs`、`README.md` |
| 镜像从不写全局配置 | `AGENTS.md:114`、`mirror.rs:1-5` |
| 时序测试的宽上限（30s/60s） | `.agents/notes/implemented/testing/2026-08-23-test-timing-budgets.md` |

---

## 附录 D：本次已实施的修复（2026-09-22）

全部改动按 P0 清单执行，另加用户要求的「Rust 侧定时更新检查」。门禁：`scripts/gate.ps1`
**GATE PASSED**（fmt / clippy -D warnings / test 71 passed / npm check / pack dry-run），
并额外做了真机端到端验证（见 D.3）。未动 git。

### D.1 修复清单

| # | 文件 | 改动 |
|---|---|---|
| 1 | `src/install/nub.rs` | registry 元数据 URL 补 `@`（新增纯函数 `latest_metadata_url` + 单测）；装配顺序改为「主包 shim 先、平台原生二进制后」（POSIX 上不再被 475 B 的 JS launcher 覆盖）；缓存命中判定改用 `bin_in_dir`；删掉不可达的重复快路径 |
| 2 | `src/platform/paths.rs` | 新增 `tool_in(name, extra_dirs)`：先搜运行时前缀再回退裸名；`tool()` 与它共用 `bare_tool` 兜底 |
| 3 | `src/flow/prepare.rs` | pm 二进制改经 `platform::tool_in(pm, &runtime.path_prefix())` 解析；`cache_needs_install(cached, target, allow_downgrade, entry_ok)` —— `latest` 派生目标不再降级（`allow_downgrade = !wants_latest()`），入口缺失即强制重装修复（`flow.prepare.cache_repair`）；`cached_version()` 暴露给更新检查；启动查询失败改用 `version_query_failed` 如实记日志，并回落 `update_check::recent_latest()`；`global_program_usable` 走有界 `run_bounded`；修正注释里的 5s→3s 与缓存路径 |
| 4 | `src/process/capture.rs` | 新增 `run_bounded(cmd, budget)`：带硬超时且 `kill_on_drop`，超时杀子进程 |
| 5 | `src/probe.rs` | 所有 `--version` 探测统一走 30s 有界（此前**无超时**，与 AGENTS.md 承诺不符）；`probe::nvm()` 的 Windows 分支接回退出码门（不再从崩溃输出挖版本） |
| 6 | `src/install/pnpm.rs` | 缓存/回退判定改 `bin_in_dir`（Windows 上 npm 装出来的是 `pnpm.cmd`，查 `.exe` 恒判失败）；`pnpm bin -g` 加 15s 上界；安装 pnpm 的 npm 经 `tool_in` 解析 |
| 7 | `src/install/bun.rs` | 缓存复用与 npm 回退判定改 `bin_in_dir`；npm 回退经 `tool_in("npm", &[node_dir])` 解析并注入 PATH（`ensure_bun` 新增 `node_dir` 参数） |
| 8 | `src/install/mod.rs` | 新增跨 pm 的 `bin_in_dir(dir, name)`（`.exe`/`.cmd`/`.bat`/裸名） |
| 9 | **`src/update_check.rs`（新）** | 后台更新检查：每 2h 一次（首次 5 分钟后），3s 有界查询 + 15s 有界全局壳探测；状态写 `progress::State::update`、控制面与日志；`recent_latest()`（6h TTL）作为启动兜底；`query_latest()` 成为启动路径与定时器的**单一实现**。**只检查不安装** |
| 10 | `src/progress.rs` | `State.update: Option<UpdateInfo>`（含 Rust 侧本地化的 `text`）；`set_update()`；`reset()` 刻意不清它 |
| 11 | `src/control.rs` | 新增控制面方法 `update-status`（零网络）与 `check-update`（3s 有界）+ 单测 |
| 12 | `src/flow/mod.rs` | 管线在启动 dsh 前把快照交给定时器（`update_check::configure`，幂等启动） |
| 13 | `crates/dshl-cli/src/lib.rs`、`crates/dshl/src/main.rs`、`src/i18n.rs` | 单实例分支不再静默：打印一行本地化提示（`cli.already_running`）；`i18n::translate()` 供跨 crate 取词 |
| 14 | `assets/app.js` | 配置表新增 `update` 行（文案由 Rust 给）；顺手修掉恒空的 `auto-mirror` 行（serde 产出的是 `auto-mirror`，前端原本读 `cfg.auto_mirror`） |
| 15 | `locales/{zh-CN,en}.yml` | 新增 `cli.already_running`、`update.*`（5 键）、`flow.prepare.cache_repair`；改写 `flow.prepare.version_query_failed`（原为死键，无参数版本）与 `install.nub.not_found`（说"通过 npm 安装"与实现不符） |
| 16 | `AGENTS.md`、`README.md` | 缓存路径 `<cache>/dshl` → `<cache>/dshl/dsh`；`Pm` 四值 + `tool_in` 解析约定；新增「更新检查」段；README 的 pm 列表补 nub、5s→3s、新增后台检查与"不降级/自愈"说明 |

### D.2 新增/更新的测试

`update_check.rs`（4）：节奏常量（2h、TTL>间隔）、`classify` 的 behind/equal/ahead/none 四态、
无上下文时 `check_once` 是纯 no-op（控制面测试依赖它不联网）、TTL 过期判定。
`flow/prepare.rs`（2）：`cache_needs_install_by_version` 覆盖新签名（含 `entry_ok=false` 强制修复）、
新增 `latest_never_downgrades_a_newer_cache`（现在正是真机状态：缓存 0.1.6-alpha.2 vs latest 0.1.5-rc.2）。
`install/nub.rs`（2）：`latest_metadata_url` 必须保留 `@`（回归锁）、`platform_package` 与宿主匹配。
`control.rs`（1）：`update-status` / `check-update` 分发（无网络）。

### D.3 真机端到端验证（临时探针，已删除）

用一个**仓库外**的临时 crate（复用仓库 target 目录）直接驱动新模块：

```
recent_latest before any check: None
latest=0.1.5-rc.2 current=Some("0.1.6-alpha.2") available=false ahead=true
status_json = {"ahead":true,"available":false,"checked":true,...,
               "text":"the local dsh 0.1.6-alpha.2 is newer than the registry's latest (0.1.5-rc.2); keeping it instead of downgrading"}
recent_latest after check: Some(0.1.5-rc.2)
progress.update field = "update":{"latest":"0.1.5-rc.2","current":"0.1.6-alpha.2","available":false,"ahead":true,...}
```

即：真实 registry 查询、真实缓存读取、`ahead` 分类、本地化文案、UI 状态字段、启动兜底全部按预期工作，
且**验证了降级被正确拦住**（这正是先前那条"被 lock"链的触发条件）。

### D.4 追加：启动器自更新（用户要求）与自身代码审查（2026-09-22 晚）

**新增：启动器自身更新**（`src/self_update.rs` + `[update]` 配置 + `build.rs` 版本注入）

- `[update] self = "off" | "notify"（默认）| "auto"`、`interval-hours = 6`；源为 GitHub Releases
  （`hibays/DSHL`，经 `mirrors.github`），资产 `dshl-<v>-<platform>.zip`。
- 安全：**必须**有 release 公布的 `sha256`（GitHub API 的 `assets[].digest`）才自动安装，否则拒绝并
  改走"打开下载页"；apply 前**复校**摘要；URL 进 shell 前做字符集校验；`staged.json` 的路径做越界拒绝。
- 替换：`swap_binary` 走"临时文件 → 改名就位"，失败回滚，`.old-*`/`.new-*` 残留下次启动清理；
  macOS `.app` 包与 cargo 构建树（`target/debug|release`）拒绝自动替换。
- **永不打断 dsh**：替换点在 `run_cli` 的单实例锁之后、任何子进程之前；新二进制下次启动生效。
- 版本单一来源：`build.rs` 注入 `DSHL_BUILD_VERSION`（CI 用 tag、本地用 `git describe`），
  `--version` / 控制面 `ping` / napi `ping` 统一读它 → **B12 已修**（`dshl --version` 现在报 0.2.22）。
- 真机端到端（一次性探针，已删）：以 `DSHL_VERSION=0.0.1` 构建，对真实 v0.2.22 release 完成
  查询 → 下载 → sha256 校验 → 解包 → 暂存 → 替换（探针 exe 变成 4,038,144 字节的正式二进制），
  全程 `DSHL_CACHE` 指向沙箱，未触碰用户已安装的 dshl 与正在运行的 dsh。
- 热替换（保 dsh 存活）的完整机制/取舍/否决备选：`.agents/notes/proposed/architecture/2026-09-22-dshl-hot-self-update.md`。

**审查后修正**（对抗性审查由独立子代理完成，逐条落地）：

| 级别 | 问题 | 处置 |
|---|---|---|
| M2 | 代码引用的热更新 ADR 不存在 | 已补 `proposed/architecture/2026-09-22-dshl-hot-self-update.md` |
| M3 | 注释称"无 sha256 即拒绝"，实现却放行 | 改为**缺 digest 直接拒绝**（`self_update.no_digest`） |
| M4 | apply 不复校摘要；marker 路径未净化 | apply 前复校 sha256；`staged_path` 拒绝绝对路径/`..`；备份名用解析后的版本 |
| M5 | `interval-hours = 0` 只在启动时生效 | `enabled` 进 Ctx，循环里每 tick 判断 |
| M6 | 修复安装绕过"不降级" | 新增 `repair_spec`：`target == "latest"` 时按缓存版本 pin，并加测试 |
| M7 | 三个 README 的控制面方法清单陈旧 | README.md / README_en.md / plugins/dshl-pipe/README.md 已同步 11 个方法 |
| S1/S2 | hybrid 的 current 语义错、未安装时谎称"最新" | hybrid 改为**全局优先**（= 实际运行的），新增 `update.not_installed` |
| S3 | 安装成功后状态不刷新 | 安装后调 `update_check::invalidate()` |
| S4 | `download-self-update` 无界 await 撞 15s 客户端超时 | 改为 spawn + 立即返回；`check-update` 注释如实写明 3s+15s |
| S7/S8 | swap 非原子、`.old-*` 只在下次 apply 时清理 | 临时文件 + rename 就位；无 marker 时也清理残留 |
| S9 | `self = "off"` 不阻止已暂存更新被应用 | `apply_staged(enabled)` 尊重开关 |
| S10 | `self_update.status_json` 的 `checked` 语义 | 与 `update_check` 对齐（"是否真的查到过"） |
| S11 | 远端 JSON 里的 URL 进 shell | `ensure_safe_url`（拒绝引号/反引号/`$`/反斜杠/空白），带单测 |
| S12 | dev 二进制可能被自更新覆盖 | `is_dev_path` 拒绝 `target/debug|release` |
| S14 | swap/校验/装配顺序零测试；一个空转测试 | 新增 `swap_*`、`sha256_*`、`is_dev_path`、`staged_path` 越界、`assemble_bin` 顺序、`repair_spec`、`ensure_safe_url`、`is_recent`（替换空转测试）等 20+ 用例 |
| S15 | 双语文档漂移 | README/README_en/AGENTS 已补 `[update]`、BUILD_VERSION、自更新段；本附录把 B12 标为已修 |

**未处置（有意留待下一轮）**：S5 的归因文案已改进但未区分"包管理器不可用"的具体日志；
S6 的哈希解析已收紧但未按固定行格式解析；S13 的 Windows 资源版本号（winresource 未设
`FileVersion`）未动；S4 的 `check-update` 仍是 3s+15s（已在注释与文档写明）；M1 的
`build.rs` 属**新增未跟踪文件**，提交时务必一并 stage（`Cargo.toml` 已显式 `build = "build.rs"`）。

### D.5 有意未做（留给下一轮）

- 安装前**占用预检**与安装**心跳**、显式"清理缓存并重装"入口、原子安装（P1）。
- macOS 的孤儿安装进程兜底（`capture.rs` 的 `not(any(windows, linux))` 仍是空分支）。
- dist-tag / 版本区间支持、`state.json` 状态层、配置 `discover()` 纯读化与字段级回落（P2）。
- `MirrorMode::Force` 仍是空实现；`flow.prepare.auto_update_off` 仍是死键（保留原文案）。
- 定时器**只检查不安装**是刻意决定：安装要写运行中 dsh 正在执行的目录，只有在启动管线里
  （dsh 尚未起来）才安全。若将来要"到点自动装"，需先做占用预检 + 原子替换。

