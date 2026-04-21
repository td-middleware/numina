# =============================================================================
# Numina Cloud Server — Dockerfile
#
# 构建命令：
#   docker build -t numina-server .
#
# 运行命令（最简）：
#   docker run -d \
#     -p 14521:14521 \
#     -e LARK_APP_ID=cli_xxxxxxxxxxxxxxxx \
#     -e LARK_APP_SECRET=xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx \
#     -e ANTHROPIC_API_KEY=sk-ant-xxx \
#     -e NUMINA_API_TOKEN=your-secret-token \
#     -v numina-data:/home/numina \
#     numina-server
#
# 环境变量：
#   LARK_APP_ID        飞书 App ID（必填，启用飞书 channel）
#   LARK_APP_SECRET    飞书 App Secret（必填，启用飞书 channel）
#   LARK_BRAND         feishu 或 lark（默认 feishu）
#   NUMINA_HOST        监听地址（默认 0.0.0.0）
#   NUMINA_PORT        监听端口（默认 14521）
#   NUMINA_API_TOKEN   Bearer token 鉴权（不设则不鉴权）
#   ANTHROPIC_API_KEY  Anthropic API Key
#   OPENAI_API_KEY     OpenAI API Key
#   RUST_LOG           日志级别（默认 numina=info）
# =============================================================================

# -----------------------------------------------------------------------------
# Stage 1: Builder
# 使用官方 Rust 镜像，开启 --features server 编译云端版本
# -----------------------------------------------------------------------------
FROM rust:1.87-bookworm AS builder

WORKDIR /app

# 安装系统依赖
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# ── 依赖缓存层 ──────────────────────────────────────────────────────────────
# 先只复制 Cargo 清单，构建空占位 main，让所有依赖（含 server feature）先缓存好
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --features server --locked \
    && rm -rf src

# ── 正式源码编译 ─────────────────────────────────────────────────────────────
COPY src ./src
RUN touch src/main.rs \
    && cargo build --release --features server --locked

# -----------------------------------------------------------------------------
# Stage 2: Runtime
# 最小化 Debian 镜像 + Node.js（用于 lark-cli）
# -----------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# 运行时依赖 + Node.js（lark-cli 需要）
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    tzdata \
    curl \
    && curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/*

# 安装 lark-cli（全局）
RUN npm install -g @larksuiteoapi/lark-cli \
    && lark-cli --version

# 创建非 root 用户（安全最佳实践）
RUN groupadd -r numina && useradd -r -g numina -m -d /home/numina numina

# 复制编译产物
COPY --from=builder /app/target/release/numina /usr/local/bin/numina

# 复制启动脚本
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

# 切换到非 root 用户
USER numina
WORKDIR /home/numina

# 环境变量默认值
ENV NUMINA_HOST=0.0.0.0 \
    NUMINA_PORT=14521 \
    LARK_BRAND=feishu \
    RUST_LOG=numina=info

# 持久化卷（lark-cli 配置、numina 配置、记忆数据库）
# /home/numina/.lark-cli  — lark-cli 认证配置（含 app_id/secret/token）
# /home/numina/.numina    — numina 配置（models.json、mcp.json 等）
VOLUME ["/home/numina"]

# 暴露 HTTP API 端口
EXPOSE 14521

# 健康检查：调用 /health 端点
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl -sf http://localhost:${NUMINA_PORT}/health || exit 1

# 启动入口：entrypoint.sh 负责 lark-cli 初始化，然后启动 numina serve
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
