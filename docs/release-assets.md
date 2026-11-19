**下载哪个？**

| 系统 | 文件 |
| --- | --- |
| Windows 10/11（64 位） | workbuddy-shift_<ver>_x64-setup.exe |
| macOS Apple Silicon（M1–M4） | workbuddy-shift_<ver>_aarch64.dmg |
| macOS Intel | workbuddy-shift_<ver>_x86_64.dmg |
| Linux Debian/Ubuntu | workbuddy-shift-linux-x86_64.deb |
| Linux 通用 | workbuddy-shift-linux-x86_64.AppImage |

**其余文件是什么？**

| 文件 | 用途 |
| --- | --- |
| *.sig | 自动更新的签名文件，不要手动下载 |
| latest*.json | 客户端检查更新用的清单，不要手动下载 |
| workbuddy-shift-macos-*.app.tar.gz | macOS 自动更新包，供应用内升级，无需手动安装 |

> `latest-macos-*.json` 与 `latest-darwin-*.json` 内容相同：前者是生成器侧的可读名，
> 后者是 Tauri 在 macOS 上实际请求的名字，两者都必须保留。
