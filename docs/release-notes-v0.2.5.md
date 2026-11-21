<!-- card:begin -->
本次更新新增 Docker 部署形态：可以把账号切换与统计跑在 NAS / 家庭服务器上，用浏览器访问。

- 新增 Docker 镜像（amd64 / arm64）与 `docker-compose.yml`，一条命令起服务。
- 默认只发布到宿主机 `127.0.0.1`；局域网访问需显式开启，安全边界不会默认放宽。
- 支持反向代理终止 TLS（Nginx / Caddy），域名需写进 Host 白名单。
- 桌面版与 npm 版行为不变：默认监听、Host/Origin 校验口径与 0.2.4 完全一致。
<!-- card:end -->

# WorkBuddy Shift 0.2.5

发布日期：2026-11-22

本次更新只做一件事：给同一个内核加上**第三种形态**——Docker。桌面 App 与 npm / webui 版的功能与行为不变。

## 新功能

- **Docker 部署**：镜像 `ghcr.io/magicapple123/workbuddy-shift`（`linux/amd64` 与 `linux/arm64`），仓库根同时提供 `docker-compose.yml`。容器里跑的是与 npm 版相同的 headless webui（axum + 内嵌前端），适合 NAS、家庭服务器长期在线跑自动签到、自动旅行与统计。
- **数据一个挂载点**：容器内 `HOME=/data`，账号库、配置、限额台账、日志与备份都在 `/data/.workbuddy-shift`；要把客户端数据（`~/.workbuddy`、`~/.codebuddy`、VS Code / JetBrains 配置）挂进来时，按 `docs/docker.md` 的对照表追加即可。
- **默认最严、放开要显式**：镜像内进程绑 `0.0.0.0`（否则 Docker 端口映射进不来），但 compose 只把端口发布到宿主机 `127.0.0.1`。要局域网直连，必须同时改 `ports` 并设置 `WB_SHIFT_ALLOWED_HOSTS`，否则用域名或内网 IP 访问会收到 403。
- **支持反向代理终止 TLS**：配置了 Host 白名单后，服务会额外接受 `https://<同一个 Host>` 的 Origin（此前 HTTPS 反代会因「Origin 与 Host 不同源」导致所有写操作 403）。跨站 Origin 仍照旧拒绝，Host / Origin 校验没有关闭开关。

## 变更

- **新增环境变量 `WB_SHIFT_LISTEN`**：覆盖监听地址，接受 `0.0.0.0`、`0.0.0.0:57910`、`[::]:57910` 这类 IP 字面量。默认仍是 `127.0.0.1`；取值非法或显式为空时回落到默认地址并在日志里告警，不会启动失败。命令行 `--port` 优先于变量里的端口。
- **新增环境变量 `WB_SHIFT_ALLOWED_HOSTS`**：逗号分隔的 Host 白名单，默认空（只认回环）。不支持 `*` 通配，写了会被忽略并告警。
- **健康检查自适应端口**：镜像与 compose 的健康检查从 `WB_SHIFT_LISTEN` 推导探测端口，不再写死 57910；裸 IPv6 绑定（如 `::1`）按默认端口探测，并会先试 `127.0.0.1` 再试 `[::1]`。
- 新增 `docs/docker.md`，覆盖三种访问方式（SSH 隧道 / 局域网 / 反向代理）、数据卷对照表、UID/GID、环境变量、已知限制与故障排查。

## 已知限制（仅 Docker 形态）

- 容器看不到宿主机进程：客户端「运行中」判定恒为否，切换前的运行中检查与 CodeBuddy CLI 自动轮换的跳过逻辑会失真。
- 没有会话悬浮窗、托盘、开机自启与应用内自动更新；升级方式是 `docker pull` 后重建容器。
- Linux 容器没有 D-Bus / Secret Service，依赖系统密钥环解密 Safe Storage 的客户端切换会失败（与无桌面环境的 Linux 上 npm 版一致）。

## 升级说明

- 桌面版与 npm 版可直接在应用内升级，无需任何配置改动；默认监听地址与 Host/Origin 校验口径与 0.2.4 完全一致。
- 首次使用 Docker 形态：`docker compose up -d --build`，浏览器打开 `http://127.0.0.1:57910`。远程服务器建议用 `ssh -L 57910:127.0.0.1:57910` 隧道访问，不必放开监听。
