# Numina 数灵

> 类 Claude Code / Aider 风格的 AI 代码助手 CLI，用 Rust 构建。
> 支持 MCP、多模型配置、Skills（技能）、Memory 管理、插件、Hooks 和 `claude.md` 初始 skills。

## 特性

- 🤖 **智能 Agent 系统** — 可配置和管理的 AI 代理
- 🧠 **多模型支持** — OpenAI、Anthropic Claude、本地模型（Ollama/llama.cpp）
- 🔧 **MCP 集成** — Model Context Protocol 工具链
- 📋 **Plan 规划** — 创建、执行和优化任务计划
- 🤝 **多 Agent 协作** — Sequential、Parallel、Consensus 模式
- 🎯 **Skills 系统** — 支持意图自动识别触发 + 手动斜杠命令，可联动 MCP 工具，自动注入 system prompt
- 💾 **Memory 管理** — 持久化记忆系统，支持全局/项目两级作用域，自动注入 system prompt
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
| `/memory` | 列出所有持久化记忆 |
| `/memory add <内容>` | 添加全局记忆 |
| `/memory add -p <内容>` | 添加项目级记忆 |
| `/memory forget <id>` | 删除指定记忆 |
| `/memory search <关键词>` | 搜索记忆 |
| `/model` | 切换当前模型 |
| `/mcp` | 浏览 MCP 服务器和工具 |
| `/clear` | 清屏 |
| `/help` | 显示帮助信息 |

---

## Skills 系统

Skills 是 Numina 的核心能力扩展机制。每个 skill 是一段结构化的 Markdown 指令，告诉 AI 在特定场景下如何行动。Skills 支持两种触发方式：**意图自动识别**和**手动斜杠命令**。

### 加载优先级

Numina 启动时按以下顺序自动发现并加载 skills（同名 skill 以先加载的为准）：

| 优先级 | 路径 | 说明 |
|--------|------|------|
| 1（最高） | `.numina/skills/<name>/SKILL.md` | 项目级 skill，仅当前目录生效 |
| 2 | `~/.numina/skills/<name>/SKILL.md` | 全局 skill，跨项目通用 |
| 3 | `~/.numina/workspace/claude.md` | 全局 claude.md（`##` 标题格式） |
| 4 | `./claude.md` | 项目 claude.md（`##` 标题格式） |

### 方式一：意图自动识别（推荐）

在 `SKILL.md` 中配置 `when_to_use` 字段，Numina 会在每次对话时自动分析用户意图，一旦匹配就将该 skill 的完整指令注入到本次请求的 system prompt 中，**无需用户手动输入斜杠命令**。

**SKILL.md 格式（带 YAML frontmatter）：**

```markdown
---
description: 搜索和分析告警信息
when_to_use: 告警、alert、报警、异常告警、控制器告警
argument_hint: <关键词> [时间范围]
---

## Alert Search Skill

当用户询问告警相关问题时，请按以下步骤处理：

1. 调用 MCP 工具 `search_alerts` 搜索相关告警
2. 分析告警的严重程度和影响范围
3. 给出处理建议
```

**意图匹配规则：**
- `when_to_use` 支持中文顿号（`、`）、逗号、空格、斜杠分隔多个关键词
- 支持子词匹配：如关键词"控制器告警"，用户输入"告警"也能命中
- 多个关键词命中时，相关度更高的 skill 优先注入

**示例：**
```
用户输入：帮我查一下最近的控制器告警
→ 自动匹配 alert-search skill
→ 注入完整 skill 指令到 system prompt
→ AI 自动调用 MCP 工具执行搜索
```

### 方式二：手动斜杠命令

在 `claude.md` 或 `SKILL.md` 中定义的 skill，可以通过 `/skill名称 [参数]` 的方式手动触发，适合需要明确指定场景或传入参数的情况。

**claude.md 格式（`##` 标题格式）：**

```markdown
## code-review
对代码进行全面审查，包括逻辑正确性、安全漏洞、性能问题和代码风格。
- 检查潜在的 SQL 注入、XSS、CSRF 等安全问题
- 分析时间复杂度和空间复杂度

## refactor
将现有代码重构为更清晰、可维护的结构，遵循 SOLID 原则。

## write-tests
为给定代码生成单元测试和集成测试。
```

**手动调用示例：**
```bash
# 触发 code-review skill
/code-review src/main.rs

# 触发 refactor skill（不带参数）
/refactor

# 查看所有已加载的 skills
/skills
```

### Skills 与 MCP 联动

