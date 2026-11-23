<!-- card:begin -->
**0.3.1 修复版**：修掉从 0.2.x 起就存在的「内置会话悬浮窗整块空白、屏幕上看不到」——窗口其实一直创建着，是悬浮栏入口模块的初始化顺序错误让页面在加载瞬间崩掉，异常又被 async 吞成静默失败。现已修复，并补了一道能真实复现该故障的回归测试。
<!-- card:end -->

# WorkBuddy Shift 0.3.1

发布日期：2026-11-23

这是 0.3 线的修复版，只修缺陷、不加新功能。数据目录、账号库、关联存储与备份格式都没有变，0.3.0 / 0.2.6 可以直接升级。

## 修复

### 内置会话悬浮窗看不到（#5）

- **现象**：启用悬浮窗后屏幕上没有任何悬浮栏；悬浮窗设置页能正常打开，但点「重置位置」「换头像样式」都像没反应；退出应用（含托盘）重进也无效。
- **根因**：悬浮栏入口 `companion-ui/src/desktop/rail.tsx` 在模块顶层就调用了挂载函数，而它依赖的一个绑定声明在调用之后。`boot()` 虽然写成 `async`，但在第一个 `await` 之前就会同步执行到 `stopAttention = ...`，撞上 TDZ 抛 `ReferenceError: Cannot access 'stopAttention' before initialization`；异常发生在 async 函数体里，被 `void boot(...)` 吞成 unhandled promise rejection——脚本其余部分照常跑完，React 从未挂载，而窗口本身是 `transparent(true)`，于是用户看到的是「完全没有悬浮窗」。
- **影响范围**：0.2.x – 0.3.0（由 2026-11-21 的 round5-09 引入）。网页演示页走的是另一条分支、不经过 `productionOptions()`，所以演示一直正常，只有桌面端中招；这也是它藏了两个版本没被发现的原因。
- **修复**：把挂载调用移到模块末尾，确保所有顶层绑定都已初始化，并写清为什么不能提前。

### 同类问题不会再静默复发

- **写法门禁**：`eslint.config.js` 只对两个入口文件（`rail.tsx`、`settings.tsx`）开启 `@typescript-eslint/no-use-before-define`，顶层「先用后声明」在 CI 就报错。不放开到全仓库——历史代码里这种写法很多，全量开启会成片报错。
- **行为门禁**：新增 `companion-ui/src/desktop/rail.smoke.test.tsx`（happy-dom + vitest），真实求值入口模块（不 mock `boot` / `productionOptions` / transport），断言 `#desktop-rail` 挂上了 React 子节点、且没有 unhandled rejection，并已接入 `frontend-tests` job。验证方式是把代码改回旧顺序——它确实会红在 `Cannot access 'stopAttention' before initialization`。

## 其它

- 统一文档里的 lint 口径：CI 实际执行 `--max-warnings=9`，`AGENTS.md` / `README.md` / `docs/DEVELOPMENT.md` 三处此前写作 10。
- `docs/companion.md` 增加「入口模块初始化顺序」一节，把这次的事故链路与两道门禁写进必读。

## 下载哪个？

| 你的系统 | 下载这个 | 说明 |
| --- | --- | --- |
| Windows 10 / 11（x64） | `workbuddy-shift_0.3.1_x64-setup.exe` | 安装包未做代码签名，SmartScreen 会拦一次：点「更多信息」→「仍要运行」 |
| macOS（Apple 芯片，M 系列） | `workbuddy-shift_0.3.1_aarch64.dmg` | 首次打开若提示「无法验证开发者」，Control + 点击应用 →「打开」 |
| macOS（Intel） | `workbuddy-shift_0.3.1_x86_64.dmg` | 同上 |
| Linux（通用） | `workbuddy-shift-linux-x86_64.AppImage` | 下载后 `chmod +x` 再运行 |
| Debian / Ubuntu | `workbuddy-shift-linux-x86_64.deb` | `sudo dpkg -i` 安装 |
| 不想装桌面版 | `npm i -g workbuddy-shift` | 需要 Node.js 20.19+ |
| 服务器 / NAS | `docker pull ghcr.io/magicapple123/workbuddy-shift:0.3.1` | 默认只绑 `127.0.0.1`，局域网访问要显式放开 |

**分不清 Mac 是哪种芯片**：左上角 Apple 菜单 →「关于本机」，看「芯片」一行——Apple M 系列选 `aarch64`，Intel 选 `x86_64`。

## 其余文件是什么？

| 文件 | 用途 |
| --- | --- |
| `*.sig` | 自动更新的签名文件，不要手动下载 |
| `latest*.json` | 客户端检查更新用的清单，不要手动下载 |
| `workbuddy-shift-macos-*.app.tar.gz` | macOS 自动更新包，供应用内更新器使用 |

> `latest-macos-*.json` 与 `latest-darwin-*.json` 内容相同：前者是生成器侧的可读名，
> 后者是 Tauri 在 macOS 上实际请求的名字，两者都必须保留。

## 升级说明

- **桌面版**：0.3.1 是 `latest`，0.3.0 / 0.2.6 用户可以在应用内「检查更新」直升；也可以手动下载对应安装包覆盖安装，配置与账号库会保留。
- **从 0.2.x 升上来的用户**：如果你此前一直觉得「悬浮窗开了但屏幕上看不到」，升级后不需要改任何设置，重启应用即可看到；位置保存在 `companion-rail-position.json`，位置异常时可以用设置页的「重置位置」。
- **npm 版**：`npm i -g workbuddy-shift`。
- **Docker**：`docker pull ghcr.io/magicapple123/workbuddy-shift:0.3.1`。
