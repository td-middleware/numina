/// 内置工具实现
///
/// 每个工具都实现 `ToolExecutor` trait，参数通过 JSON 传入。
/// 工具列表：
///   - read_file      读取文件内容
///   - write_file     写入文件内容
///   - list_dir       列出目录内容
///   - shell          执行 shell 命令（受限）
///   - search_code    在目录中搜索代码（grep）
///   - find_files     按文件名 glob 查找文件

use super::{ToolExecutor, ToolResult};
use async_trait::async_trait;
use serde_json::json;

// ─────────────────────────────────────────────
// read_file
// ─────────────────────────────────────────────

pub struct ReadFileTool;

#[async_trait]
impl ToolExecutor for ReadFileTool {
    async fn execute(&self, params: serde_json::Value) -> anyhow::Result<ToolResult> {
        let path = params["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("read_file: missing 'path' parameter"))?;

        // 可选：限制读取行数
        let max_lines = params["max_lines"].as_u64().unwrap_or(500) as usize;

        match std::fs::read_to_string(path) {
            Ok(content) => {
                let lines: Vec<&str> = content.lines().collect();
                let total = lines.len();
                let truncated = total > max_lines;
                let shown: String = lines[..max_lines.min(total)].join("\n");
                Ok(ToolResult {
                    success: true,
                    data: json!({
                        "content": shown,
                        "total_lines": total,
                        "truncated": truncated,
                        "path": path,
                    }),
                    error: None,
                })
            }
            Err(e) => Ok(ToolResult {
                success: false,
                data: json!(null),
                error: Some(format!("Failed to read '{}': {}", path, e)),
            }),
        }
    }

    fn name(&self) -> &str { "read_file" }
    fn description(&self) -> &str {
        "Read the contents of a file. Parameters: {\"path\": \"<file_path>\", \"max_lines\": <optional_number>}"
    }
}

// ─────────────────────────────────────────────
// write_file
// ─────────────────────────────────────────────

pub struct WriteFileTool;

#[async_trait]
impl ToolExecutor for WriteFileTool {
    async fn execute(&self, params: serde_json::Value) -> anyhow::Result<ToolResult> {
        let path = params["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("write_file: missing 'path' parameter"))?;
        let content = params["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("write_file: missing 'content' parameter"))?;

        // 自动创建父目录
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }

        match std::fs::write(path, content) {
            Ok(_) => Ok(ToolResult {
                success: true,
                data: json!({
                    "path": path,
                    "bytes_written": content.len(),
                }),
                error: None,
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                data: json!(null),
                error: Some(format!("Failed to write '{}': {}", path, e)),
            }),
        }
    }

    fn name(&self) -> &str { "write_file" }
    fn description(&self) -> &str {
        "Write content to a file (creates parent directories if needed). Parameters: {\"path\": \"<file_path>\", \"content\": \"<content>\"}"
    }
}

// ─────────────────────────────────────────────
// list_dir
// ─────────────────────────────────────────────

pub struct ListDirTool;

#[async_trait]
impl ToolExecutor for ListDirTool {
    async fn execute(&self, params: serde_json::Value) -> anyhow::Result<ToolResult> {
        let path = params["path"].as_str().unwrap_or(".");
        let recursive = params["recursive"].as_bool().unwrap_or(false);

        let mut entries: Vec<serde_json::Value> = Vec::new();

        if recursive {
            collect_recursive(std::path::Path::new(path), &mut entries, 0, 3)?;
        } else {
            let dir = std::fs::read_dir(path)
                .map_err(|e| anyhow::anyhow!("list_dir: {}", e))?;
            for entry in dir.flatten() {
                let meta = entry.metadata().ok();
                let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                entries.push(json!({
                    "name": entry.file_name().to_string_lossy(),
                    "type": if is_dir { "dir" } else { "file" },
                    "size": size,
                }));
            }
            entries.sort_by(|a, b| {
                let ta = a["type"].as_str().unwrap_or("");
                let tb = b["type"].as_str().unwrap_or("");
                tb.cmp(ta).then(a["name"].as_str().unwrap_or("").cmp(b["name"].as_str().unwrap_or("")))
            });
        }

        Ok(ToolResult {
            success: true,
            data: json!({ "path": path, "entries": entries, "count": entries.len() }),
            error: None,
        })
    }