Skills 可以在指令中描述需要调用的 MCP 工具，当 skill 被触发（自动或手动）后，AI 会根据 skill 指令自动调用对应的 MCP 工具完成任务：

```markdown
---
description: 查询数据库慢查询日志
when_to_use: 慢查询、slow query、数据库性能、SQL慢
---

当用户询问慢查询问题时：
1. 调用 MCP 工具 `db_query` 查询慢查询日志（最近1小时）
2. 按耗时降序排列，展示 Top 10
3. 分析可能的优化方向
```

### Skill 参数替换

在 skill 内容中可以使用占位符，调用时自动替换：

| 占位符 | 说明 |
|--------|------|
| `$ARGUMENT` 或 `${ARGUMENT}` | 完整参数字符串 |
| `$1`, `$2`, ... | 按空格分割的位置参数 |
| `${SKILL_DIR}` | skill 所在目录的绝对路径 |

```markdown
## deploy
将 $1 服务部署到 $2 环境。
请先检查 ${SKILL_DIR}/deploy-checklist.md 中的部署前检查项。
```

```bash
/deploy user-service production
# $1 = user-service，$2 = production
```

查看示例：[examples/claude.md](examples/claude.md)

---

## Memory 管理

Numina 内置**持久化记忆系统**，让 AI 在每次对话中都能记住你的偏好、项目背景和重要信息。记忆以 JSON 文件形式存储，支持**全局**和**项目**两个作用域，并在每次对话时自动注入 system prompt。

### 存储位置

| 作用域 | 路径 | 说明 |
|--------|------|------|
| 全局（Global） | `~/.numina/memory/global.json` | 跨项目通用，适合个人偏好、习惯设置 |
| 项目（Project） | `{当前目录}/.numina/memory.json` | 仅当前项目生效，适合项目背景、技术栈说明 |

### 记忆数据结构

每条记忆包含以下字段：

```json
{
  "id": "a1b2c3d4",
  "content": "用户偏好：回答请使用中文，并保持简洁",
  "tags": ["用户偏好", "回答", "中文", "简洁"],
  "created_at": "2025-04-13T09:00:00Z",
  "updated_at": "2025-04-13T09:00:00Z",
  "source": "User",
  "scope": "Global"
}
```

- **id**：8 位短 UUID，用于 `/memory forget` 删除操作
- **tags**：从 content 中自动提取的关键词，用于搜索匹配
- **source**：`User`（手动添加）或 `Auto`（AI 自动生成，预留）
- **scope**：`Global` 或 `Project`

### 交互式命令

在 `numina chat` 交互模式中，使用以下斜杠命令管理记忆：

| 命令 | 说明 |
|------|------|
| `/memory` | 列出所有记忆（全局 + 项目，颜色区分） |
| `/memory add <内容>` | 添加一条全局记忆 |
| `/memory add -p <内容>` | 添加一条项目级记忆（仅当前目录生效） |
| `/memory forget <id>` | 按 ID 删除一条记忆 |
| `/memory search <关键词>` | 按关键词搜索记忆 |

### 使用示例

```bash
# 启动交互式聊天
numina chat

# 添加全局记忆（跨项目生效）
/memory add 我偏好用 Rust 写后端，回答请简洁

# 添加项目级记忆（仅当前目录生效）
/memory add -p 本项目使用 Anthropic Claude API，模型为 claude-3-5-sonnet

# 查看所有记忆
/memory
#   [a1b2c3d4] 我偏好用 Rust 写后端，回答请简洁  [global/user]
#   [e5f6g7h8] 本项目使用 Anthropic Claude API  [project/user]

# 搜索记忆
/memory search Rust

# 删除记忆
/memory forget a1b2c3d4
```

### 自动注入 System Prompt

每次发送消息时，Numina 会根据消息内容自动检索相关记忆，并将其注入 system prompt：

```
## Memories
- [global] 我偏好用 Rust 写后端，回答请简洁
- [project] 本项目使用 Anthropic Claude API，模型为 claude-3-5-sonnet
```

这样 AI 无需你每次重复说明背景，即可给出更贴合你习惯的回答。

### 会话持久化（Session Memory）

除了上述持久化记忆，Numina 还会自动保存每次对话的完整历史：

```
~/.numina/workspace/sessions/<session-id>.json
```

```bash
# 继续上次会话（自动恢复上下文）
numina chat --session my-project

# 每次新建会话（不带 --session 时自动生成 UUID）
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
| 会话 Memory 持久化（session history） | ✅ 完成 |
| 持久化记忆系统（/memory 命令） | ✅ 完成 |
| 记忆自动注入 System Prompt | ✅ 完成 |
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
