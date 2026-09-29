# 启动器自更新：为什么是"下次启动替换"而不是进程热替换

- 日期：2026-09-22
- 状态：proposed（本 note 的 A 部分**已实现**；B 部分是待决策的热替换设计）
- 类别：architecture
- 相关：`src/self_update.rs`、`src/config.rs`（`[update]`）、`crates/dshl-cli/src/run.rs`、`docs/2026-09-22-dshl-audit.md`

## 背景

用户要的是「后台保留 dsh 进程，无感更新启动器，更新完恢复窗口状态，dsh 不动、后台任务继续跑」。
需要先分清两个"更新"：

1. **dsh 自身的更新**（`src/update_check.rs`）——后台 2h 检查，**只检查不安装**，安装仍在启动管线里做
   （安装会重写正在运行的 dsh 所执行的目录）。
2. **启动器自身的更新**（本 note）——替换 `dshl.exe` / 二进制本体。

## A. 已实现：检查 → 下载 → 校验 → 暂存 → 下次启动替换

- 配置 `[update] self = "off" | "notify"（默认）| "auto"`、`interval-hours = 6`。
- 源：GitHub Releases（`hibays/DSHL`，`releases/latest`），经 `mirrors.github` 前缀；资产名
  `dshl-<version>-<platform>.zip`，**必须**带 `assets[].digest` 的 sha256，否则拒绝自动安装并走手动路线。
- 暂存：`<cache>/dshl/update/{staged.json, staged/dshl[.exe]}`。
- 应用点：`run_cli` 里**单实例锁之后、任何子进程之前**（`crates/dshl-cli/src/run.rs`）——
  此时没有 dsh 在跑、没有窗口要重建，替换是安全的；替换后本进程继续跑旧代码，
  新二进制在**下一次启动**接管。
- 为什么不在运行中替换（即"为什么不是热更新"）见 B。

## B. 热替换（保留 dsh 存活）需要的机制——设计已明确，但未实现

若要让"启动器重启而 dsh 不断"，必须同时解决下面四件事，缺一不可：

1. **进程所有权**：Windows 上 dsh 属于启动器的 kill-on-close Job Object（`process/win_job.rs`），
   Linux 上子进程带 `PR_SET_PDEATHSIG`（`process/capture.rs`）。启动器一退出，dsh 就被杀。
   可行路径：交接前清除 job 的 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`（`SetInformationJobObject`），
   新进程再建一个新 job 并把已存在的 PID 加进去（Win8+ 嵌套 job）；Linux 则需要在 spawn 时
   就不设 PDEATHSIG（或改由一个常驻 keeper 持有）。
2. **控制端点连续性**：`DSHL_CONTROL_URL=dshl://<token>@127.0.0.1:<port>` 是**启动时注入 dsh 环境**的，
   启动器重启后 dsh 环境里还是旧 URL。要么把 port+token 持久化，让新进程绑定同一个端点，
   要么接受"重启后插件控制面失效直到 dsh 重启"。
3. **重新收养子进程**：`AsyncChild` 是 tokio child，**无法收养非亲生子进程**；而旧进程死后
   dsh 的 stdout/stderr 管道写端已断（写会失败）。要热替换，必须从一开始就把 dsh 的输出
   重定向到**日志文件**并 tail 它，而不是管道——这会动 `flow/launch.rs` + `process/child.rs`
   的全部流式逻辑（当前有一整套时序测试守着）。
4. **窗口恢复**：几何已经有 `window-state.json`；还需要持久化 dsh URL 以便重启后直接导航回去。

结论：热替换是一个**架构级改造**（监督传输从管道改文件 + job 交接 + 端点持久化），
不是"复制文件 + 重启自己"。因此本 note 只把 A 落地；B 待"离线切版本/无中断更新"成为硬需求时再做，
并且**必须**在沙箱（伪 dsh + 沙箱 cache）里端到端验证后才可开启。

## 否决的备选

- **运行中直接替换 exe 并 re-exec**：会杀掉 dsh（job/PDEATHSIG），且 re-exec 与单实例锁有竞态
  （新进程抢锁时旧进程还没退出，会被判 AlreadyRunning 直接退出）。已否决。
- **只做"提示 + 打开下载页"**：安全但不满足"有更新就下载、用户决定"的诉求，已作为
  自动路线失败时的回落（`self_update.manual` / 按钮 → 下载页）保留。
- **后台静默替换 + 不提示**：等于让用户在不知情时换二进制；且默认 `notify` 已足够省事。已否决。

## 验证

- 单测：`self_update` 的平台资产选择/无资产即非更新/tag 不可解析/`proxied`/`inside_app_bundle`/
  `is_dev_path`/`staged_path` 越界拒绝/`swap_binary` 成功与拒绝分支/`cleanup_old_binaries`/
  `sha256_of` 往返/空态 no-op；`config::update_*` 的默认与 clamp。
- 真机端到端（一次性探针，已删除）：以 `DSHL_VERSION=0.0.1` 构建，跑
  `check_once → download_once → apply_staged`，对真实 v0.2.22 release 完成
  查询→下载→sha256 校验→解包→暂存→替换（探针 exe 被替换成 4,038,144 字节的正式二进制），
  且全程 `DSHL_CACHE` 指向沙箱目录，未触碰用户已安装的 dshl 与正在运行的 dsh。
- 未验证：真正的"重启后生效"（需要一次真实启动器重启）；热替换（B）全部未实现。
