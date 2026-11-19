# WorkBuddy Shift v0.2.1

第三轮整改版本：自动更新链路、JetBrains 支持、统计口径与界面一致性。

## 主要变化

- **自动更新**：启用系统代理；检查与下载共用同一代理配置，只填不保存也能下载；
  检查 20 秒内返回结果，不再无限转圈；签名失败时引导手动下载一次。
- **更新发布**：Release 先以 draft 上传全部资产，成功后一次性切到 latest；
  清单中的资产 URL 固定到 tag，避免「检查到旧版、下载到新版」。
- **JetBrains**：新增关联会话与账号切换支持；有未识别的运行中 IDE 时拒绝写入并提示先退出。
- **统计**：Token 统计支持按账号筛选；积分官方用量失败时回退本地观察值并标注来源；
  Token 导出新增 xlsx，CSV 不再出现空列错位。
- **存储**：打开页面即预热总览，扫描更快；「工作目录」可以正确定位。
- **界面**：悬浮窗全界面深色适配；权限检测按钮在 Windows 可用；
  跨账号同步表格、账号卡水印与自动签到文案修正。

## 下载哪个？

| 系统 | 文件 |
| --- | --- |
| Windows 10/11（64 位） | workbuddy-shift_0.2.1_x64-setup.exe |
| macOS Apple Silicon（M1–M4） | workbuddy-shift_0.2.1_aarch64.dmg |
| macOS Intel | workbuddy-shift_0.2.1_x86_64.dmg |
| Linux Debian/Ubuntu | workbuddy-shift-linux-x86_64.deb |
| Linux 通用 | workbuddy-shift-linux-x86_64.AppImage |

## 其余文件是什么？

| 文件 | 用途 |
| --- | --- |
| *.sig | 自动更新的签名文件，不要手动下载 |
| latest*.json | 客户端检查更新用的清单，不要手动下载 |
| workbuddy-shift-macos-*.app.tar.gz | macOS 自动更新包，供应用内升级，无需手动安装 |

> `latest-macos-*.json` 与 `latest-darwin-*.json` 内容相同：前者是生成器侧的可读名，
> 后者是 Tauri 在 macOS 上实际请求的名字，两者都必须保留。

## 升级说明

从 0.2.0 起使用同一签名密钥，应用内自动更新可直接升级。若提示「签名校验失败」，
说明当前安装版本的可信公钥与新包不一致，请手动下载本版本安装一次，之后即可恢复自动更新。
