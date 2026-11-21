# workbuddy-shift

WorkBuddy、CodeBuddy IDE、CodeBuddy CLI、VS Code CodeBuddy 插件与 JetBrains IDE 插件账号切换工具，在浏览器中操作（webui）。多账号共享登录态：一键切换各客户端账号，把会话以新 id 复制给其他账号并建立跨账号关联；同时汇总积分到期、官方请求用量、Token 消耗与模型限额。

同时提供桌面 App（Tauri）版本：界面一致，额外提供会话悬浮窗、开机自启与整包自动更新。

**在线演示**：[打开 GitHub Pages 演示](https://magicapple123.github.io/workbuddy-shift/)（只读演示；账号、积分与请求记录均为虚构数据，业务操作已禁用，另含可交互的会话悬浮栏演示）

## 快速开始

### npm 安装（webui）

```bash
npm i -g workbuddy-shift
workbuddy-shift              # 启动本地服务 + 自动打开浏览器
workbuddy-shift status       # 终端查看当前账号
```

支持 macOS（Apple Silicon / Intel）、Windows x64、Linux x64 与 Linux ARM64；Linux ARM64
只提供 npm / webui 形态（桌面安装包仍只有 x64）。

#### 备用渠道：GitHub Packages

同一版本也镜像在 GitHub Packages（scoped 包名，版本号与 npmjs 完全相同）：

```bash
npm login --registry=https://npm.pkg.github.com   # 需要 GitHub token，至少 read:packages
npm i -g @magicapple123/workbuddy-shift --registry=https://npm.pkg.github.com
```

`npm login` 只写入 token，不会建立 `@magicapple123` 的 registry 映射，所以安装时必须带
`--registry`（否则 scoped 包仍去 npmjs 找、必然 404）。长期使用可把 scope 映射写进
`~/.npmrc`（`npm config set @magicapple123:registry https://npm.pkg.github.com`），之后不必每次带。

为什么需要先登录：GitHub Packages 的 npm registry 即使对公开包也要求携带 token。
因此**主渠道仍是 npmjs 的 `workbuddy-shift`**，这条只在 npmjs 不可用时兜底；
`postinstall` 会依次尝试 npmjs 的平台包与 scoped 平台包，因此无论从哪个渠道安装都能拿到对应二进制。

webui 界面与桌面 App 一致，功能覆盖下方全部模块（会话悬浮窗与开机自启为桌面版专属）。

### 桌面 App

从 [GitHub Releases](https://github.com/magicapple123/workbuddy-shift/releases/latest) 下载 macOS / Windows / Linux 安装包（Tauri，推荐日常使用）。

> **macOS 提示「已损坏，无法打开」？** 未签名应用会触发隔离机制，在终端执行一次即可：
>
> ```bash
> xattr -rd com.apple.quarantine "/Applications/WorkBuddy Shift.app"
> ```

应用能启动但切换账号时提示无权限，请参阅下方 [macOS 权限说明](#macos-权限说明)。

## 功能

| 模块 | 说明 |
| --- | --- |
| 账号管理 | OAuth 扫码登录、备份导入导出、删除账号；可为账号添加备注并选择卡片上显示的字段 |
| 账号切换 | 一键切换 WorkBuddy、CodeBuddy IDE、CodeBuddy CLI、VS Code 插件与 JetBrains 插件账号，切换过程实时显示进度 |
| 会话复制 | 把当前账号勾选的会话以新 id 复制给目标账号：源账号数据不受影响，云端归属目标账号（WorkBuddy / CodeBuddy IDE / VS Code 插件） |
| 关联会话 | 复制过的会话自动建立跨账号关联；按客户端集中查看同一会话在各账号中的副本状态，支持增量同步、分歧处理与新增 / 解除关联 |
| 积分到期查询 | 自动查询每个账号的积分剩余量与到期时间；7 天内到期高亮，并按紧迫程度排序、标注「建议优先使用」 |
| 积分统计 | 汇总官方请求用量：总览、近 30 天趋势、账号消耗构成、按模型分类与请求明细 |
| Token 统计 | 按来源查看 Token 总览与趋势、构成占比、活跃热力图、项目与模型 Top 10、会话排行 |
| 模型限额 | 合并本机日志与客户端 hook 信号，显示「哪个账号的哪个模型被限流、官方给出的恢复时刻」；hook 可一键接入 / 卸载 |
| 自动签到 | 在设定的时间段内为允许的账号核验签到状态并补签，可按账号单独关闭 |
| 自动旅行 | 按官方状态自动派发或领取「派猫猫旅行」奖励，切换开关后立即生效 |
| 自动轮换 | 后台把积分最紧迫的账号设为 CodeBuddy CLI 的默认账号；检测到有 CLI 会话在运行时跳过本次切换并提示 |
| 外观与入口 | 浅色 / 深色 / 跟随系统；按客户端开关入口与状态轮询；可隐藏「关联会话」菜单 |
| 通知与日志 | 应用内提示存档（保留最近 100 条）；前端错误落盘，可在设置中打开日志目录 |
| 自动更新 | 桌面版从 GitHub Releases 检查新版本，整包更新经签名校验；npm 版本可用 `npm update -g workbuddy-shift` 升级 |
| 权限检测 | macOS「App 管理 / 完全磁盘访问」授权的引导与检测 |

## 使用

1. **添加与导出账号**：账号页 →「OAuth 扫码登录」「导入备份」；「导出」可将勾选账号备份为 JSON
2. **切换账号与账号信息**：账号卡片 →「切换」，可勾选复制当前会话；「账号信息」可添加备注，并选择卡片上显示账号名 / 手机号 / 备注
3. **查看积分与统计**：账号页自动查询各账号积分到期情况，点「刷新积分」手动更新；侧栏「积分统计」「Token 统计」查看用量明细
4. **切换各客户端账号**：CodeBuddy CLI、CodeBuddy IDE、VS Code 插件、JetBrains 插件均可在账号卡片一键切换；CodeBuddy IDE 与 VS Code 插件支持在弹窗中勾选复制当前账号的会话。CodeBuddy IDE 首次使用前需手动打开并登录一次
5. **管理关联会话**：侧栏「关联会话」按客户端查看同一会话在各账号中的副本状态，把新增内容增量同步到目标账号、处理内容分歧；「新增关联会话」可把会话复制到新账号并建立关联
6. **自动化**：设置 →「自动签到」「自动旅行」按官方状态补签与领取奖励；「CodeBuddy CLI 自动轮换」按积分紧迫程度自动设置默认账号
7. **限额监听**：设置 →「限额监听」接入 hook（CLI / WorkBuddy 实时上报），IDE 端走本地日志扫描

## macOS 权限说明

切换账号需要写入 WorkBuddy 认证文件，macOS 要求授权「App 管理」（或「完全磁盘访问」）：

1. 首次切换报「无权限」时，点「打开系统设置」
2. 优先在 **App 管理** 里打开 WorkBuddy Shift 开关；若没有，则去 **完全磁盘访问** 把 WorkBuddy Shift 拖进带箭头的框
3. 授权后重启本应用生效；设置页「权限检测」可随时验证

> webui 模式：由启动服务的终端进程权限决定；若终端已授权完全磁盘访问则无需额外操作。

## 许可

[MIT](./LICENSE)
