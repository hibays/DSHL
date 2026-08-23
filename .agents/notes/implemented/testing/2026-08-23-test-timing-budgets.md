# 测试时序预算改为「宽挂起上限」：HANG_CEILING=60s / drain 30s

- 日期：2026-08-23
- 状态：implemented
- 类别：testing

## 背景

flow/launch.rs 与 install/stream.rs 的多个时序断言用秒级预算（8s 挂起上限、5s
drain），在共享 windows-11-arm runner 的 CI 负载下成批假失败（实测 >8s、drain
>5s 均出现）；本机 x64 从未复现。

## 决策

这些断言守卫的失效模式是**永久挂起**（lost-wakeup park、reaper 卡死）——任何
有限上限都能完整检测。因此：

- flow/launch.rs 四处 8s 挂起上限统一为 `HANG_CEILING = 60s`（#[cfg(test)]，
  必须保持 < URL_TIMEOUT=120s 以便真挂起快速失败）。
- install/stream.rs drain 外层 5s → 30s；powershell 生产者去掉逐行 Start-Sleep
  （ARM64 上每拍 ~35ms，150 行直接爆预算；快速连发同样逐行触发 notify wakeup）。

## 否决的备选

- 按 runner 类型缩放预算：环境探测脆弱且掩盖真实回归。
- CI 上 #[ignore] 时序组：失去挂起类回归的 CI 防线。

## 验证

- 本机 x64 与 windows-11-arm CI 全绿；
- 挂起检测力不变：任何无限期 park 都会撞 60s/30s 上限。

## 规则

不要把 HANG_CEILING/drain 预算收紧回秒级；也不要给 powershell 生产者重新引入
逐行 sleep。新增同类测试直接复用 HANG_CEILING。
