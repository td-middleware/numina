# Numina 数灵

<div align="center">

**比 Claude Code 更强的 AI 编程助手 CLI**

*Claude Code 的代码能力 × Openclaw 的飞书生态 × Rust 的极致性能*

[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org/)

</div>

---

## 为什么选择 Numina？

| 能力 | Claude Code | Openclaw | **Numina** |
|------|:-----------:|:--------:|:----------:|
| AI 代码编写 / Review | ✅ | ❌ | ✅ |
| 飞书消息 / 日历 / 文档 | ❌ | ✅ | ✅ |
| 飞书多维表格 / Base | ❌ | ✅ | ✅ |
| Skills 意图自动识别 | ❌ | ✅ | ✅ |
| MCP 工具链 | ✅ | ✅ | ✅ |
| 持久化 Memory | ❌ | ❌ | ✅ |
| 多模型支持（OpenAI / Claude / 本地） | ❌ | ❌ | ✅ |
| 多 Agent 协作 | ❌ | ❌ | ✅ |
| 飞书 OAuth 登录 + 头像提示符 | ❌ | ✅ | ✅ |
| 命令输出实时显示 + URL 一键打开 | ❌ | ❌ | ✅ |
| Rust 原生，启动 < 50ms | ❌ | ❌ | ✅ |

> **Numina = Claude Code 的代码能力 + Openclaw 的飞书生态**，用 Rust 重写，性能更强、功能更全。

---

## 核心特性

- 🚀 **极速启动** — Rust 原生二进制，冷启动 < 50ms，无 Node.js / Python 运行时依赖
- 🤖 **智能 Agent** — 内置 ReAct 循环，自动规划、执行、反思，支持多步骤复杂任务
- 🎯 **Skills 系统** — 意图自动识别触发 + 手动斜杠命令，可联动 MCP 工具，自动注入 system prompt
- 🔧 **MCP 工具链** — 完整 Model Context Protocol 支持，兼容所有 MCP 服务器
- 🧠 **持久化 Memory** — 全局 / 项目两级记忆，每次对话自动注入上下文，AI 永远记得你的偏好
- 🌐 **飞书深度集成** — 消息、日历、文档、多维表格、审批、考勤……一个命令搞定所有飞书操作
- 🔑 **飞书 OAuth 登录** — 浏览器扫码登录，提示符实时显示用户头像（iTerm2 显示真实图片）
- 🖥️ **智能命令执行** — 实时输出流式显示，自动检测 URL 并提供一键打开浏览器，TUI 命令自动继承终端
- 🤝 **多 Agent 协作** — Sequential、Parallel、Consensus 三种协作模式
- 📦 **多模型支持** — OpenAI、Anthropic Claude、本地模型（Ollama / llama.cpp）

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

安装完成后自动初始化：
- `~/.numina/config.toml` — 主配置文件
- `~/.numina/workspace/` — sessions / memory / cache / logs
- `~/.numina/workspace/claude.md` — 初始 skills 配置

### 可选依赖

