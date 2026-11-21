<!-- card:begin -->
本次更新给发版流程加了一道**自动验收**：每次正式版发布后，自动在真实环境里装上一版、升级、启动，把结果写回 Release 正文。

- 三端桌面（Windows / macOS / Linux）+ npm + Docker 五个形态全覆盖。
- 结论直接写在 Release 正文（矩阵 + 日志链接），失败按 A–J 归类，只做有限自动处置（不会擅自改代码或撤版本）。
- 用户侧无行为变化：默认启动路径、监听地址、数据目录与 0.2.5 完全一致。
<!-- card:end -->

# WorkBuddy Shift 0.2.6

发布日期：2026-11-22

本次更新只动**发布流程**，不改产品功能：桌面 / npm / Docker 三个形态的行为与 0.2.5 一致。

## 新功能

- **发版后自动验收（release-smoke）**：`release.published` 触发，7 个 job 并行跑完再汇总。
  - **Windows**：装 0.2.4 → 触发应用内自更新 → 断言装完版本、自更新日志阶段序列、启动三件套（进程存活 / 单实例 / 沙箱落盘）。
  - **macOS**：dmg 覆盖升级（arm64 为主目标，x86_64 只记录不判定）。
  - **Linux**：deb 升级 + AppImage 同路径替换（sha256 对齐 Release 资产），xvfb 下验证启动。
  - **npm**：官方源安装 → 0.2.4→0.2.5 升级 → `version` / `serve` / `/api/status`。
  - **Docker**：双 tag digest 一致、healthy、非 root（uid 1000）、卷可写。
- **结论写回 Release 正文**：`<!-- release-smoke:begin/end -->` 幂等区块，含矩阵（安装/升级/启动/版本/serve/api/pull/health/nonroot/volume）、跨文件版本一致性断言、run 与证据链接。失败首行恒为 `**结论：FAIL**`；`⛔` 表示取不到证据（不计入结论），`⏭` 表示不适用，均单独汇总。
- **有限自动处置**：按 A–J 归类，只允许三类动作（重跑验收 job / 派发 Docker 构建 / 重跑 build 的 release job）；单次 run 最多 3 项、每类每版本最多 1 次、attempt ≥ 2 一律不重跑。其余情况开 issue（同 tag 合并，不重复开）。

## 变更

- 新增隐藏启动参数 `--smoke-self-update`（配合环境变量 `WB_SHIFT_SMOKE_SELF_UPDATE=1` 与 `WB_SHIFT_SMOKE_LOG`）。**三重闸门**：参数、环境变量、非截图演示模式；任一不满足立即返回，默认启动路径零行为变化。
- 新增 `docs/release-smoke.md`（怎么读结论、A–J 类别、处置闸门、演练方式、已知限制）。
- `scripts/check-structure.mjs` 的长文件检查纳入 `.py`，6 个脚本如实登记。

## 已知限制

- **应用内升级的自验有一版延迟**：验收依赖被测版本的二进制里带 `--smoke-self-update` 钩子。0.2.6 是第一个带钩子的版本，它的上一版（0.2.5）没有，所以 **0.2.6 的 Windows 升级格会标 `⏭`**；从 0.2.7 起（上一版 0.2.6 带钩子）才真正验证升级。
- **托管 runner 没有桌面会话**：Linux 在 xvfb 下若 WebKitGTK 起不来，启动格记 `⛔`（不计入结论），需人工复核；Windows / macOS 的 runner 有交互式会话，不受影响。
- **历史 tag 回填**：更新器端点固定 `releases/latest`，回填旧 tag 时升级格会标 `⏭`（无法定向升级到指定版本）。
- 演练（`dry_run=true`）会真跑全部验收、**不**重跑、**不**开 issue，但会写一次 Release 正文的结论区块（幂等，可手动删除）。

## 升级说明

- 桌面版可应用内升级；npm 版 `npm i -g workbuddy-shift`；Docker `docker pull` 后重建容器。
- 无配置迁移；默认监听、Host/Origin 校验、数据目录与 0.2.5 一致。
