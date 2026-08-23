# 锁中毒加固边界：只加固 napi 直达面与长持有锁

- 日期：2026-08-23
- 状态：implemented
- 类别：architecture

## 背景

仓库内 `Mutex` 的访问策略曾不一致：部分用 `.lock().unwrap()`（中毒即 panic），
部分用 `.lock().unwrap_or_else(|p| p.into_inner())`（容忍中毒）。审查时曾提议
把全仓 ~30 处 unwrap 统一改成容忍式。

## 决策

**按暴露面划界，不做全仓统一**：

- 必须容忍中毒：napi 直达面（panic 会 abort 宿主 Node 进程）——`pty::sessions()`、
  `WS_SERVER`、`run.rs HANDLE/CLI_LOCK`、`kernel.rs RUN_HANDLE`；以及长持有锁
  （持锁窗口横跨外部代码，panic 落在窗口内的概率不可忽略）。
- 保持 `unwrap()`：临界区为**纯赋值短段**的内部锁（`progress.rs STATE`、
  `control.rs LAST_RUNTIME_PATH`、`child.rs inner 字段锁` 等）。panic 不可能
  发生在持锁窗口内，中毒概率≈0；翻 30 个点只产生噪声。

## 否决的备选

- 全仓统一 into_inner：改动面大、可读性下降，且未改变任何真实风险。

## 验证

- `pty/server.rs::tests`、既有 workspace 测试全绿；
- 新增锁时按此边界归类（写新锁先问：这个 Mutex 会被 napi 直接触达吗？
  持锁窗口内有外部代码吗？）。
