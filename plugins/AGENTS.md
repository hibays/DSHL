# plugins/AGENTS.md — JS 插件编写规则

本目录（`plugins/`）实现 harness 的**桌面能力 seam**，遵循 deepseek-harness 的
三角色模式：Service Definition / Provider / Consumer。参考源：
`D:\Belong\Case\deepseek-harness\packages\shell\*`。

## 角色分工

| 包 | 角色 | 说明 |
|---|---|---|
| `dshl-native` | Provider（FULL tier） | napi DLL 本地内核：window/tray/supervisor/terminal/actions/status 全量 |
| `dshl-pipe` | Provider（REMOTE tier） | 控制管道远程：actions 子集 + supervisor + status，无 window/tray/terminal |
| `dshl-control` | Consumer + 聚合 | 折叠两 Provider 为 `nativeCapabilities`；托管 Service 契约 `src/backend-contract.js` |

## 硬性约定

1. **可选服务消费用 `ctx.get(name)`，不进 inject、不 require 包**
   （决策记录：`.agents/notes/implemented/2026-08-23-optional-backend-seam.md`）。
2. **契约同步**：改 backend 能力组必须同步 `backend-contract.js::TIERS`；
   漂移会在 control 启动时 warn 点名到方法。
3. **launch 选项白名单**：经 `resolveLaunchOptions` 过滤——napi 会静默丢弃
   未声明键，snake_case 拼写错误零反馈。
4. **导出形态**：`export const name = '<包名>'`（与 cordis.patch.yml id 对齐）、
   `export const inject = [...]`、`export function apply(ctx)`。
5. **生命周期单一所有者**：谁创建的资源谁 dispose（pipe client 由 pipe 插件的
   effect 负责，control 不得重复）。
6. **napi 键名 camelCase**：JS 侧读写一律 camelCase（prependPath 曾因蛇形键静默
   失效）；对照表见 dshl-native/README.md。
7. **用户可见文本双语**：ui.js 内用 `T(zh, en)`；错误消息不得内嵌敏感值
   （控制 token 用 `//***@` 脱敏）。
8. **guard 语义**：自动 markHealthy（加载后 60s）+ dispose 时 markShutdown；
   gracefulExit 不计崩溃。改动阈值/窗口需同步 README 与 plugin-guard.js 常量。

## 验证

- `npm run check` + `npm pack --workspaces --dry-run`；
- 行为级验证用 mock ctx 捕获 handler 直调（参照本会话 route/guard 测试脚手架），
  或在真机 dsh bundle 中冒烟。
