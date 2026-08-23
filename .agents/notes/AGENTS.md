# Agent Notes — 决策记录规则

Agent Note 是本仓的 ADR：记录设计决策的**动机、备选方案、后果与验证方式**。
它的存在理由：后续 agent 会话没有历史上下文，缺少记录时会把已权衡过的方案
当作疏漏「修」回去（真实案例：锁中毒策略、CLI_LOCK 生命周期、可选服务消费方式
都曾被反复重新提出）。

## 生命周期

```
proposed/  ──采纳并落地──▶  implemented/  ──被新决策取代──▶  archived/
```

- `implemented/<category>/YYYY-MM-DD-<slug>.md`：已生效的决策。category 取
  `architecture | bug-fix | feature | process | simplification | testing`。
- `proposed/architecture/…`：待讨论提案；写明备选与取舍，等决策后迁移。
- `archived/`：冻结快照，禁止编辑，不得作为现行依据。

## 写作要求

1. 每篇必须包含：**背景**（什么问题）、**决策**（选了什么）、**否决的备选**及原因、
   **验证方式**（哪个测试/场景守着它）。
2. 新 note 落笔前先检索既有 note：被取代的旧 note 在同一次改动中移入 `archived/`
   并在新 note 里交叉链接；部分取代则保持活跃并互链。
3. 一篇只讲一个决策；与代码不符的 note 视为待更新项，优先修订而非绕过。