| 工具 | 用途 | 安装 |
|------|------|------|
| [iTerm2](https://iterm2.com/) | 提示符显示真实头像图片（最佳体验） | `brew install --cask iterm2` |
| [chafa](https://hpjansson.org/chafa/) | Terminal.app 显示彩色字符画头像 | `brew install chafa` |

---

## 快速开始

### 1. 初始化配置

```bash
numina config init
```

### 2. 配置 AI 模型

```bash
# Anthropic Claude（推荐）
export ANTHROPIC_API_KEY="sk-ant-..."
numina model add claude-3-5-sonnet-20241022 --provider anthropic --default

# OpenAI
export OPENAI_API_KEY="sk-..."
numina model add gpt-4o --provider openai

# 本地模型（Ollama）
numina model add llama3.1 --provider local --endpoint http://localhost:11434
```

### 3. 登录飞书（可选，解锁飞书全部能力）

```bash
numina chat
# 进入交互模式后执行：
/login
```

### 4. 开始使用

```bash
# 交互式聊天（自动加载 skills + memory + 流式输出）
numina chat

# 单条消息
numina chat --message "帮我 review 这段代码"

# 指定模型
numina chat --message "解释这个函数" --model claude-3-5-sonnet-20241022

# 继续上次会话
numina chat --session <session-id>
```

---

## 交互式命令

在 `numina chat` 交互模式中，支持以下内置命令：

| 命令 | 说明 |
|------|------|
| `/quit` 或 `/exit` | 退出 |
| `/new` | 开始新会话 |
| `/session` | 显示当前 session ID |
| `/sessions` | 列出所有历史会话 |
| `/skills` | 查看已加载的 skills |
| `/memory` | 列出所有持久化记忆 |
| `/memory add <内容>` | 添加全局记忆 |
| `/memory add -p <内容>` | 添加项目级记忆 |
| `/memory forget <id>` | 删除指定记忆 |
| `/memory search <关键词>` | 搜索记忆 |
| `/model` | 切换当前模型 |
| `/mcp` | 浏览 MCP 服务器和工具 |
| `/login` | 飞书 OAuth 登录 |
| `/auth` | 飞书授权管理 |
| `/clear` | 清屏 |
| `/help` | 显示帮助 |

---

## Skills 系统

Skills 是 Numina 超越 Claude Code 的核心能力。每个 skill 是一段结构化 Markdown 指令，告诉 AI 在特定场景下如何行动，并可联动 MCP 工具自动执行。

### 加载优先级

| 优先级 | 路径 | 说明 |
|--------|------|------|
| 1（最高） | `.numina/skills/<name>/SKILL.md` | 项目级 skill |
| 2 | `~/.numina/skills/<name>/SKILL.md` | 全局 skill |
| 3 | `~/.numina/workspace/claude.md` | 全局 claude.md |
| 4 | `./claude.md` | 项目 claude.md |

### 意图自动识别（推荐）

配置 `when_to_use` 后，Numina 自动分析用户意图，匹配时将 skill 完整指令注入 system prompt，**无需手动输入命令**：

```markdown
---
description: 搜索和分析飞书告警信息
when_to_use: 告警、alert、报警、异常告警、控制器告警
argument_hint: <关键词> [时间范围]
---

## Alert Search Skill

当用户询问告警相关问题时：
1. 调用 MCP 工具 `search_alerts` 搜索相关告警
2. 分析告警的严重程度和影响范围
3. 给出处理建议
```

```
用户输入：帮我查一下最近的控制器告警
→ 自动匹配 alert-search skill
→ 注入完整 skill 指令到 system prompt
→ AI 自动调用 MCP 工具执行搜索
```

### 手动斜杠命令

```bash
/code-review src/main.rs    # 触发 code-review skill
/refactor                   # 触发 refactor skill
/skills                     # 查看所有已加载的 skills
```

### Skills 与 MCP 联动

```markdown
---
description: 查询数据库慢查询日志
when_to_use: 慢查询、slow query、数据库性能
---

当用户询问慢查询问题时：
1. 调用 MCP 工具 `db_query` 查询慢查询日志（最近1小时）
2. 按耗时降序排列，展示 Top 10
3. 分析可能的优化方向
```

### Skill 参数替换

| 占位符 | 说明 |
|--------|------|
| `$ARGUMENT` / `${ARGUMENT}` | 完整参数字符串 |
| `$1`, `$2`, ... | 按空格分割的位置参数 |
| `${SKILL_DIR}` | skill 所在目录的绝对路径 |

---

## 飞书深度集成

Numina 内置完整的飞书生态支持，通过 `lark-cli` MCP 工具链，AI 可以直接操作飞书的所有功能：

### 支持的飞书能力

| 模块 | 能力 |
|------|------|
| 即时通讯 | 发送/接收消息、管理群聊、上传下载文件 |
| 日历 | 查看/创建日程、管理参会人、预定会议室 |
| 云文档 | 创建/编辑文档、上传图片、搜索文档 |
| 多维表格 | 建表、字段管理、记录读写、视图配置 |
| 电子表格 | 创建表格、批量读写数据、导出文件 |
| 任务 | 创建待办、查看任务列表、分配协作成员 |
| 审批 | 审批实例管理、审批任务处理 |
| 考勤 | 查询打卡记录 |
| 知识库 | 管理知识空间、节点层级结构 |
| 视频会议 | 查询会议记录、获取会议纪要 |

### 使用示例

```
用户：帮我在飞书上发一条消息给张三，说今天下午3点开会
→ AI 自动调用 lark-im skill
→ 搜索张三的 open_id
→ 发送消息

用户：把这个 Excel 数据导入到飞书多维表格
→ AI 自动调用 lark-base skill
→ 创建表格、配置字段、批量写入数据
```

### 飞书登录配置

```bash
# 方式一：交互式登录（推荐）
numina chat
/login

# 方式二：环境变量
export LARK_APP_ID="cli_xxx"
export LARK_APP_SECRET="your_secret"
```

首次登录需要飞书自建应用凭证：
1. 前往 [飞书开放平台](https://open.feishu.cn/app) 创建「自建应用」
2. 获取 **App ID** 和 **App Secret**
3. 在「安全设置」→「重定向 URL」中添加：`http://localhost:9527/callback`

---

## Memory 管理

Numina 内置持久化记忆系统，让 AI 永远记住你的偏好和项目背景：

### 存储位置

| 作用域 | 路径 | 说明 |
|--------|------|------|
| 全局 | `~/.numina/memory/global.json` | 跨项目通用，个人偏好 |
| 项目 | `{当前目录}/.numina/memory.json` | 仅当前项目，技术栈说明 |

### 使用示例

```bash
# 添加全局记忆
/memory add 我偏好用 Rust 写后端，回答请简洁

# 添加项目级记忆
/memory add -p 本项目使用 Anthropic Claude API，模型为 claude-3-5-sonnet

# 查看所有记忆
/memory

# 搜索记忆
/memory search Rust

# 删除记忆
/memory forget a1b2c3d4
```

每次发送消息时，Numina 自动检索相关记忆并注入 system prompt：

```
## Memories
- [global] 我偏好用 Rust 写后端，回答请简洁
- [project] 本项目使用 Anthropic Claude API，模型为 claude-3-5-sonnet
```

---

## 智能命令执行

Numina 的 shell 工具具备智能交互能力，远超普通 AI 助手：

### 实时输出显示

命令执行时，每行输出实时打印给用户，不再等待命令结束才显示结果。

### URL 自动检测 + 一键打开浏览器

命令输出中检测到 URL 时，自动弹出交互框：

```
  ╭─ 🔗 需要浏览器授权 ────────────────────────────────
  │
  │  https://accounts.feishu.cn/oauth/authorize?...
  │
  │  按 Enter 自动打开浏览器，或 Esc 跳过
  ╰────────────────────────────────────────────────────
```

按 **Enter** 自动打开浏览器，按 **Esc** 跳过。

### TUI 命令自动识别

`lark-cli config init`、`fzf`、`lazygit`、`htop` 等需要 TTY 的 TUI 命令，自动继承终端 stdin/stdout/stderr，界面正常显示。

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
default_model = "claude-3-5-sonnet-20241022"
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

## 终端使用指南

### iTerm2（推荐，最佳体验）

```bash
brew install --cask iterm2
```

- 提示符显示真实飞书头像图片（圆形裁剪）
- 支持 iTerm2 inline image 协议，头像精确对齐

**推荐字体：**

```bash
brew install --cask font-jetbrains-mono
```

### VSCode 集成终端

在 `settings.json` 中配置：

```json
{
  "terminal.integrated.fontFamily": "JetBrains Mono, Menlo, monospace",
  "terminal.integrated.fontSize": 14
}
```

---

## 项目状态

| 功能 | 状态 |
|------|------|
| CLI 命令框架 | ✅ 完成 |
| 多模型支持（OpenAI / Anthropic / Local） | ✅ 完成 |
| MCP 协议支持 | ✅ 完成 |
| Skills 系统（意图识别 + 斜杠命令） | ✅ 完成 |
| ChatEngine（skills + session memory） | ✅ 完成 |
| 会话 Memory 持久化 | ✅ 完成 |
| 持久化记忆系统（/memory 命令） | ✅ 完成 |
| 记忆自动注入 System Prompt | ✅ 完成 |
| 飞书 OAuth 用户登录 | ✅ 完成 |
| 头像显示（iTerm2 真实图片 / 字符画） | ✅ 完成 |
| 智能命令执行（实时输出 + URL 检测） | ✅ 完成 |
| TUI 命令自动 TTY 继承 | ✅ 完成 |
| 多 Agent 协作 | ✅ 完成 |
| 一键安装脚本 | ✅ 完成 |
| 飞书数据通道（Channel） | 🔄 开发中 |
| 全屏 TUI 模式 | 🔄 规划中 |
| 任务执行 Graph | 🔄 规划中 |
| Skills 生成器（genskills） | 🔄 规划中 |
| 云端托管 / 定时任务 | 🔄 规划中 |
| Web GUI（`numina gui`） | 🔄 规划中 |

---

## 构建

```bash
cargo check              # 检查代码
cargo build              # 调试构建
cargo build --release    # 发布构建（启用 LTO 优化）
cargo test               # 运行测试
cargo install --path .   # 安装到本地
```

---

## 目录结构

```
src/
├── cli/              # CLI 命令层
│   ├── chat.rs       # 聊天（ChatEngine + skills + memory）
│   ├── plan.rs       # 计划管理
│   ├── agent.rs      # Agent 管理
│   ├── model.rs      # 模型配置
│   ├── mcp.rs        # MCP 工具
│   ├── collaborate.rs # 协作功能
│   ├── auth.rs       # 飞书 OAuth 登录
│   └── session/      # 交互式会话（readline / renderer / completer）
├── core/             # 核心功能
│   ├── agent/        # Agent 实现（base / executor / memory）
│   ├── chat.rs       # ChatEngine（skills + session memory + model）
│   ├── skills/       # Skills 系统（claude.md 解析 + SkillManager）
│   ├── plan/         # 规划系统
│   ├── tools/        # 工具注册表（builtin / mcp）
│   ├── mcp/          # MCP 协议（client / server）
│   ├── models/       # 模型抽象（openai / anthropic / local）
│   └── collaboration/# 协作系统（coordinator / message_bus / consensus）
├── channel/          # 消息通道（飞书 / 其他平台）
├── memory/           # 持久化记忆系统
├── config/           # 配置加载与验证
└── utils/            # 工具函数（logger / fs / crypto）
```

---

## 许可证

MIT OR Apache-2.0

---

<div align="center">

**Numina 数灵 — Claude Code + Openclaw 的合体，用 Rust 打造** 🚀

*更快、更强、更懂你的工作流*

</div>
