<!-- card:begin -->
**0.3.2 修复版**：应用还开着时直接覆盖安装，不再弹「Error opening file for writing: companion-runtime.exe」——安装器现在会先结束那个常驻子进程再复制文件。另把发版流水线等容器镜像的窗口从 20 分钟放宽到 75 分钟，不再每次发版都误报一条 docker 验收失败。
<!-- card:end -->

# WorkBuddy Shift 0.3.2

发布日期：2026-11-23

这是 0.3 线的第二个修复版，只修缺陷、不加新功能。数据目录、账号库、关联存储与备份格式都没有变，0.3.0 / 0.3.1 可以直接升级。

## 修复

### 覆盖安装时报 `companion-runtime.exe` 写文件失败

- **现象**：应用还在运行（含托盘常驻）时直接执行新版本安装包，会弹出
  ```
  Error opening file for writing:
  ...\WorkBuddy Shift\companion-runtime.exe
  ```
  只能选「忽略」继续，那一个文件就停在旧版本。
- **原因**：`companion-runtime.exe` 是内置会话悬浮窗的采集运行时，由主程序拉起后常驻。Tauri 的 NSIS 模板只会结束**主程序**，不会结束这个子进程，于是安装器复制同名文件时撞上共享冲突。
- **修复**：给 Windows 安装包挂上 `NSIS_HOOK_PREINSTALL` 钩子——在复制文件、写注册表、建快捷方式之前 `taskkill` 该进程，并等 500ms 让文件句柄释放。此时上游的「检查应用是否在运行」已经把主程序关掉，子进程不会被立刻重新拉起。
- **影响面**：0.3.1 及更早版本的安装包都会弹这个提示。0.3.1 恰好该文件与主程序功能一致，所以没有造成实际影响，但那是运气，不该依赖。

### 发版验收不再误报 docker 失败

- 发版流水线在派发「发版后验收」之前会等容器镜像推上去，但窗口只有 20 分钟，而多架构构建实测需要 55–61 分钟——于是每次发版都会先超时、再派发一次注定 docker 记 C 的验收（0.3.1 因此自动开过一条 `[release-smoke]` issue，还得人工重跑）。
- 现在窗口放宽到 **75 分钟**，并会跟随 `docker.yml` 的状态：它已结束且确实没有镜像时立刻告警派发，成功但镜像还没传播完则继续等。

## 工程改进（不影响使用）

- `companion-ui` 增加**设置页入口的冒烟测试**：真实加载 `desktop-settings.html` 的入口模块，断言表单挂上了 DOM 且没有 unhandled rejection。此前只有悬浮栏有行为级门禁。
- `update_service` 测试锁说明补全 `PACKAGE` 的三个写入口（`store_package` / `take_package` / `take_package_for_install`），避免后续新增用例漏锁。

## 下载哪个？

| 你的系统 | 下载这个 | 说明 |
| --- | --- | --- |
| Windows 10 / 11（x64） | `workbuddy-shift_0.3.2_x64-setup.exe` | 安装包未做代码签名，SmartScreen 会拦一次：点「更多信息」→「仍要运行」 |
| macOS（Apple 芯片，M 系列） | `workbuddy-shift_0.3.2_aarch64.dmg` | 首次打开若提示「无法验证开发者」，Control + 点击应用 →「打开」 |
| macOS（Intel） | `workbuddy-shift_0.3.2_x86_64.dmg` | 同上 |
| Linux（通用） | `workbuddy-shift-linux-x86_64.AppImage` | 下载后 `chmod +x` 再运行 |
| Debian / Ubuntu | `workbuddy-shift-linux-x86_64.deb` | `sudo dpkg -i` 安装 |
| 不想装桌面版 | `npm i -g workbuddy-shift` | 需要 Node.js 20.19+ |
| 服务器 / NAS | `docker pull ghcr.io/magicapple123/workbuddy-shift:0.3.2` | 默认只绑 `127.0.0.1`，局域网访问要显式放开 |

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

- **桌面版**：0.3.2 是 `latest`，0.3.0 / 0.3.1 用户可以在应用内「检查更新」直升；也可以手动下载对应安装包覆盖安装，配置与账号库会保留。
- **覆盖安装不再需要手动退出**：钩子在**安装包**里，所以用 0.3.2 的安装包覆盖安装时，安装器会自己结束 `companion-runtime.exe`，即使应用正开着也不会再弹写文件失败。反过来说，如果你之后回退去装 0.3.1 或更早版本的安装包，那些旧安装器没有这个钩子，仍会弹一次。
- **npm 版**：`npm i -g workbuddy-shift`。
- **Docker**：`docker pull ghcr.io/magicapple123/workbuddy-shift:0.3.2`。
