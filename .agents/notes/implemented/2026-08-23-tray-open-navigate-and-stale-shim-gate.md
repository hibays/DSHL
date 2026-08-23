# 托盘「打开 dsh」改为导航现有窗口 + 全局壳失效验证门

- 日期：2026-08-23
- 状态：implemented
- 类别：bug-fix

## 背景

托盘菜单「打开 dsh」原实现无条件调用 `platform::open_url`——用系统默认浏览器
**新开一个窗口/标签**。浏览器模式下这直接违背模式本身的意义：已生成的那个浏
览器窗口就是 dsh 的前端，再开一个默认浏览器实例等于出现两个互不相干的入口。
此外 hybrid 判定"用全局"时信任 `which("dsh")` 命中的第一个程序，但 PATH 上可能
残留旧包管理器（如 bun）安装的失效壳——壳内写死的 bin.js 路径早已不存在，
启动即 MODULE_NOT_FOUND；而探测 `--version` 可能被 PATH 上另一个可用安装应答，
造成「探测成功、启动失败」的割裂。

## 决策

1. **导航优先**：托盘打开 dsh 时按窗口状态路由——
   - `WINDOW_ID != 0`：`navigate_when_connected(&url)` 导航现有窗口
     （WebView 立即导航；浏览器模式等待连接后导航）；
   - `WINDOW_ID == 0`（无活窗）：`restore_from_tray(false)` 重建并回跳 dsh URL；
   - 不存在任何 open_url 兜底路径。
2. **全局壳失效验证门**：hybrid 选定全局程序后先跑一次
   `global_program_usable()`（`--version` 15s 内成功退出才算可用）；失败则双语
   日志提示「全局 dsh 已失效（残留壳）」并翻转 global=false，走缓存分支自愈
   （重装全新副本并运行其真实 node 入口）。

## 否决的备选

- 保留 open_url 作为兜底：任何兜底都会在 browser 模式产生第二个浏览器实例，
  与模式语义冲突；删除比保留更符合预期。
- 仅修 launch 不修 probe：probe 与 launch 解析到不同程序才是割裂根源的一半，
  两处必须同源（都走 which 语义），验证门放在使用点。

## 验证

- clippy -D warnings、workspace 测试全绿；
- 真机场景（PATH 含 bun 残留壳）由仓库主人复测：第二次及以后的托盘恢复、
  托盘打开 dsh 应导航现有窗口而非新开实例。
