---
name: dshl-code-review
description: Use when reviewing changes in the dsh-launcher repo — 本仓特有的审查检查点：时序测试红线、napi 边界、锁中毒策略边界、backend 契约同步、双语 locales，以及代码本身看不出来的历史决策。
---

# 审查 dsh-launcher 变更

**先读本仓规则**：根 [AGENTS.md](../../AGENTS.md) 与 [plugins/AGENTS.md](../../plugins/AGENTS.md)；
设计决策背景查 [.agents/notes/implemented/](../notes/implemented/)——与 note 冲突的写法
先当设计讨论，不要直接判缺陷。

## 本仓特有红线（代码正确也常被误报/误改）

1. **时序测试红线勿放宽**：`install/stream.rs::verbose_output_drain_completes_promptly`
   是 lost-wakeup 竞态回归测试；`flow/launch.rs` 多个 8s 上限断言同理。禁止加迭代次数
   或 sleep 时长。子进程测试一律用 `crate::testutil::shell(win, unix)` 选壳——
   手写 `COMSPEC` 运行时判断在 WSL 互操作下会选错（历史 bug）。
2. **锁中毒策略有明确边界**：见
   [lock-poison-policy](../notes/implemented/2026-08-23-lock-poison-policy.md)。
   只要求 napi 直达面与长持有锁容忍中毒；内部纯赋值短临界区的 `unwrap()` 不是缺陷，
   不要提议全仓统一。
3. **CLI_LOCK 只罩 setup**：见
   [cli-lock-scope-window-show](../notes/implemented/2026-08-23-cli-lock-scope-window-show.md)。
   不要建议恢复终身持有或重新引入 booting 旗标；`window_show` 的 409 `code:'booting'`
   是诚实反馈链的一部分。
4. **可选 backend seam 用 ctx.get**：见
   [optional-backend-seam](../notes/implemented/2026-08-23-optional-backend-seam.md)。
   禁止在 control 里 require @dshl/native|pipe；新增能力组必须同步
   `backend-contract.js::TIERS`；launch 路由只透传白名单键。
5. **napi 边界命名**：`#[napi(object)]` 字段导出为 camelCase（prependPath 曾因此静默
   失效）。JS 入参/出参键名必须对照 crates/dshl-native/src/types.rs 核对。

## 常规检查点

- **locales 双语同步**：改 `t!()` 消息必须同时落 `locales/en.yml` 与 `zh-CN.yml`；
  全角括号曾干扰占位符正则对比，逐键人工复核。
- **i18n 键使用**：用户可见消息走键而非裸英文串拼接（Error(String) 经 napi/HTTP 直达用户）。
- **文档即契约**：行为变更同 diff 更新所属 README/backend-contract 注释；README 中的
  协议/路由/默认值描述必须能在代码中指出出处。
- **门禁**：`scripts/gate.ps1|gate.sh`（fmt + clippy -D warnings + test --locked +
  npm check + pack dry-run）；跨平台验证可用 WSL bash 跑 Linux 侧（注意本机裸 `bash`
  可能是 WSL）。
- **git 纪律**：不 commit/add/stage——用户手动管理暂存区自行审查。
