# Numina 快速入门教程

> 5 分钟上手 Numina —— 类 Claude Code 风格的 AI 代码助手 CLI

---

## 目录

1. [安装](#1-安装)
2. [初始化配置](#2-初始化配置)
3. [配置 AI 模型](#3-配置-ai-模型)
4. [开始聊天（Chat 模式）](#4-开始聊天chat-模式)
5. [自主 Agent（Act 模式）](#5-自主-agentact-模式)
6. [Skills 系统（claude.md）](#6-skills-系统claudemd)
7. [会话管理（Memory）](#7-会话管理memory)
8. [常用命令速查](#8-常用命令速查)

---

## 1. 安装

### 方式一：从源码编译（推荐开发者）

```bash
git clone https://github.com/td-middleware/numina.git
cd numina
cargo install --path .
```

### 方式二：一键安装脚本

```bash
curl -fsSL https://raw.githubusercontent.com/td-middleware/numina/main/install.sh | bash
```

安装完成后验证：

```bash
numina --version
# numina 0.1.0
```

---

## 2. 初始化配置

首次使用需要初始化工作区：

```bash
numina config init
```

这会自动创建：
- `~/.numina/config.toml` — 主配置文件
- `~/.numina/workspace/sessions/` — 会话记录目录
- `~/.numina/workspace/claude.md` — 默认 Skills 配置

查看当前配置：

```bash
numina config show
```

---

## 3. 配置 AI 模型

Numina 支持 OpenAI、Anthropic Claude 和本地模型（Ollama）。

### 使用 OpenAI（GPT-4o）

```bash
# 设置 API Key（推荐用环境变量）
export OPENAI_API_KEY="sk-..."

# 注册模型（设为默认）
numina model add gpt-4o --provider openai --default
```

### 使用 Anthropic Claude

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
numina model add claude-3-5-sonnet-20241022 --provider anthropic
```

### 使用本地模型（Ollama）

```bash
# 先启动 Ollama
ollama serve
ollama pull llama3.1

# 注册本地模型
numina model add llama3.1 --provider local --endpoint http://localhost:11434
```

### 查看已配置的模型

```bash
numina model list
# 🧠 Configured Models:
#   - gpt-4o (openai) [DEFAULT] (key: env)
#   - claude-3-5-sonnet-20241022 (anthropic) (key: env)
```

### 切换默认模型

```bash
numina model set-default claude-3-5-sonnet-20241022
```

---

## 4. 开始聊天（Chat 模式）

Chat 模式适合**问答、代码审查、解释代码**等交互式场景。

### 单条消息（非交互式）

```bash
numina chat --message "解释一下 Rust 的所有权机制"
numina chat --message "帮我 review 这段代码" --model gpt-4o
```

### 交互式聊天

```bash
numina chat
```

进入交互模式后，可以使用内置命令：

| 命令 | 说明 |
|------|------|
| `/quit` 或 `/exit` | 退出 |
| `/session` | 显示当前会话 ID |
| `/sessions` | 列出所有历史会话 |
| `/new` | 开始新会话 |
| `/skills` | 查看已加载的 Skills 数量 |

### 继续上次对话

```bash
# 先查看历史会话
numina chat sessions
# 📋 Sessions (2 total):
#    1. [a1b2c3d4] 3 turns — 解释一下 Rust 的所有权机制

# 继续指定会话
numina chat --session a1b2c3d4-...（完整 UUID）
```

### 查看会话详情

```bash
numina chat show <session-id>
```

---

## 5. 自主 Agent（Act 模式）

Act 模式是 Numina 的核心能力：Agent 会**自主规划、调用工具、执行步骤**来完成复杂任务，无需人工干预。

### 基本用法

```bash
numina agent run "<任务描述>"
```

### 示例 1：探索代码库

```bash
numina agent run "列出这个项目的所有 Rust 源文件，并统计总行数" --cwd .
```

Agent 会自动：
1. 调用 `find_files` 找到所有 `.rs` 文件
2. 调用 `shell` 执行 `wc -l` 统计行数
3. 汇总并给出最终答案

### 示例 2：代码审查

```bash
numina agent run "读取 src/main.rs，找出潜在的问题并给出改进建议" --cwd .
```

### 示例 3：自动写代码

```bash
numina agent run "在当前目录创建一个 hello.rs 文件，内容是打印 Hello, Numina!" --cwd /tmp
```

### 示例 4：搜索代码

```bash
numina agent run "在 src/ 目录中找到所有使用 unwrap() 的地方" --cwd .
```

### 常用参数

```bash
numina agent run "<task>" \
  --cwd .                    # 工作目录（shell 工具的默认目录）
  --model gpt-4o             # 指定模型
  --max-steps 10             # 最大执行步数（默认 20）
  --json                     # 输出 JSON 格式的完整步骤记录
  --quiet                    # 静默模式，只输出最终答案
```

### 查看可用工具

```bash
numina agent tools
# 🔧 Available Tools (6 total):
#   📌 read_file    — 读取文件内容
#   📌 write_file   — 写入文件
#   📌 list_dir     — 列出目录
#   📌 shell        — 执行 shell 命令
#   📌 search_code  — 搜索代码（grep）
#   📌 find_files   — 按文件名查找
```

---

## 6. Skills 系统（claude.md）

Skills 让你可以**自定义 AI 的行为和专长**，类似 Claude Code 的 `CLAUDE.md`。

### 工作原理

Numina 启动时自动查找 `claude.md`，优先顺序：
1. `./claude.md`（当前项目目录）
2. `~/.numina/workspace/claude.md`（全局默认）

解析后的 Skills 会自动注入到 system prompt 中。

### claude.md 格式

```markdown
# 项目技能配置

## Code Review
对代码进行全面审查，包括逻辑正确性、安全漏洞、性能问题和代码风格。
- 检查潜在的 SQL 注入、XSS、CSRF 等安全问题
- 分析时间复杂度和空间复杂度
- 遵循项目的编码规范

## Refactor
将现有代码重构为更清晰、可维护的结构，遵循 SOLID 原则。
- 提取重复代码为函数
- 改善命名和注释

## Write Tests
为给定代码生成单元测试和集成测试。
- 覆盖正常路径、边界条件和错误路径
- 使用项目已有的测试框架
```

### 在项目中使用

在你的项目根目录创建 `claude.md`：

```bash
cat > ./claude.md << 'EOF'
## 项目规范
这是一个 Rust 项目，使用 tokio 异步运行时。
- 所有错误使用 anyhow::Result 处理
- 公共 API 必须有文档注释
- 测试覆盖率要求 > 80%

## 代码风格
遵循 Rust 官方风格指南，使用 rustfmt 格式化。
EOF

# 在项目目录运行 numina，会自动加载 ./claude.md
numina chat --message "帮我写一个新的模块"
```

---

## 7. 会话管理（Memory）

Numina 自动将对话历史保存到 `~/.numina/workspace/sessions/`。

```bash
# 列出所有会话
numina chat sessions

# 查看某个会话的完整记录
numina chat show <session-id>

# 继续某个会话（保持上下文）
numina chat --session <session-id>
numina chat --session <session-id> --message "继续上面的话题"
```

---

## 8. 常用命令速查

```bash
# ── 配置 ──────────────────────────────────────
numina config init                    # 初始化工作区
numina config show                    # 查看配置
numina config set model.default_model gpt-4o  # 修改配置项

# ── 模型 ──────────────────────────────────────
numina model list                     # 列出模型
numina model add gpt-4o --provider openai --default
numina model set-default claude-3-5-sonnet-20241022
numina model test                     # 测试 API Key 是否有效
numina model remove gpt-4o

# ── 聊天 ──────────────────────────────────────
numina chat                           # 交互式聊天
numina chat -M "你好"                 # 单条消息
numina chat -M "问题" -o gpt-4o       # 指定模型
numina chat -s <session-id>           # 继续会话
numina chat sessions                  # 列出历史会话
numina chat show <session-id>         # 查看会话详情

# ── Agent Act ─────────────────────────────────
numina agent run "任务描述"           # 运行 Agent
numina agent run "任务" --cwd .       # 指定工作目录
numina agent run "任务" --max-steps 5 # 限制步数
numina agent run "任务" --json        # JSON 输出
numina agent run "任务" --quiet       # 只输出最终答案
numina agent tools                    # 查看可用工具
```

---

## 下一步

- 📖 查看 [API 文档](API.md)
- 🏗️ 了解 [架构设计](ARCHITECTURE.md)
- 💡 查看 [示例 claude.md](../examples/claude.md)
- 🐛 遇到问题？提交 [Issue](https://github.com/td-middleware/numina/issues)
