# 缓存 `.bin` 进了用户 PATH 也不算「全局 dsh」

- 日期：2026-09-29
- 状态：implemented
- 类别：bug-fix

## 背景

用户想让 `dsh` 在普通终端里可用，最自然的做法是把 dshl 缓存安装的
`<cache>/dshl/dsh/node_modules/.bin` 加进自己的 PATH。此后启动管线的「全局
dsh」探测（`platform::which("dsh")`，即 PATH 上第一个 `dsh`）命中的就是这个
缓存壳，于是：

1. 时间线谎报来源——把 dshl 自己管理的副本说成「全局」；
2. `dsh.mode = global` 的语义被架空：它承诺只用**用户自己**装的 dsh（缺失即
   报错），却会接受一份 dshl 建的缓存副本；
3. 归属该副本的版本 / 修复决策链被整条跳过（`cache_needs_install` /
   `repair_spec` / `entry_missing` 只在缓存分支里跑），缓存坏了也没人修；
4. probe 与 spawn 是两次独立的 `which` 解析，两者选到不同安装时会出现
   「探测说 0.1.0-rc.6、启动的却是别的壳」的割裂（见
   `2026-08-23-tray-open-navigate-and-stale-shim-gate.md` 的一半）。

## 决策

1. **排除自有缓存树**：`flow::prepare::dshl_cache_roots()` 列出 dshl 自己的
   缓存根（`<cache>/dshl`；`DSHL_CACHE` 搬走缓存时，默认位置的残留副本同样
   算自家的），`pick_global_candidate()` 沿 PATH 顺序跳过落在其中的候选——
   **继续往下找**，PATH 后面真有用户自装的 dsh 就照旧用它。
2. **跳过要说话**：每跳过一个候选打一条双语日志
   （`flow.prepare.global_skip_cache`），否则 `mode = global` 那条「请自行安装
   dsh」会显得莫名其妙。
3. **同源解析**：`probe_user_global_dsh()` 返回 `probe::Tool`，其 `path` 就是
   随后要 spawn 的程序；`run()` 用 `(global, program)` 一起决策，并用
   `let global = global && program.is_some()` 把两者绑成一个事实——原来的
   「probe 一次、spawn 前再 `which` 一次」以及随之而来的 `expect()` 一起删掉。
4. **后台更新检查复用同一判定**（`update_check::probe_global` →
   `prepare::probe_user_global_dsh`），否则两处对「当前跑的是哪个 dsh」又会
   各说各话。
5. 新增 `platform::which_all_in()`（列出全部候选，而非第一个）与
   `probe::dsh_at()`（探测指定路径）作为底座；`which_in()` 变成
   `which_all_in().next()`，语义不变。

## 否决的备选

- **按目录过滤 PATH**（把缓存目录从搜索目录里剔掉）：只能处理「目录一模一样」
  的拼写，junction / 符号链接 / 大小写变体就漏了；按文件判定可以对两侧做
  `canonicalize` 归一。
- **在 `hybrid` 里把命中缓存的「全局」当成缓存用**（保留判定、只改标签）：等于
  在探测层就默认了来源，`global` 模式仍然会接受缓存副本；不如在解析层就排除。
- **只加日志提示、不改判定**：那只是把误判写成可见的误判，模式语义依旧不成立。
- **让 `mode = global` 在只剩缓存副本时降级到缓存**：`global` 的定义就是
  「用户自己装的」，静默降级会让 `private` / `global` 的区别消失；现在它按
  设计报错，并已有一条日志说明为什么 PATH 上那个 dsh 不算数。

## 验证

- 单测：`flow::prepare::tests::global_candidate_skips_dshl_cache_copies`
  （缓存副本在真全局之前 → 取真全局、副本只报一次；只有副本 → 不认作全局）、
  `cache_roots_are_the_launcher_cache_tree`。
- 真机沙箱（本机 `mode = global`，假缓存 shim + 假全局 shim）：
  - 只有缓存 `.bin` 在 PATH：日志出现「…位于 dshl 自己的缓存里，不算全局安装，
    已跳过」，随后按设计报 `dsh.mode = global 需要你的 PATH 上已安装 dsh`，
    **没有**去跑那个缓存壳；
  - 缓存 `.bin` 在前、真全局在后：仍取真全局（`dsh 已安装：全局 9.9.9`），
    且被 spawn 的正是它（子进程输出带假全局的标记），跳过日志只出现一次。
- `cargo fmt` / `clippy -D warnings` / `test --workspace` 全绿。
