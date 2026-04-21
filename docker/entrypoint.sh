#!/bin/sh
# =============================================================================
# Numina Cloud Server — 容器启动脚本
#
# 职责：
#   1. 自动完成 lark-cli config init（非交互式，从环境变量读取凭证）
#   2. 启动 numina serve
#
# 必须环境变量（飞书 channel 需要）：
#   LARK_APP_ID       飞书应用 App ID（如 cli_xxxxxxxxxxxxxxxx）
#   LARK_APP_SECRET   飞书应用 App Secret
#
# 可选环境变量：
#   LARK_BRAND        feishu 或 lark（默认 feishu）
#   NUMINA_PORT       HTTP 监听端口（默认 14521）
#   NUMINA_API_TOKEN  Bearer token 鉴权
#   ANTHROPIC_API_KEY / OPENAI_API_KEY  AI 模型 API Key
# =============================================================================

set -e

echo "╔══════════════════════════════════════════════════╗"
echo "║         Numina Cloud Server — 启动中             ║"
echo "╚══════════════════════════════════════════════════╝"
echo ""

# ─────────────────────────────────────────────
# Step 1: 初始化 lark-cli 配置（非交互式）
# ─────────────────────────────────────────────

if [ -n "$LARK_APP_ID" ] && [ -n "$LARK_APP_SECRET" ]; then
    echo "🔧 [1/2] 初始化 lark-cli 配置..."
    echo "   App ID: ${LARK_APP_ID}"
    echo "   Brand : ${LARK_BRAND:-feishu}"

    # --app-secret-stdin 从 stdin 读取 secret，避免在进程列表中暴露
    echo "${LARK_APP_SECRET}" | lark-cli config init \
        --app-id "${LARK_APP_ID}" \
        --app-secret-stdin \
        --brand "${LARK_BRAND:-feishu}" \
        --lang "zh"

    echo "   ✅ lark-cli 配置完成"
    echo ""
else
    echo "⚠️  [1/2] 未设置 LARK_APP_ID / LARK_APP_SECRET"
    echo "   飞书 channel 将不可用（仅 HTTP API 模式）"
    echo "   如需启用飞书，请设置以下环境变量："
    echo "     LARK_APP_ID=cli_xxxxxxxxxxxxxxxx"
    echo "     LARK_APP_SECRET=xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
    echo ""
fi

# ─────────────────────────────────────────────
# Step 2: 启动 numina serve
# ─────────────────────────────────────────────

echo "🚀 [2/2] 启动 Numina Server..."
echo ""

# 如果没有配置飞书凭证，自动加 --no-lark
if [ -z "$LARK_APP_ID" ] || [ -z "$LARK_APP_SECRET" ]; then
    exec numina serve --no-lark "$@"
else
    exec numina serve "$@"
fi
