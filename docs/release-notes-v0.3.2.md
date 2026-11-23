<!-- card:begin -->
**0.3.2**：修掉「应用还开着时覆盖安装，会弹 Error opening file for writing: companion-runtime.exe」——从这版起安装包会先结束那个常驻进程再装，开着应用也能正常覆盖。没有新功能，配置与账号都不受影响。
<!-- card:end -->

# WorkBuddy Shift 0.3.2

发布日期：2026-11-23

只修缺陷，不加功能。数据、账号、备份格式都没有变，0.3.0 / 0.3.1 可以直接升级。

## 修复了什么

**覆盖安装不再弹「写文件失败」**

应用还在运行时（托盘常驻也算）直接装新版，会弹出：

```
Error opening file for writing:
...\WorkBuddy Shift\companion-runtime.exe
```

只能选「忽略」继续，那个文件就停在旧版本。`companion-runtime.exe` 是内置悬浮窗的采集进程，一直被占用；而旧安装器只会关主程序，不会关它。

0.3.2 的安装包会在复制文件之前先结束这个进程，所以**即使应用开着也能正常覆盖安装**。

> 一点提醒：如果你之后回退去装 0.3.1 或更早版本的安装包，那些旧安装器没有这个处理，仍会弹一次。选「忽略」继续即可，不影响使用。

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

## 升级说明

- **桌面版**：应用内「检查更新」直升，或下载对应安装包覆盖安装；配置与账号库都会保留。
- **npm 版**：`npm i -g workbuddy-shift`。
- **Docker**：`docker pull ghcr.io/magicapple123/workbuddy-shift:0.3.2`。
