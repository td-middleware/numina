# Numina 数灵

> 类 Claude Code / Aider 风格的 AI 代码助手 CLI，用 Rust 构建。
> 支持 MCP、多模型配置、Skills（技能）、Memory 管理、插件、Hooks 和 `claude.md` 初始 skills。

## 特性

- 🤖 **智能 Agent 系统** — 可配置和管理的 AI 代理
- 🧠 **多模型支持** — OpenAI、Anthropic Claude、本地模型（Ollama/llama.cpp）
- 🔧 **MCP 集成** — Model Context Protocol 工具链
- 📋 **Plan 规划** — 创建、执行和优化任务计划
- 🤝 **多 Agent 协作** — Sequential、Parallel、Consensus 模式
- 🎯 **Skills 系统** — 通过 `claude.md` 定义初始技能，自动注入 system prompt
- 💾 **Memory 管理** — 基于 workspace 的会话持久化（`~/.numina/workspace/sessions/`）
- 🔌 **插件/Hooks** — 可扩展的工具注册表（规划中）
- 📦 **一键安装** — `curl | bash` 或 `cargo install`

---

## 一键安装

```bash
# 方式一：curl 一键安装（推荐）
curl -fsSL https://raw.githubusercontent.com/td-middleware/numina/main/install.sh | bash

# 方式二：从源码安装
git clone https://github.com/td-middleware/numina.git
cd numina
cargo install --path .
```

安装完成后，`install.sh` 会自动：
1. 创建 `~/.numina/config.toml`（主配置）
2. 初始化 `~/.numina/workspace/`（sessions / memory / cache / logs）
3. 生成默认的 `~/.numina/workspace/claude.md`（初始 skills 配置）

---

## 快速开始

### 1. 初始化配置

```bash
numina config init
```

### 2. 配置模型

```bash
# OpenAI
export OPENAI_API_KEY="sk-..."
numina model add gpt-4o --provider openai --default

# Anthropic Claude
export ANTHROPIC_API_KEY="sk-ant-..."
numina model add claude-3-5-sonnet-20241022 --provider anthropic

# 本地模型（Ollama）
numina model add llama3.1 --provider local --endpoint http://localhost:11434
```

### 3. 开始聊天

```bash
# 交互式聊天（自动加载 claude.md skills + 会话 memory + 流式输出）
numina chat

# 单条消息（非交互式，默认流式输出）
numina chat --message "帮我 review 这段代码"

# 指定模型
numina chat --message "解释这个函数" --model claude-3-5-sonnet-20241022

# 继续上次会话（通过 session ID）
numina chat --session <session-id>

# 查看所有历史会话
numina chat sessions

# 查看某个会话的完整记录
numina chat show <session-id>
```

交互式模式内置命令：

| 命令 | 说明 |
|------|------|
| `/quit` 或 `/exit` | 退出 |
| `/session` | 显示当前 session ID |
| `/sessions` | 列出所有历史会话 |
| `/new` | 开始新会话 |
| `/skills` | 查看已加载的 skills 数量 |

---

## Skills 系统（claude.md）

Numina 在启动时会自动查找并解析 `claude.md`，优先顺序：

1. `./claude.md`（当前项目目录）
2. `~/.numina/workspace/claude.md`（全局默认）

`claude.md` 使用 Markdown 二级标题（`## 技能名`）定义 skills：

```markdown
## Code Review
对代码进行全面审查，包括逻辑正确性、安全漏洞、性能问题和代码风格。
- 检查潜在的 SQL 注入、XSS、CSRF 等安全问题
- 分析时间复杂度和空间复杂度

## Refactor
将现有代码重构为更清晰、可维护的结构，遵循 SOLID 原则。

## Write Tests
为给定代码生成单元测试和集成测试。
```

解析后的 skills 会自动拼接到 system prompt 中，让模型了解它的能力边界。

查看示例：[examples/claude.md](examples/claude.md)

---

## Memory 管理

Numina 使用基于文件的会话 memory，存储在：

```
~/.numina/workspace/sessions/<session-id>.json
```

每次对话的用户输入和模型回复都会追加到对应的 session 文件中，下次使用相同 `--session` 参数时会自动加载历史记录。

```bash
# 使用命名 session（跨次对话保持上下文）
numina chat --session my-project

# 每次都用新 session（不带 --session 时自动生成 UUID）
numina chat
```

---

## MCP 工具

```bash
# 添加 MCP 服务器
numina mcp add filesystem --server-type stdio --command-or-url "mcp-server-filesystem"

# 列出可用工具
numina mcp list-tools

# 测试连接
numina mcp test filesystem
```

---

## 多 Agent 协作

```bash
# 启动协作会话
numina collaborate start code-review \
  --agents reviewer analyst \
  --task "审查 PR #123"

# 列出活跃会话
numina collaborate list

# 发送消息
numina collaborate message <session-id> "请检查代码质量"
```

---

## 配置文件

默认位置：`~/.numina/config.toml`

```toml
[general]
version = "0.1.0"
log_level = "info"

[model]
default_model = "gpt-4o"
temperature = 0.7
max_tokens = 4096

[collaboration]
timeout_seconds = 300
max_parallel_agents = 5
consensus_required = false

[mcp]
enabled_servers = []
auto_connect = false

[workspace]
path = "~/.numina/workspace"
max_memory_mb = 1024
```

---

## 目录结构

```
src/
├── cli/              # CLI 命令层
│   ├── chat.rs       # 聊天（接入 ChatEngine + skills + memory）
│   ├── plan.rs       # 计划管理
│   ├── agent.rs      # Agent 管理
│   ├── model.rs      # 模型配置
│   ├── mcp.rs        # MCP 工具
│   ├── collaborate.rs # 协作功能
│   └── config.rs     # 配置管理
├── core/             # 核心功能
│   ├── agent/        # Agent 实现（base / executor / memory）
│   ├── chat.rs       # ChatEngine（skills + session memory + model）
│   ├── skills/       # Skills 系统（claude.md 解析 + SkillManager）
│   ├── plan/         # 规划系统
│   ├── tools/        # 工具注册表（builtin / mcp）
│   ├── mcp/          # MCP 协议（client / server）
│   ├── models/       # 模型抽象（openai / anthropic / local）
│   └── collaboration/# 协作系统（coordinator / message_bus / consensus）
├── config/           # 配置加载与验证
└── utils/            # 工具函数（logger / fs / crypto）
```

---

## 项目状态

| 功能 | 状态 |
|------|------|
| CLI 命令框架 | ✅ 完成 |
| 多模型抽象（OpenAI / Anthropic / Local） | ✅ 完成（stub，待接真实 API） |
| MCP 协议支持 | ✅ 完成（stub） |
| Skills 系统（claude.md 解析） | ✅ 完成 |
| ChatEngine（skills + session memory） | ✅ 完成（stub 模型） |
| 会话 Memory 持久化 | ✅ 完成 |
| 一键安装脚本 | ✅ 完成 |
| 多 Agent 协作 | ✅ 完成（stub） |
| 真实 API 调用（reqwest） | 🔄 规划中 |
| 流式输出（streaming） | 🔄 规划中 |
| Plugin / Hooks 系统 | 🔄 规划中 |
| TUI 聊天界面（ratatui） | 🔄 规划中 |

---

## 构建

```bash
cargo check    # 检查代码
cargo build    # 调试构建
cargo build --release  # 发布构建
cargo test     # 运行测试
```

---

## 许可证

MIT OR Apache-2.0

---

**Numina 数灵 — 让 AI 辅助编程触手可及** 🚀
