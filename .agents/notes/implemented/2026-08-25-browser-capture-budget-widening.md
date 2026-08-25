# 浏览器 pid 捕获预算扩到 40 次 ×2s 并随生命周期重构私有化

- 日期：2026-08-25
- 状态：implemented
- 类别：refactor

## 背景

2026-08-23 的捕获预算决策（见同目录 browser-capture-budget-reset）撰写时是
8 次 ×2s（≈16 s），计数器提为共享原子。70f970d 把浏览器生命周期收敛进
`ui/browser.rs` 时，该计数随五个旧 `state::BROWSER_*` 全局量一并私有化；
4c9b6d8 又放宽了上限。

## 决策

- 上限：`CAPTURE_ATTEMPTS_LIMIT = 40`（×2s 节流 ≈ 80 s）——外部浏览器冷启动
  可能远超 16 s（杀软扫描、profile 锁、低配机器），捕获必须在窗口最终出现时
  仍有余量；放弃日志一次性输出，不逐 tick 刷屏。
- 归属：`CAPTURE_ATTEMPTS` 计数与节流时间戳（`LAST_CAPTURE`，
  `capture_due()` 自 arm）都是 `ui/browser.rs` 模块私有态；托盘周期复位仍走
  `note_window_recreated`。
- 本篇取代前篇中的数字与归属描述；按「写新不改旧」惯例，前篇保留原样。
