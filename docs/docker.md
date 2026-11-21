# Docker 部署

本页只讲**容器形态**的部署、数据与安全。分层与模块职责见 [ARCHITECTURE.md](ARCHITECTURE.md)，
本机开发命令见 [DEVELOPMENT.md](DEVELOPMENT.md)，CLI 子命令见 [cli.md](cli.md)。

## 定位：三种形态的差异

同一份 `wb-shift-core` 有三种宿主，Docker 形态只是其中「headless webui」那一种的打包方式
（见 [ARCHITECTURE.md](ARCHITECTURE.md#三宿主与依赖方向)）。它跑的是
`workbuddy-shift serve --no-open`，**没有 Tauri 桌面壳**。

| | 桌面 App | npm / webui | Docker（本页） |
| --- | --- | --- | --- |
| 界面 | 原生窗口 + 托盘 | 浏览器打开 `127.0.0.1:57910` | 浏览器打开公布的端口 |
| 会话悬浮窗 | 有 | 无 | 无 |
| 开机自启 | 有 | 无 | 靠容器 `restart` 策略 |
| 应用内自动更新 | 有 | 无 | 无（升级 = `docker pull`） |
| 能看到的客户端 | 宿主机全部 | 宿主机全部 | **只有挂进来的目录**（容器看不到宿主机进程） |
| 数据位置 | `~/.workbuddy-shift` | 同左 | `/data/.workbuddy-shift`（卷） |
| 典型用途 | 日常使用 | 临时/远程终端 | NAS、家庭服务器、长期在线的后台自动化 |

一句话：Docker 形态适合「7×24 跑自动签到 / 自动轮换 / 统计数据，人从别的机器用浏览器看」，
**不适合**「在这台机器上切换本机客户端的账号」——容器看不到宿主机的进程与客户端目录。

## 快速开始

### 方式一：`docker run`（最短路径）

```bash
docker run -d \
  --name workbuddy-shift \
  --restart unless-stopped \
  -p 127.0.0.1:57910:57910 \
  -v "$PWD/data:/data" \
  -e TZ=Asia/Shanghai \
  ghcr.io/magicapple123/workbuddy-shift:latest
```

浏览器打开 <http://127.0.0.1:57910>。

> 镜像随 `v*` 发版推送到 GHCR；尚未发布、或想跑当前仓库的改动时，用
> `docker compose up -d --build`（或见文末「本地构建与多架构」自行 `docker build`）。

### 方式二：compose（推荐，仓库根已带 `docker-compose.yml`）

```bash
docker compose up -d              # 用 ghcr 上的 latest
docker compose up -d --build      # 用当前源码本地构建
docker compose logs -f            # 看启动横幅与后台任务日志
docker compose down               # 停止（数据保留在 ./data）
```

`docker-compose.yml` 已经写好默认的安全姿势：只发布到 `127.0.0.1`、非 root 运行、
`no-new-privileges`、健康检查（探测端口跟随 `WB_SHIFT_LISTEN`，不写死）、`./data` 卷。

### 首次使用

1. 打开 webui，用「扫码登录」添加账号（登录态写在 `/data/.workbuddy-shift/accounts.json`）；
2. 设置页按需开启自动签到 / 自动旅行 / 自动轮换；
3. `docker compose restart` 后确认配置与账号还在（数据在卷里，容器可随时重建）。

## 三种访问方式

容器内进程监听 `0.0.0.0:57910`（否则端口映射进不来），但**默认只发布到宿主机回环**。
按安全性从高到低选一种：

### 1. SSH 隧道（推荐给单机远程使用）

```bash
# 在远程主机上（compose 默认配置即可，无需改任何环境变量）
ssh -N -L 57910:127.0.0.1:57910 user@nas.local
# 本地浏览器打开 http://127.0.0.1:57910
```

此时浏览器发的 `Host` 就是 `127.0.0.1:57910`，落在默认白名单里，不需要配置任何域名。

### 2. 局域网直连（需要两个开关）

```yaml
# docker-compose.yml
ports:
  - "57910:57910"                                  # 改这里 = 发布到 0.0.0.0
environment:
  WB_SHIFT_ALLOWED_HOSTS: nas.local,192.168.1.10   # 加这里 = 放行这些 Host
```

- `WB_SHIFT_ALLOWED_HOSTS` 是**逗号分隔的主机名**（可带端口，比对时只比主机名、大小写不敏感）；
- 它是回环白名单的**并集**：`127.0.0.1` / `localhost` 仍然可用；
- **不支持通配符**（写 `*` 会被忽略并在日志里警告）——域名就是这道边界的全部内容，
  一个 `*` 等于把边界拆掉；
- 访问内网 IP 时 `Host` 是 `192.168.1.10:57910`，所以要么把它也写进白名单，要么用主机名访问。

> 服务没有登录鉴权。发布到局域网等于「同一网段的任何人都能看你的账号列表、切号、删数据」。
> 只在可信网络里这么做，或者直接看下一种。

### 3. 反向代理（对外提供服务时）

Nginx / Caddy / Traefik / 群晖反代都适用，**必须**满足两点：

1. **保留原始 `Host`**：`proxy_set_header Host $host;`（Nginx）。反代把 Host 改写成
   `127.0.0.1:57910` 也不会出错（那本来就在白名单里），但页面里的同源判定会以它为准；
2. **把对外域名写进 `WB_SHIFT_ALLOWED_HOSTS`**，否则 Host 校验直接 403。

```nginx
location / {
    proxy_pass http://127.0.0.1:57910;
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
}
```

Caddy 默认就会保留 Host 并自动签证书，配置只需 `reverse_proxy 127.0.0.1:57910`。

关于 HTTPS：TLS 通常在反代终止，浏览器页面是 `https://…`，因此 `Origin` 请求头也带
`https://`，而服务本身只出 http。**只要配置了 `WB_SHIFT_ALLOWED_HOSTS`**，服务会额外接受
`https://<同一个 Host>` 的 Origin（仍要求主机完全一致，跨站照旧拒绝）；没有配置时口径与
改造前完全相同，只认 `http://<Host>`。

反代前面一定要再加一层认证（Basic Auth / OAuth2 Proxy / 只在内网可达）。本服务本身
不提供登录。

## 数据卷对照表

容器里只有一个挂载点也能跑起来：`/data`（即 `HOME`）。账号库、配置、日志、备份默认都在
`/data/.workbuddy-shift`。要让**客户端相关功能**可用，再把对应目录挂进来。

| 容器内路径 | 宿主对应 | 内容 | 不挂会怎样 |
| --- | --- | --- | --- |
| `/data/.workbuddy-shift` | `~/.workbuddy-shift` | **工具数据**：账号库 `accounts.json`、备份、签到/旅行/轮换配置与日志、限额台账、通知存档、错误日志 | 容器重建即全丢，必须挂 |
| `/data/.workbuddy` | `~/.workbuddy` | WorkBuddy 国内版登录态与会话库 | 无法读取/切换 WorkBuddy、无法复制会话 |
| `/data/.workbuddy-ai` | `~/.workbuddy-ai` | WorkBuddy 国际版同上 | 国际版相关功能不可用 |
| `/data/.codebuddy` | `~/.codebuddy` | CodeBuddy CLI 配置（默认账号等） | CLI 账号切换与自动轮换不可用 |
| `/data/.codebuddy-rotate` | `~/.codebuddy-rotate` | CLI 轮换状态与 helper 脚本 | 自动轮换记录与 helper 不可用 |
| `/data/.config/Code` | `~/.config/Code`（Linux） | VS Code 及其插件（CodeBuddy 扩展）配置 | 扩展账号切换 / 会话复制不可用 |
| `/data/.local/share/CodeBuddyExtension` | `~/.local/share/CodeBuddyExtension` | CodeBuddy 插件宿主共享数据（会话树、日志） | IDE 会话列表与限额来源缺失 |
| `/data/.config/JetBrains` | `~/.config/JetBrains` | JetBrains 系 IDE 配置（账号与插件设置） | JetBrains 端切换不可用 |
| `/data/.config/Google` | `~/.config/Google` | Android Studio（JetBrains 第二个配置根） | Android Studio 端切换不可用 |

两种挂法，按目标选：

- **只挂工具数据**（`./data:/data`）：账号库、积分/用量统计、自动签到、自动旅行、
  **自动轮换的决策**都能用；但凡是需要读写客户端目录或判断客户端是否在跑的功能都会失灵
  （见下面「已知限制」）。
- **整家目录挂载**（`-v /home/user:/data`）：把宿主机的家目录整个挂进容器，客户端目录
  自动就位，功能覆盖最全。代价是容器对家目录有写权限——**只在自己可控的机器上这么做**，
  并且记得 `user:` 用宿主机的 uid/gid，否则会出现 root 拥有的配置文件。

> 挂宿主机的家目录不会让容器看到宿主机的**进程**。`ps` 只看得到容器自己的 PID 命名空间。

## UID / GID 与权限

镜像里的运行用户是 `app`，uid/gid **固定 1000**，`HOME=/data`。这与多数桌面发行版的首个
普通用户一致，因此 `./data` 归你所有时不需要任何额外操作。

宿主机用户不是 1000 时，用环境变量对齐（不要用 `chown` 去改宿主目录）：

```bash
PUID=$(id -u) PGID=$(id -g) docker compose up -d
```

`docker-compose.yml` 里的 `user: "${PUID:-1000}:${PGID:-1000}"` 就是干这个的。

- **不要**用 `chown -R` 去「修」`./data`：如果里面已有真实客户端数据（整家目录挂载那种），
  一次递归 chown 会改掉你所有个人文件的属主，且不可逆；
- 出现 `EACCES` 时先看 `ls -ln ./data` 的 uid/gid，再用 `PUID`/`PGID` 对齐，而不是放宽权限；
- 需要 root 的场景（例如想读别的用户的目录）请改用 `user: "0:0"` 并接受「数据文件归 root」的
  后果——这属于显式例外，不要写进默认配置。

## 环境变量

| 变量 | 默认值 | 作用与安全含义 |
| --- | --- | --- |
| `WB_SHIFT_LISTEN` | 镜像内 `0.0.0.0:57910`；**代码默认 `127.0.0.1`** | 监听地址。可写 `0.0.0.0`、`0.0.0.0:57910`、`[::]:57910`、`192.168.1.10:57910`。只接受 IP 字面量；非法值或**显式空值**（`-e WB_SHIFT_LISTEN=`）**回落 127.0.0.1** 并在日志里警告，不会启动失败。端口优先级：命令行 `--port` > 这里的端口 > 57910。**健康检查的探测端口只从这里推**，用 `--port` 改端口时两者要一起改；裸 IPv6（如 `::1`）没有端口概念，健康检查按默认 57910 探测，且探测先试 `127.0.0.1`、失败再试 `[::1]`（兼容 `[::]` 与 IPv6-only 绑定） |
| `WB_SHIFT_ALLOWED_HOSTS` | 空（仅回环） | 额外放行的 `Host` 白名单，逗号分隔主机名（可带端口，只比主机名、大小写不敏感）。与回环白名单是并集。**不支持 `*`**（被忽略并警告）。空 = 只有 `127.0.0.1` / `localhost` / `[::1]` 能访问；填了之后，跨站 Origin 仍被拒绝，只是多接受同主机的 `https://` Origin（反代终止 TLS 的场景） |
| `WB_SHIFT_HOME` | 镜像内未设置（等价于 `/data`，因为 `HOME=/data`） | 覆盖「家目录」，数据落在其下的 `.workbuddy-shift`。想把数据换到别的挂载点时才需要 |
| `TZ` | `Asia/Shanghai`（镜像内 `ENV`） | 时区。自动签到的时间段、轮换日志时间按它计算。镜像已默认 `Asia/Shanghai`，跨时区部署用 `-e TZ=<区域>` 覆盖；只有自己把 `TZ` 显式清空时才会退回 UTC（那时「9:00 签到」会偏移 8 小时） |
| `PUID` / `PGID` | `1000` / `1000` | 仅 `docker-compose.yml` 使用（映射到容器的 `user:`），镜像本身不读 |

安全默认值刻意保持着「不配置就是最严」：默认监听回环、默认只信回环 Host、**没有任何环境
变量能关闭 Host / Origin 校验**。

## 后台任务

容器起来后进程内就有这些循环（与 webui 形态相同，见 `crates/wb-shift-server/src/main.rs`）：

| 任务 | 触发时机 | 与容器相关的注意点 |
| --- | --- | --- |
| 自动签到 | 启动核验一次，之后按「签到时间段」计算下一轮 | 时间段按 `TZ` 计算；容器停机期间不会补跑 |
| 自动旅行（派发 / 领取） | 启动即执行，之后周期重试 | 需要出网；离线时只重试不报错 |
| 自动轮换 | 按设置的间隔检查（默认 5 分钟） | 需要 `~/.codebuddy`；判断「CLI 是否在跑」在容器里永远为否（见已知限制） |
| 限额 hook 信号 | 每秒轮询 `hook-events.jsonl` | 文件在 `/data/.workbuddy-shift` 下，随卷持久化 |
| 历史日志整理 | 启动时 | 只动工具自己的日志 |

容器重启策略用 `restart: unless-stopped`：机器重启后自动拉起，`docker compose stop` 后不会
自己回来。

## 已知限制

**1. 容器看不到宿主机的进程。** 这是命名空间隔离的必然结果，影响三类判定，且都是「静默失真」
而不是报错：

- 客户端「运行中」状态永远显示未运行；
- 切换前的「客户端正在运行，是否关闭并重开」检测拿不到真实答案；
- 自动轮换里「检测到有 CLI 会话在运行就跳过本次切换」的保护**不会触发**——可能在有人正在
  用 CLI 的时候把默认账号改掉。

**2. 没有会话悬浮窗。** 悬浮窗是桌面 App 的内置窗口，容器里没有窗口系统。

**3. Linux 上没有 D-Bus / Secret Service。** 部分客户端（CodeBuddy CN IDE）的登录态存在
Safe Storage 里，需要系统密钥环解密。容器内没有密钥环时，依赖它的切换会失败并给出错误，
不是数据损坏。需要这类能力请用桌面 App。

**4. 镜像不参与应用内自动更新。** webui 形态本来就没有这个入口；升级方式固定为：

```bash
docker compose pull && docker compose up -d
```

数据在卷里，跨版本升级不需要迁移步骤（文件名与 JSON 键是稳定契约）。

**5. 只发布到回环时，`localhost` 与 `127.0.0.1` 都可以；用容器主机名访问会 403。**
这是 Host 校验在正常工作，按上面「局域网直连」配白名单即可。

**6. 时间与日志。** 容器日志走 stdout（`docker compose logs`）；应用内的错误日志落在
`/data/.workbuddy-shift/error.log`（工具数据根下，与 `accounts.json` 同级，不在子目录里）。

## 安全须知

- 服务**没有登录鉴权**：能访问端口的人就能看账号列表、切换账号、删账号。**禁止直接暴露到公网**。
- 默认只发布到 `127.0.0.1`。任何放开都要同时做三件事：改 `ports`、设
  `WB_SHIFT_ALLOWED_HOSTS`、在前面加认证。三者缺一不可。
- 不要 `-p 0.0.0.0:57910:57910` 之后又指望「反正没人知道地址」——这是本服务最容易踩的坑。
- 卷里是**真实账号数据**（`accounts.json` 含登录态）。备份用
  `docker run --rm -v "$PWD/data:/data" -v "$PWD:/backup" debian:bookworm-slim tar czf /backup/backup.tgz /data`，
  备份文件按密钥对待。
- 容器以非 root 运行、`no-new-privileges`。不要为了省事加 `--privileged` 或挂 `/var/run/docker.sock`。

## 故障排查

| 现象 | 原因与处理 |
| --- | --- |
| 浏览器显示 `{"ok":false,"error":"仅接受本机（127.0.0.1 / localhost）或 WB_SHIFT_ALLOWED_HOSTS 列出的 Host 访问"}` | Host 不在白名单。用 `127.0.0.1:57910` 访问，或把访问用的域名/内网 IP 加进 `WB_SHIFT_ALLOWED_HOSTS` 后重启容器 |
| 页面能打开但点任何按钮都 403「仅接受同源（本机页面）请求」 | 反代没保留 Host，或 HTTPS 反代但没配置 `WB_SHIFT_ALLOWED_HOSTS`。前者加 `proxy_set_header Host $host;`，后者补白名单 |
| 日志里有 `[监听] WB_SHIFT_LISTEN=… 无法解析` | 取值不是 IP 字面量（写了域名）或端口非法，已回落 `127.0.0.1`。改成 `0.0.0.0:57910` 这类写法并重启 |
| 日志里有 `[监听] WB_SHIFT_LISTEN 为空` | `-e WB_SHIFT_LISTEN=` 传了空值，已按未设置处理（只绑回环，外部连不上、容器内探测却健康）。填 `0.0.0.0:57910` 或直接删掉这个变量 |
| 启动即 `启动失败: 无法监听 …` | 端口被占用或没有权限绑定 <1024 的端口。改 `ports` 与 `WB_SHIFT_LISTEN` 的端口，注意**两处要一致** |
| `EACCES: permission denied` 写 `/data/...` | uid/gid 不匹配。`ls -ln ./data` 看属主，用 `PUID=$(id -u) PGID=$(id -g) docker compose up -d`，**不要**递归 chown |
| 服务能访问但健康检查一直 `unhealthy` | 健康检查的探测端口**只从 `WB_SHIFT_LISTEN` 推**。用命令行 `--port` 覆盖端口、或 `ports`/`WB_SHIFT_LISTEN` 两处不一致时，探的还是旧端口（默认 57910）——把 `WB_SHIFT_LISTEN` 一起改成同一端口即可。裸 IPv6（`::1` / `fe80::1`）没有端口，按默认 57910 探测；探测先试 `127.0.0.1`、失败再试 `[::1]`，所以 IPv6-only 绑定也能探到。再用 `docker inspect --format '{{json .State.Health}}' workbuddy-shift` 看最后一次探测的输出 |
| 签到时间对不上 | 镜像是 `ENV TZ=Asia/Shanghai`，先 `docker compose exec workbuddy-shift date` 确认；跨时区用 `-e TZ=<区域>` 覆盖后重启。若把 `TZ` 显式清空，容器会退回 UTC |
| 状态里账号在，但「客户端运行中」永远是否 | 已知限制：容器看不到宿主机进程 |
| 切号后客户端毫无变化 | 客户端目录没挂进来（见数据卷表），或客户端在宿主机上运行、配置被它自己的内存态覆盖 |

## 本地构建与多架构

```bash
# 单架构（当前机器），首次约 10–20 分钟（Rust release 编译 + 前端构建）
docker build -t workbuddy-shift:dev .
docker run --rm -d --name wbs-dev -p 127.0.0.1:57999:57910 \
  -v "$PWD/.docker-smoke:/data" workbuddy-shift:dev
curl -s localhost:57999/api/status
docker rm -f wbs-dev && rm -rf .docker-smoke

# 多架构（需要 buildx；与 CI 的 docker.yml 一致）
docker buildx create --name wbs-builder --use        # 一次性
docker buildx build --platform linux/amd64,linux/arm64 \
  -t ghcr.io/magicapple123/workbuddy-shift:dev --push .
```

镜像结构：

| 阶段 | 基础镜像 | 产出 |
| --- | --- | --- |
| `web` | `node:22-bookworm-slim` | `dist/`（前端静态资源，被 `rust-embed` 编译进二进制） |
| `build` | `rust:1.95-bookworm` | `target/release/workbuddy-shift` |
| `runtime` | `debian:bookworm-slim` | 只含二进制 + `ca-certificates` / `tzdata` / `curl` |

运行时镜像里没有 node、pnpm、gcc、cargo；`dist/` 已经嵌进二进制，不需要随卷挂载。

镜像发布：`.github/workflows/docker.yml` 在 `v*` tag 或手动触发时构建
`linux/amd64` + `linux/arm64` 并推送到 `ghcr.io/magicapple123/workbuddy-shift`
（tag 版本 + `latest` + `sha-*`）。
