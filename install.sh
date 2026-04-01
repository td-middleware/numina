#!/bin/bash

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Print functions
print_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

# Check if Rust is installed
check_rust() {
    if ! command -v rustc &> /dev/null; then
        print_error "Rust is not installed. Installing Rust..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        source $HOME/.cargo/env
    else
        print_info "Rust is already installed: $(rustc --version)"
    fi
}

# Install system dependencies
install_system_deps() {
    if [[ "$OSTYPE" == "linux-gnu"* ]]; then
        if command -v apt-get &> /dev/null; then
            print_info "Installing system dependencies (Debian/Ubuntu)..."
            sudo apt-get update
            sudo apt-get install -y build-essential pkg-config
        elif command -v dnf &> /dev/null; then
            print_info "Installing system dependencies (Fedora)..."
            sudo dnf group install -y development-tools
            sudo dnf install -y pkg-config
        fi
    elif [[ "$OSTYPE" == "darwin"* ]]; then
        print_info "macOS detected. Make sure Xcode Command Line Tools are installed."
        xcode-select --install || print_info "Xcode Command Line Tools may already be installed"
    fi
}

# Build and install Numina
build_numina() {
    print_info "Building Numina..."
    cargo build --release
    
    print_info "Installing Numina..."
    cargo install --path .
}

# Initialize Numina configuration
init_config() {
    print_info "Initializing Numina configuration..."
    NUMINA_DIR="$HOME/.numina"
    mkdir -p "$NUMINA_DIR"
    
    if [[ ! -f "$NUMINA_DIR/config.toml" ]]; then
        cat > "$NUMINA_DIR/config.toml" << EOF
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
EOF
        print_info "Configuration created at $NUMINA_DIR/config.toml"
    fi
    
    mkdir -p "$NUMINA_DIR/workspace/sessions"
    mkdir -p "$NUMINA_DIR/workspace/memory"
    mkdir -p "$NUMINA_DIR/workspace/cache"
    mkdir -p "$NUMINA_DIR/workspace/logs"
    print_info "Workspace initialized at $NUMINA_DIR/workspace"

    # Copy default claude.md (skills config) if not already present
    WORKSPACE_CLAUDE="$NUMINA_DIR/workspace/claude.md"
    if [[ ! -f "$WORKSPACE_CLAUDE" ]]; then
        # Try to copy from the repo's examples directory
        SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
        if [[ -f "$SCRIPT_DIR/examples/claude.md" ]]; then
            cp "$SCRIPT_DIR/examples/claude.md" "$WORKSPACE_CLAUDE"
            print_info "Default skills (claude.md) copied to $WORKSPACE_CLAUDE"
        else
            # Inline fallback
            cat > "$WORKSPACE_CLAUDE" << 'CLAUDEEOF'
# Numina Skills Configuration (claude.md)
#
# 在这里用 Markdown 二级标题（## 技能名）定义 Numina 的初始 skills。
# Numina 启动时会自动读取此文件，将每个 ## 段落注入到 system prompt 中。

## Code Review
对代码进行全面审查，包括逻辑正确性、安全漏洞、性能问题和代码风格。

## Refactor
将现有代码重构为更清晰、可维护的结构，遵循 SOLID 原则。

## Write Tests
为给定代码生成单元测试和集成测试，覆盖正常路径、边界条件和错误路径。

## Explain Code
用简洁的中文解释代码的功能、设计意图和关键逻辑。

## Debug
帮助定位和修复 bug，分析错误信息和堆栈跟踪。
CLAUDEEOF
            print_info "Default skills (claude.md) created at $WORKSPACE_CLAUDE"
        fi
    fi
}

# Main installation
main() {
    print_info "Starting Numina installation..."
    
    # Check and install Rust
    check_rust
    
    # Install system dependencies
    install_system_deps
    
    # Build and install
    build_numina
    
    # Initialize configuration
    init_config
    
    print_info "Numina installed successfully!"
    print_info "Run 'numina --help' to see available commands"
    print_info "Run 'numina config init' to initialize your configuration"
}

# Run main function
main
