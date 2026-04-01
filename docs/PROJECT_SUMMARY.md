# Numina 项目总结

## 项目概述

**Numina (数灵)** 是一个基于 Rust 开发的 AI 智能体 CLI 工具，支持模型配置、MCP 集成、Plan 规划和 Multi-Agent 协作能力。该项目参考了 ZeroClaw/OpenClaw 的架构设计，专注于数据智能分析场景。

## 技术栈

- **语言**: Rust 2024 Edition
- **异步运行时**: Tokio
- **CLI 框架**: Clap
- **序列化**: Serde (JSON/TOML)
- **HTTP 客户端**: Reqwest
- **日志**: Tracing
- **配置**: Dirs + Config
- **数据库**: SQLite (内存管理)

## 项目结构

```
numina/
├── Cargo.toml                    # Rust 项目配置
├── README.md                     # 项目说明
├── install.sh                    # 一键安装脚本
├── Makefile                      # 构建和测试命令
├── LICENSE / LICENSE-APACHE      # 双许可证
│
├── src/                          # 源代码
│   ├── main.rs                   # 程序入口
│   │
│   ├── cli/                      # CLI 命令层
│   │   ├── mod.rs                # 命令模块
│   │   ├── chat.rs               # 聊天功能
│   │   ├── plan.rs               # 计划管理
│   │   ├── agent.rs              # Agent 管理
│   │   ├── model.rs              # 模型配置
│   │   ├── mcp.rs                # MCP 工具
│   │   ├── collaborate.rs        # 协作功能
│   │   └── config.rs             # 配置管理
│   │
│   ├── core/                     # 核心功能层
│   │   ├── agent/                # Agent 系统
│   │   │   ├── base.rs           # Agent 基础结构
│   │   │   ├── executor.rs       # Agent 执行器
│   │   │   └── memory.rs         # Agent 记忆系统
│   │   │
│   │   ├── plan/                 # 规划系统
│   │   │   ├── parser.rs         # 计划解析器
│   │   │   ├── executor.rs       # 计划执行器
│   │   │   └── optimizer.rs      # 计划优化器
│   │   │
│   │   ├── tools/                # 工具系统
│   │   │   ├── builtin.rs        # 内置工具
│   │   │   ├── mcp.rs            # MCP 工具
│   │   │   └── registry.rs       # 工具注册表
│   │   │
│   │   ├── mcp/                  # MCP 协议
│   │   │   ├── client.rs         # MCP 客户端
│   │   │   └── server.rs         # MCP 服务器
│   │   │
│   │   ├── models/               # 模型抽象
│   │   │   ├── provider.rs       # Provider trait
│   │   │   ├── openai.rs         # OpenAI 集成
│   │   │   ├── anthropic.rs      # Anthropic 集成
│   │   │   └── local.rs          # 本地模型支持
│   │   │
│   │   └── collaboration/        # 多 Agent 协作
│   │       ├── coordinator.rs    # 协作协调器
│   │       ├── message_bus.rs    # 消息总线
│   │       └── consensus.rs      # 共识引擎
│   │
│   ├── config/                   # 配置系统
│   │   ├── mod.rs                # 配置模块
│   │   ├── parser.rs             # TOML 解析器
│   │   └── validator.rs          # 配置验证器
│   │
│   └── utils/                    # 工具函数
│       ├── logger.rs             # 日志初始化
│       ├── crypto.rs             # 加密工具
│       └── fs.rs                 # 文件系统工具
│
├── docs/                         # 文档
│   ├── ARCHITECTURE.md           # 架构文档
│   ├── API.md                    # API 参考
│   ├── QUICKSTART.md             # 快速开始
│   └── PROJECT_SUMMARY.md        # 项目总结
│
├── examples/                     # 示例配置
│   ├── agent-example.toml        # Agent 示例
│   └── plan-example.toml         # Plan 示例
│
└── .env.example                  # 环境变量示例
```

## 核心功能

### 1. CLI 命令系统

- **chat**: 交互式聊天，支持自动规划和协作模式
- **plan**: 计划管理（创建、执行、优化）
- **agent**: Agent 生命周期管理
- **model**: 模型配置和切换
- **mcp**: MCP 工具链管理
- **collaborate**: 多 Agent 协作会话
- **config**: 系统配置管理

### 2. Agent 系统

- 基于角色的 Agent 设计
- 可配置的能力和参数
- 内置记忆系统（短期/长期）
- 异步任务执行
- 状态管理和监控

### 3. Plan 规划系统

- 声明式计划定义
- 步骤依赖管理
- 多种执行策略（顺序/并行/混合）
- 计划优化器
- Dry-run 模式

### 4. MCP 集成

- 完整的 MCP 协议支持
- stdio/http/websocket 服务器类型
- 工具注册和发现
- 自动连接管理
- 服务器健康检查

### 5. 模型抽象

- 统一的 Provider 接口
- 支持多种 LLM 提供商
- 流式和非流式响应
- 可配置参数
- 本地模型支持

### 6. 多 Agent 协作

- 三种协作模式：Sequential、Parallel、Consensus
- 消息总线（Pub/Sub）
- 投票和共识机制
- 会话管理
- 实时消息传递

## 设计原则

1. **最小依赖**: 保持二进制文件小巧 (<10MB)
2. **异步优先**: 所有 I/O 操作异步化
3. **Trait 驱动**: Provider、Tools 可互换
4. **安全默认**: API 密钥加密、工作区隔离
5. **可扩展**: 易于添加新工具和模型

## 架构亮点

### 参考了 ZeroClaw/OpenClaw 的设计

- **Trait 驱动架构**: 核心组件通过 trait 抽象
- **可插拔系统**: Providers、Channels、Tools 可替换
- **安全设计**: 配置文件、沙盒执行
- **轻量运行时**: 单一二进制、快速启动

### 针对数据分析场景优化

- 数据处理 Agent
- 分析计划模板
- 数据库连接器（通过 MCP）
- 报告生成工具
- 可视化支持（通过 MCP）

## 文件统计

- **Rust 源文件**: 41 个
- **总代码行数**: 约 3,500+ 行
- **文档文件**: 7 个
- **示例配置**: 2 个

## 许可证

双许可证模式：
- **MIT License**: 开源、研究、学术、个人使用
- **Apache 2.0**: 专利保护、机构、商业部署

## 后续规划

### 短期目标
1. 完善单元测试覆盖
2. 添加更多 MCP 服务器集成
3. 实现流式响应
4. 改进错误处理

### 中期目标
1. 可视化执行仪表板
2. 分布式 Agent 执行
3. Agent 市场和模板
4. 性能优化和基准测试

### 长期目标
1. Web UI 界面
2. 云端部署支持
3. 插件生态系统
4. 企业级功能

## 快速开始

```bash
# 一键安装
curl -fsSL https://raw.githubusercontent.com/numina/numina/main/install.sh | bash

# 初始化配置
numina config init

# 添加模型
numina model add gpt-4o --provider openai --default
export OPENAI_API_KEY="sk-your-key"

# 开始使用
numina chat "分析这个项目的架构"
```

## 贡献指南

欢迎贡献！请查看：
- [架构文档](ARCHITECTURE.md)
- [API 参考](API.md)
- [快速开始](QUICKSTART.md)

## 联系方式

- 项目主页: https://numina.ai
- GitHub: https://github.com/numina/numina
- Issues: https://github.com/numina/numina/issues

---

**Numina - 让数据智能分析更简单** 🚀
