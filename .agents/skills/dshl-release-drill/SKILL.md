---
name: dshl-release-drill
description: Use before/when cutting a release tag (v*) in the dsh-launcher repo — 发布前置检查单、双轨发布链路说明、无 NPM_TOKEN 的跳过语义、部分失败的补发方式与已知幂等坑。
---

# dshl 发布演练与检查单（tag v*）

## 触发链路（一次 tag push）

```
release.yml          → 7 腿安装器 + GitHub Release（GITHUB_TOKEN 自动鉴权，无需 secret）
release-native.yml   → 6 腿并行：构建 .node → stage 子包 → npm publish 各自子包
release-plugins.yml  → workflow_run 等 native 全绿后：
                       bump-versions.mjs 对齐版本 → native → pipe → control
```

## 发布前检查单

1. **workflows 已在默认分支**：`workflow_run` 只认 main 上的定义。首个发布 tag
   必须打在工作流合入 main 之后的提交上，否则聚合三包静默不发。
2. **tag 形如 `v<semver>`**：版本号取 `${GITHUB_REF_NAME#v}`；非法 semver 会被
   npm 拒绝。
3. **NPM_TOKEN secret**：未配置时所有 publish 步骤自判跳过（exit 0，`::notice`
   留痕）——这是**正常态不是失败**；GitHub Release 部分照常产出（用的是自动
   GITHUB_TOKEN，见根 AGENTS.md）。
4. **npm-publish environment** 若配了 required reviewers：六腿各自需要人工批准。
5. **先演练**：推一个 `v0.0.0-test.1` pre-release tag 走全链，确认后再正式发。
6. **本地预检**：跑 `scripts/package.ps1|package.sh --no-installer` 验证构建，
   `scripts/publish.* -DryRun` 验证聚合包 bump/pack。

## 失败补救

- 某腿 publish 失败：**只 re-run 失败的腿**（每腿只发自己的子包，互不影响）。
- 不要 re-run 整个 workflow：已发布的包会撞 `EPUBLISHCONFLICT`（已知未修的
  幂等缺口——修复方向是 publish 前 `npm view <pkg>@$VERSION` 存在即跳过）。
- 聚合包被 guard 卡住：确认六腿 conclusion 全 success 后重跑 plugins 工作流。

## 已知取舍（勿当缺陷上报）

- 无 token 时聚合包的 bump/pack 仍会执行（本地 package.json 版本对齐，无副作用），
  仅真正触网的 publish 被跳过。
- 六个平台子包无法在本地单机完整演练（需逐平台构建），首次真实发布前建议用
  pre-release tag 全链验证一次。
