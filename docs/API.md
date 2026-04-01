# Numina API Reference

## CLI Commands

### Chat

```bash
numina chat [OPTIONS]
numina chat [OPTIONS] <MESSAGE>
```

Options:
- `-m, --message <TEXT>`: Message to send
- `--model <NAME>`: Model to use
- `--auto-plan`: Enable auto plan generation
- `--collaborate`: Enable multi-agent collaboration
- `-s, --session <ID>`: Continue from session

### Plan

```bash
numina plan create <NAME> [OPTIONS]
numina plan execute <PLAN> [OPTIONS]
numina plan list
numina plan show <PLAN>
numina plan delete <PLAN>
numina plan optimize <PLAN> [OPTIONS]
```

### Agent

```bash
numina agent list
numina agent create <NAME> [OPTIONS]
numina agent show <NAME>
numina agent start <NAME>
numina agent stop <NAME>
numina agent update <NAME> --config <FILE>
numina agent delete <NAME>
```

### Model

```bash
numina model list
numina model add <NAME> --provider <TYPE> [OPTIONS]
numina model show <NAME>
numina model set-default <NAME>
numina model remove <NAME>
numina model test [NAME]
```

### MCP

```bash
numina mcp list
numina mcp add <NAME> --server-type <TYPE> [OPTIONS]
numina mcp remove <NAME>
numina mcp test <NAME>
numina mcp list-tools [NAME]
numina mcp start <NAME>
numina mcp stop <NAME>
```

### Collaborate

```bash
numina collaborate start <NAME> --agents <AGENTS> --task <TASK> [OPTIONS]
numina collaborate list
numina collaborate show <SESSION>
numina collaborate message <SESSION> <MESSAGE>
numina collaborate stop <SESSION>
numina collaborate config [OPTIONS]
```

### Config

```bash
numina config init
numina config show
numina config set <KEY> <VALUE>
numina config get <KEY>
numina config edit
numina config reset
```

## Configuration

### Main Config (`~/.numina/config.toml`)

```toml
[general]
version = "0.1.0"
log_level = "info"  # debug, info, warn, error

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

## Environment Variables

- `OPENAI_API_KEY`: OpenAI API key
- `ANTHROPIC_API_KEY`: Anthropic API key
- `NUMINA_LOG_LEVEL`: Log level override
- `NUMINA_WORKSPACE`: Workspace path override

## Exit Codes

- `0`: Success
- `1`: General error
- `2`: Configuration error
- `3`: Network/communication error
- `4`: Authentication error
- `5`: Timeout
