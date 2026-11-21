# WorkBuddy Shift —— headless webui 的 Docker 形态（第三个宿主：容器）
#
# 只交付 `workbuddy-shift serve --no-open` 这一个进程：没有 Tauri 桌面壳，因此
# 没有会话悬浮窗、没有托盘与开机自启。构建/权限/数据卷说明见 docs/docker.md。
#
# 三段式：web（前端 dist）→ build（Rust 二进制）→ runtime（只留二进制与运行期依赖）。
# 最终镜像里**不得**出现 node / pnpm / gcc / cargo：它们只在中间层存在。

# ---------------------------------------------------------------------------
# 1) web：产出 dist/。rust-embed 在编译期把 dist/ 写进二进制，所以它必须先就绪。
# ---------------------------------------------------------------------------
FROM node:22-bookworm-slim AS web
WORKDIR /app

# corepack 按根 package.json 的 packageManager 固定 pnpm 版本；CI 同款。
ENV COREPACK_ENABLE_DOWNLOAD_PROMPT=0 \
    CI=1
RUN corepack enable && corepack prepare pnpm@12.21.0 --activate

# 先只拷依赖清单与锁文件：改源码不会让这层失效，重建只需十几秒而不是重装依赖。
COPY package.json pnpm-workspace.yaml pnpm-lock.yaml ./
COPY companion-ui/package.json ./companion-ui/package.json
RUN pnpm install --frozen-lockfile

# 再拷源码（node_modules 由 .dockerignore 排除，不会覆盖掉刚装的依赖）。
COPY . .
RUN pnpm run build

# ---------------------------------------------------------------------------
# 2) build：Rust 单二进制。版本钉在 CI 的 msrv（1.95），不用 latest。
# ---------------------------------------------------------------------------
FROM rust:1.95-bookworm AS build
WORKDIR /app

# dist/ 必须在 cargo build 之前就位：rust-embed 是编译期读盘（见 api/static_assets.rs）。
COPY --from=web /app/dist ./dist

# workspace 清单。六个成员缺一不可：少一个 Cargo.toml 整个 workspace 都解析不了；
# 这里只编 -p wb-shift-server，所以 src-tauri 只要「能被解析」即可，不进构建产物。
# `src-tauri/src` 只拷进去让 Cargo 能校验 `[lib]` 的 `src/lib.rs` 存在（不参与编译）；
# icons/、gen/、capabilities/ 与 binaries/ 都不需要。
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY src-tauri/Cargo.toml ./src-tauri/Cargo.toml
COPY src-tauri/build.rs ./src-tauri/build.rs
COPY src-tauri/src ./src-tauri/src
# wb-shift-core 用 include_str! 把 CLI helper 编进二进制（codebuddy_cli/mod.rs）。
COPY scripts/codebuddy-cli-helper.cjs ./scripts/codebuddy-cli-helper.cjs

RUN cargo build --release --locked -p wb-shift-server

# ---------------------------------------------------------------------------
# 3) runtime：最小运行环境。
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

LABEL org.opencontainers.image.title="WorkBuddy Shift" \
      org.opencontainers.image.description="WorkBuddy / CodeBuddy 账号切换工具的 headless webui（Docker 形态）" \
      org.opencontainers.image.source="https://github.com/magicapple123/workbuddy-shift" \
      org.opencontainers.image.licenses="MIT"

# ca-certificates：reqwest(rustls) 走系统信任库；curl 只服务 HEALTHCHECK；
# tzdata：自动签到 / 自动轮换按本地时间段判断，TZ 影响这些时间段（见 docs/docker.md）。
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tzdata curl \
    && rm -rf /var/lib/apt/lists/*

# 非 root 运行。uid/gid 固定 1000：与多数桌面发行版的首个普通用户对齐，宿主目录挂进来
# 才不会满地 root 文件（compose 用 user: "${PUID:-1000}:${PGID:-1000}" 显式对齐）。
# HOME=/data 是刻意的：WB_SHIFT_HOME 未设置时，账号库与配置落在 /data/.workbuddy-shift，
# 一个挂载点就覆盖了全部工具数据。
RUN groupadd --gid 1000 app \
    && useradd --uid 1000 --gid 1000 --no-create-home \
        --home-dir /data --shell /usr/sbin/nologin app \
    && mkdir -p /data \
    && chown -R 1000:1000 /data

COPY --from=build /app/target/release/workbuddy-shift /usr/local/bin/workbuddy-shift

# WB_SHIFT_LISTEN=0.0.0.0：容器内进程必须绑所有网卡，否则 docker 的端口映射进不来。
# 对外只暴露到宿主机回环由 docker-compose.yml 的 ports 决定，两者不要搞混。
# （默认值仍是 127.0.0.1 —— 放开监听只发生在镜像这一层的显式环境变量里。）
ENV HOME=/data \
    TZ=Asia/Shanghai \
    WB_SHIFT_LISTEN=0.0.0.0:57910

WORKDIR /data
USER app
EXPOSE 57910

# 探测端口从 WB_SHIFT_LISTEN 推出来，不写死 57910：改端口时（`-e WB_SHIFT_LISTEN=0.0.0.0:9000`）
# 写死会让容器「服务可访问但永远 unhealthy」，比不写健康检查更容易误导。
# 只认环境变量：用 `--port` 覆盖端口时两者必须一起改（见 docs/docker.md 故障排查）。
# 端口推导必须按分支判断，不能只做 `${VAR##*:}`：裸 IPv6（`::1` / `fe80::1`）没有端口概念，
# 抠出来的是最后一段地址（`1`），而且是纯数字、拦不住——结果就是探一个不存在的端口。
# 探测先走 IPv4 回环、失败再试 `[::1]`：绑定 `[::]` 时两者都通，IPv6-only 绑定时只有后者通。
# Host 头是 `127.0.0.1:<port>` / `[::1]:<port>`，不配置 WB_SHIFT_ALLOWED_HOSTS 也能通过校验。
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD case "$WB_SHIFT_LISTEN" in \
          *"]:"*) port="${WB_SHIFT_LISTEN##*]:}" ;; \
          *:*:*) port=57910 ;; \
          *:*) port="${WB_SHIFT_LISTEN##*:}" ;; \
          *) port=57910 ;; \
        esac; \
        case "$port" in ''|*[!0-9]*) port=57910 ;; esac; \
        curl -fsS "http://127.0.0.1:$port/api/status" >/dev/null 2>&1 \
          || curl -fsS "http://[::1]:$port/api/status" >/dev/null 2>&1

# --no-open：容器里没有浏览器，省掉一次注定失败 xdg-open。
ENTRYPOINT ["/usr/local/bin/workbuddy-shift", "serve", "--no-open"]
