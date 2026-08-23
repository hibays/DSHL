# .agents — Agent 工作流

本目录承载 agent 协作的持久资产（参考 deepseek-harness 的组织方式，按本仓规模轻量化）：

- [`skills/`](skills/)：可复用流程。每个子目录一个 `SKILL.md`，frontmatter 的
  `description` 写明触发时机——agent 在遇到对应任务时应主动加载并遵循。
- [`notes/`](notes/AGENTS.md)：Agent Notes = 本仓的决策记录（ADR）。记录「为什么这样设计」，
  防止后续会话在不知情时把已权衡过的方案改回去。

与根 [AGENTS.md](../AGENTS.md) 的分工：根文件是**长期有效的硬规则**（结构、命令、约定）；
这里是**流程与方法**（怎么审查、怎么发布、为什么这么设计）。

## 约定

- Skill 命名：`dshl-<主题>`；`SKILL.md` frontmatter 必含 `name` 与触发用 `description`。
- Note 命名：`YYYY-MM-DD-<slug>.md`，置于 `implemented/`（提案态放 `proposed/`，
  废弃移 `archived/`）。写新 note 前先检索旧 note 是否被部分/全部取代并交叉链接。
- 语言：中文为主；skill 的 `description` 可中英并列以便匹配。