    fn name(&self) -> &str { "list_dir" }
    fn description(&self) -> &str {
        "List files and directories. Parameters: {\"path\": \"<dir_path>\", \"recursive\": <bool>}"
    }
}

fn collect_recursive(
    dir: &std::path::Path,
    entries: &mut Vec<serde_json::Value>,
    depth: usize,
    max_depth: usize,
) -> anyhow::Result<()> {
    if depth > max_depth { return Ok(()); }
    let read = std::fs::read_dir(dir)?;
    for entry in read.flatten() {
        let meta = entry.metadata().ok();
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let path_str = entry.path().to_string_lossy().to_string();
        entries.push(json!({
            "name": entry.file_name().to_string_lossy(),
            "path": path_str,
            "type": if is_dir { "dir" } else { "file" },
            "size": size,
            "depth": depth,
        }));
        if is_dir {
            collect_recursive(&entry.path(), entries, depth + 1, max_depth)?;
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────
// shell
// ─────────────────────────────────────────────

pub struct ShellTool;

#[async_trait]
impl ToolExecutor for ShellTool {
    async fn execute(&self, params: serde_json::Value) -> anyhow::Result<ToolResult> {
        let command = params["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("shell: missing 'command' parameter"))?;

        // 安全限制：拒绝危险命令
        let blocked = ["rm -rf /", "mkfs", "dd if=", ":(){:|:&};:"];
        for b in &blocked {
            if command.contains(b) {
                return Ok(ToolResult {
                    success: false,
                    data: json!(null),
                    error: Some(format!("Blocked dangerous command pattern: {}", b)),
                });
            }
        }

        let working_dir = params["cwd"].as_str().unwrap_or(".");
        let timeout_secs = params["timeout"].as_u64().unwrap_or(30);

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            tokio::process::Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(working_dir)
                .output(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Command timed out after {}s", timeout_secs))?
        .map_err(|e| anyhow::anyhow!("Failed to execute command: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        Ok(ToolResult {
            success: output.status.success(),
            data: json!({
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": exit_code,
                "command": command,
            }),
            error: if output.status.success() { None } else {
                Some(format!("Command exited with code {}", exit_code))
            },
        })
    }

    fn name(&self) -> &str { "shell" }
    fn description(&self) -> &str {
        "Execute a shell command. Parameters: {\"command\": \"<cmd>\", \"cwd\": \"<optional_dir>\", \"timeout\": <optional_seconds>}"
    }
}

// ─────────────────────────────────────────────
// search_code
// ─────────────────────────────────────────────

pub struct SearchCodeTool;

#[async_trait]
impl ToolExecutor for SearchCodeTool {
    async fn execute(&self, params: serde_json::Value) -> anyhow::Result<ToolResult> {
        let pattern = params["pattern"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("search_code: missing 'pattern' parameter"))?;
        let path = params["path"].as_str().unwrap_or(".");
        let file_pattern = params["file_pattern"].as_str().unwrap_or("*");
        let max_results = params["max_results"].as_u64().unwrap_or(50) as usize;

        // 用 grep -rn 实现
        let grep_cmd = format!(
            "grep -rn --include='{}' -m {} '{}' '{}' 2>/dev/null | head -{}",
            file_pattern, max_results, pattern, path, max_results
        );

        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&grep_cmd)
            .output()
            .await
            .map_err(|e| anyhow::anyhow!("search_code failed: {}", e))?;

        let raw = String::from_utf8_lossy(&output.stdout).to_string();
        let matches: Vec<serde_json::Value> = raw
            .lines()
            .filter(|l| !l.is_empty())
            .map(|line| {
                // 格式: file:line:content
                let parts: Vec<&str> = line.splitn(3, ':').collect();
                if parts.len() == 3 {
                    json!({
                        "file": parts[0],
                        "line": parts[1].parse::<u64>().unwrap_or(0),
                        "content": parts[2].trim(),
                    })
                } else {
                    json!({ "raw": line })
                }
            })
            .collect();

        Ok(ToolResult {
            success: true,
            data: json!({
                "pattern": pattern,
                "path": path,
                "matches": matches,
                "count": matches.len(),
            }),
            error: None,
        })
    }

    fn name(&self) -> &str { "search_code" }
    fn description(&self) -> &str {
        "Search for a pattern in code files. Parameters: {\"pattern\": \"<regex>\", \"path\": \"<dir>\", \"file_pattern\": \"*.rs\", \"max_results\": <number>}"
    }
}

// ─────────────────────────────────────────────
// find_files
// ─────────────────────────────────────────────

pub struct FindFilesTool;

#[async_trait]
impl ToolExecutor for FindFilesTool {
    async fn execute(&self, params: serde_json::Value) -> anyhow::Result<ToolResult> {
        let pattern = params["pattern"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("find_files: missing 'pattern' parameter"))?;
        let path = params["path"].as_str().unwrap_or(".");

        let find_cmd = format!(
            "find '{}' -name '{}' -not -path '*/target/*' -not -path '*/.git/*' 2>/dev/null | head -100",
            path, pattern
        );

        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&find_cmd)
            .output()
            .await
            .map_err(|e| anyhow::anyhow!("find_files failed: {}", e))?;

        let raw = String::from_utf8_lossy(&output.stdout).to_string();
        let files_owned: Vec<String> = raw
            .lines()
            .filter(|l| !l.is_empty())
            .map(|s| s.to_string())
            .collect();

        Ok(ToolResult {
            success: true,
            data: json!({
                "pattern": pattern,
                "path": path,
                "files": files_owned,
                "count": files_owned.len(),
            }),
            error: None,
        })
    }

    fn name(&self) -> &str { "find_files" }
    fn description(&self) -> &str {
        "Find files by name pattern. Parameters: {\"pattern\": \"*.rs\", \"path\": \"<dir>\"}"
    }
}

// ─────────────────────────────────────────────
// BuiltinTool 兼容旧接口（保留）
// ─────────────────────────────────────────────

pub struct BuiltinTool {
    name: String,
    description: String,
}

impl BuiltinTool {
    pub fn new(name: String, description: String) -> Self {
        Self { name, description }
    }
}

#[async_trait]
impl ToolExecutor for BuiltinTool {
    async fn execute(&self, _parameters: serde_json::Value) -> anyhow::Result<ToolResult> {
        Ok(ToolResult {
            success: false,
            data: serde_json::Value::Null,
            error: Some(format!("Use specific tool structs instead of BuiltinTool for: {}", self.name)),
        })
    }

    fn name(&self) -> &str { &self.name }
    fn description(&self) -> &str { &self.description }
}

// ─────────────────────────────────────────────
// 工厂函数：创建默认工具注册表
// ─────────────────────────────────────────────

use std::sync::Arc;
use super::ToolRegistry;

/// 创建包含所有内置工具的注册表
pub fn default_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    let _ = registry.register(Arc::new(ReadFileTool));
    let _ = registry.register(Arc::new(WriteFileTool));
    let _ = registry.register(Arc::new(ListDirTool));
    let _ = registry.register(Arc::new(ShellTool));
    let _ = registry.register(Arc::new(SearchCodeTool));
    let _ = registry.register(Arc::new(FindFilesTool));
    registry
}
