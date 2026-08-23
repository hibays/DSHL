# CLI_LOCK 生命周期收窄：setup 后即释放，window_show 用同一把锁互斥

- 日期：2026-08-23
- 状态：implemented
- 类别：architecture
- 取代：「KERNEL_BOOTING 旗标 + window_show 轮询」方案（已回退删除）

## 背景

napi `windowShow()` 可在 `launch()` 返回后立刻落下，撞进内核线程正在执行的
`ui::setup`——二次并发 setup 会损坏 webui 单一全局态。曾引入 `KERNEL_BOOTING`
旗标让 `window_show` 轮询等待，但置位晚于线程 spawn 仍有调度缝隙，且与
`CLI_LOCK`、`RunHandle.started` 形成三套重叠防御。

## 决策

1. 内核线程对 `CLI_LOCK` 的持有**收窄到 reset_runtime_state → ui::setup 返回**
   （两条轨一致），`launch_flow/run_loop` 不再持锁——它们不做 setup。
2. `window_show()`（crates/dshl-cli/src/control.rs）改为
   `try_kernel_lock()` + 10s 有界等待：拿到锁才执行 show，**真互斥**而非
   「等旗清了再赌」；超时返回 `false`。
3. 删除 `KERNEL_BOOTING` 静态及其全部 store 点；`KernelCleanup` 继续负责
   panic 时复位 `started` 与 HANDLE 缓存。

## 否决的备选

- 保留旗标并前移置位：仍需轮询，且多一面要同步复位的旗。
- window_show 直接取 CLI_LOCK 阻塞等待：boot 期会无限等，需要超时逻辑，
  等价于 try_lock 循环但更绕。

## 验证

- 行为测试：双后端/native 破损降级/双缺三场景（见 plugins/dshl-control 测试脚手架）；
- workspace 测试 48/48（Windows 与 WSL Linux 双侧）；
- 注意：`ui::show()` 内部 `WINDOW_ID==0` 分支因此获得互斥保护，勿再叠加旗标。

## 已知取舍

setup 真实超过 10s 时 windowShow 返回 false，HTTP 路由以 409 `{code:'booting'}`
诚实上报（UI 本地化为「启动器仍在启动中」），不补开窗。
