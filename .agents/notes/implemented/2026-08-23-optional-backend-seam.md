# 可选 backend seam：ctx.get 习语 + 契约托管在 consumer

- 日期：2026-08-23
- 状态：implemented
- 类别：architecture
- 参考：deepseek-harness `packages/core/tools/src/index.ts` 的 approval 消费方式
  （`ctx.get('approval')` 机会式获取可选服务）

## 背景

`@dshl/native`（本地内核）与 `@dshl/pipe`（远程管道）是同一能力的两个**可选**
Provider——前者要求已安装 .node，后者要求 DSHL_CONTROL_URL。control 曾在模块
顶层 `require()` 二者静态取 backend：绑定冻结在求值期、生命周期所有权分裂
（control 与 pipe 各自 dispose 同一 client）、且无法表达「可能不存在」。

## 决策

1. **消费习语**：control 在 apply 内用 `ctx.get('dshlNativeBackend') /
   ctx.get('dshlPipeBackend') ?? null` 机会式获取（对齐源仓 approval 先例）；
   可选 seam **不进 inject**。禁止在 control 里 require 这两个包。
2. **native 无条件 provide**：`.node` 加载失败也注册服务，描述符带
   `{ backend: null, loadError }`——失败诊断经容器透传；能力可用性以
   `backend === 'native'` 标记。pipe 保持条件注册。
3. **契约托管在 consumer**：`plugins/dshl-control/src/backend-contract.js`
   定义 `TIERS`（native 全量 / pipe 远程子集）+ `checkBackend()` 折叠时校验
   （warn 不阻断）+ `resolveLaunchOptions` 白名单。放 consumer 侧是因为两个
   provider 都可选，谁拥有契约都会强迫对方依赖自己。

## 否决的备选

- 独立 `@dshl/backend-definition` 包：符合教科书三角色拆分，但当前两 tier
  差异有限且会新增发布矩阵成本。若漂移加剧再升格（契约文件整体迁出即可）。
- inject 声明两个 backend：强依赖语义会把「未安装」变成插件加载失败。

## 验证

- 行为测试脚手架：双后端选 native / native 破损降级 pipe + loadError 告警 /
  双缺全 501，三场景断言；
- `checkBackend` 单元断言：漂移点名到具体方法、pipe 子集合规合法；
- `resolveLaunchOptions` 过滤 snake_case 与未知键。

## 连带规则

- 新增/修改 backend 能力组时**必须同步 TIERS**，否则折叠层告警或静默漏检。
- launch 路由只允许 `LAUNCH_OPTION_KEYS` 白名单透传（napi 会静默丢弃未知键）。
