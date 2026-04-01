# Numina Architecture

## Overview

Numina is a Rust-based CLI tool for managing AI agents, plans, and multi-agent collaboration. It's inspired by ZeroClaw/OpenClaw architecture but optimized for data intelligence analysis.

## Core Components

### 1. CLI Layer (`src/cli/`)

The CLI layer provides user-facing commands:

- **Chat**: Interactive conversations with AI agents
- **Plan**: Create and execute task plans
- **Agent**: Manage AI agents
- **Model**: Configure LLM providers
- **MCP**: Manage MCP tools
- **Collaborate**: Multi-agent coordination
- **Config**: System configuration

### 2. Core Layer (`src/core/`)

The core layer implements the main functionality:

#### Agent System (`src/core/agent/`)
- `Agent`: Base agent structure with role, capabilities, and status
- `AgentExecutor`: Executes tasks on agents
- `AgentMemory`: Short-term and long-term memory storage

#### Plan System (`src/core/plan/`)
- `Plan`: Task plan with steps and dependencies
- `PlanParser`: Parse plan configurations
- `PlanExecutor`: Execute plans sequentially or in parallel
- `PlanOptimizer`: Optimize plans for different strategies

#### Tool System (`src/core/tools/`)
- `ToolRegistry`: Register and manage tools
- `BuiltinTool`: Built-in tools (file operations, search, etc.)
- `McpTool`: MCP protocol tools

#### MCP Integration (`src/core/mcp/`)
- `McpClient`: Connect to MCP servers
- `McpServer`: Host MCP functionality

#### Model Abstraction (`src/core/models/`)
- `ModelProvider`: Trait for different LLM providers
- `OpenAIProvider`: OpenAI API integration
- `AnthropicProvider`: Claude API integration
- `LocalProvider`: Local model support (llama.cpp, ollama)

#### Collaboration (`src/core/collaboration/`)
- `CollaborationCoordinator`: Manage collaboration sessions
- `MessageBus`: Pub/sub messaging between agents
- `ConsensusEngine`: Voting and consensus mechanisms

### 3. Configuration (`src/config/`)

- `NuminaConfig`: Main configuration structure
- `ConfigParser`: Parse TOML configuration
- `ConfigValidator`: Validate configuration values

### 4. Utilities (`src/utils/`)

- Logger initialization
- Cryptographic helpers
- File system utilities

## Data Flow

### Chat Flow
```
User Input → CLI → Model Provider → Response → CLI → User
              ↓
         Agent Memory
```

### Plan Execution Flow
```
Plan → Parser → Optimizer → Executor → Steps → Results
                    ↓
               Tool Registry
```

### Collaboration Flow
```
Request → Coordinator → Message Bus → Agents
                              ↑
                         Consensus Engine
```

## Design Principles

1. **Minimal Dependencies**: Keep binary size small (<10MB)
2. **Async-First**: All I/O operations are async
3. **Trait-Based**: Providers and tools are swappable
4. **Secure-By-Default**: API keys encrypted, workspace scoped
5. **Extensible**: Easy to add new tools, models, and features

## Key Innovations

1. **Hybrid Architecture**: Combines ZeroClaw's Rust performance with data-focused features
2. **MCP Native**: First-class support for Model Context Protocol
3. **Flexible Collaboration**: Sequential, parallel, and consensus modes
4. **Plan Optimization**: Automatic plan optimization for different scenarios
5. **Memory Management**: Agent memory with persistence

## Future Enhancements

1. Streaming responses with real-time updates
2. Visual plan execution dashboard
3. More MCP server integrations
4. Distributed agent execution
5. Agent marketplace and templates
