#!/bin/bash

# Numina Project Check Script
echo "🔍 Checking Numina project..."
echo ""

# Check file structure
echo "📁 Checking project structure..."

required_files=(
    "Cargo.toml"
    "README.md"
    "install.sh"
    "LICENSE"
    "LICENSE-APACHE"
    "Makefile"
    ".env.example"
    ".gitignore"
)

missing_files=0
for file in "${required_files[@]}"; do
    if [ -f "$file" ]; then
        echo "  ✓ $file"
    else
        echo "  ✗ $file (missing)"
        ((missing_files++))
    fi
done

# Check source files
echo ""
echo "📦 Checking source files..."

source_dirs=(
    "src/cli"
    "src/core/agent"
    "src/core/plan"
    "src/core/tools"
    "src/core/mcp"
    "src/core/models"
    "src/core/collaboration"
    "src/config"
    "src/utils"
)

missing_dirs=0
for dir in "${source_dirs[@]}"; do
    if [ -d "$dir" ]; then
        file_count=$(find "$dir" -name "*.rs" | wc -l)
        echo "  ✓ $dir ($file_count files)"
    else
        echo "  ✗ $dir (missing)"
        ((missing_dirs++))
    fi
done

# Check documentation
echo ""
echo "📚 Checking documentation..."

doc_files=(
    "docs/ARCHITECTURE.md"
    "docs/API.md"
    "docs/QUICKSTART.md"
    "docs/PROJECT_SUMMARY.md"
)

missing_docs=0
for file in "${doc_files[@]}"; do
    if [ -f "$file" ]; then
        echo "  ✓ $file"
    else
        echo "  ✗ $file (missing)"
        ((missing_docs++))
    fi
done

# Check examples
echo ""
echo "📝 Checking examples..."

example_files=(
    "examples/agent-example.toml"
    "examples/plan-example.toml"
)

missing_examples=0
for file in "${example_files[@]}"; do
    if [ -f "$file" ]; then
        echo "  ✓ $file"
    else
        echo "  ✗ $file (missing)"
        ((missing_examples++))
    fi
done

# Summary
echo ""
echo "📊 Summary:"
echo "  Missing files: $missing_files"
echo "  Missing directories: $missing_dirs"
echo "  Missing docs: $missing_docs"
echo "  Missing examples: $missing_examples"

total_issues=$((missing_files + missing_dirs + missing_docs + missing_examples))

if [ $total_issues -eq 0 ]; then
    echo ""
    echo "✅ All checks passed! Numina project is complete."
    exit 0
else
    echo ""
    echo "⚠️  Found $total_issues issue(s). Please review the output above."
    exit 1
fi
