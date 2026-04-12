/// 内置工具实现
///
/// 每个工具都实现 `ToolExecutor` trait，参数通过 JSON 传入。
/// 工具列表：
///   - read_file      读取文件内容（支持行范围）
///   - write_file     写入文件内容
///   - edit_file      精确编辑文件（search/replace）
///   - list_dir       列出目录内容
///   - shell          执行 shell 命令（受限）
///   - search_code    在目录中搜索代码（grep）
///   - find_files     按文件名 glob 查找文件
///   - http_get       发起 HTTP GET 请求
///   - task_complete  标记任务完成并返回最终结果

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

        let max_lines = params["max_lines"].as_u64().unwrap_or(500) as usize;
        let start_line = params["start_line"].as_u64().map(|n| n as usize);
        let end_line = params["end_line"].as_u64().map(|n| n as usize);

        match std::fs::read_to_string(path) {
            Ok(content) => {
                let all_lines: Vec<&str> = content.lines().collect();
                let total = all_lines.len();

                // 支持行范围读取
                let (from, to) = if let (Some(s), Some(e)) = (start_line, end_line) {
                    let s = s.saturating_sub(1).min(total);
                    let e = e.min(total);
                    (s, e)
                } else {
                    (0, max_lines.min(total))
                };

                let truncated = to < total && end_line.is_none();
                // 带行号输出（类似 Claude Code 的 read_file）
                let shown: String = all_lines[from..to]
                    .iter()
                    .enumerate()
                    .map(|(i, line)| format!("{:>4} | {}", from + i + 1, line))
                    .collect::<Vec<_>>()
                    .join("\n");

                Ok(ToolResult {
                    success: true,
                    data: json!({
                        "content": shown,
                        "total_lines": total,
                        "shown_lines": to - from,
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
        "Read the contents of a file with line numbers. Parameters: {\"path\": \"<file_path>\", \"max_lines\": <optional_number>, \"start_line\": <optional>, \"end_line\": <optional>}"
    }
    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "The file path to read" },
                "max_lines": { "type": "integer", "description": "Maximum number of lines to read (default: 500)" },
                "start_line": { "type": "integer", "description": "Start line number (1-based, inclusive)" },
                "end_line": { "type": "integer", "description": "End line number (1-based, inclusive)" }
            },
            "required": ["path"]
        })
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
                    "lines_written": content.lines().count(),
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
    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "The file path to write" },
                "content": { "type": "string", "description": "The content to write to the file" }
            },
            "required": ["path", "content"]
        })
    }
}

// ─────────────────────────────────────────────
// edit_file — 精确 search/replace 编辑
// ─────────────────────────────────────────────

pub struct EditFileTool;

#[async_trait]
impl ToolExecutor for EditFileTool {
    async fn execute(&self, params: serde_json::Value) -> anyhow::Result<ToolResult> {
        let path = params["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("edit_file: missing 'path' parameter"))?;
        let search = params["search"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("edit_file: missing 'search' parameter"))?;
        let replace = params["replace"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("edit_file: missing 'replace' parameter"))?;

        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("edit_file: cannot read '{}': {}", path, e))?;

        if !content.contains(search) {
            return Ok(ToolResult {
                success: false,
                data: json!(null),
                error: Some(format!(
                    "edit_file: search string not found in '{}'. Make sure the search string exactly matches the file content.",
                    path
                )),
            });
        }

        // 只替换第一次出现（精确编辑）
        let new_content = content.replacen(search, replace, 1);
        std::fs::write(path, &new_content)
            .map_err(|e| anyhow::anyhow!("edit_file: cannot write '{}': {}", path, e))?;

        Ok(ToolResult {
            success: true,
            data: json!({
                "path": path,
                "replaced": true,
                "old_lines": search.lines().count(),
                "new_lines": replace.lines().count(),
            }),
            error: None,
        })
    }

    fn name(&self) -> &str { "edit_file" }
    fn description(&self) -> &str {
        "Precisely edit a file by replacing an exact string. Parameters: {\"path\": \"<file_path>\", \"search\": \"<exact_string_to_find>\", \"replace\": \"<replacement_string>\"}"
    }
    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "The file path to edit" },
                "search": { "type": "string", "description": "The exact string to search for (must match exactly including whitespace)" },
                "replace": { "type": "string", "description": "The replacement string" }
            },
            "required": ["path", "search", "replace"]
        })
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
        let max_entries = params["max_entries"].as_u64().unwrap_or(200) as usize;

        let mut entries: Vec<serde_json::Value> = Vec::new();

        if recursive {
            collect_recursive(std::path::Path::new(path), &mut entries, 0, 3, max_entries)?;
        } else {
            let dir = std::fs::read_dir(path)
                .map_err(|e| anyhow::anyhow!("list_dir: {}", e))?;
            let mut children: Vec<_> = dir.flatten().collect();
            children.sort_by(|a, b| {
                let a_dir = a.path().is_dir();
                let b_dir = b.path().is_dir();
                b_dir.cmp(&a_dir).then(a.file_name().cmp(&b.file_name()))
            });
            for entry in children.into_iter().take(max_entries) {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                // 非递归模式也过滤构建产物目录（但显示隐藏文件）
                if should_skip(&name_str) {
                    continue;
                }
                let meta = entry.metadata().ok();
                let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                entries.push(json!({
                    "name": name_str,
                    "type": if is_dir { "dir" } else { "file" },
                    "size": size,
                }));
            }
        }

        let truncated = entries.len() >= max_entries;
        Ok(ToolResult {
            success: true,
            data: json!({
                "path": path,
                "entries": entries,
                "count": entries.len(),
                "truncated": truncated,
                "note": if truncated { format!("Results truncated to {} entries. Use more specific path or increase max_entries.", max_entries) } else { String::new() }
            }),
            error: None,
        })
    }

    fn name(&self) -> &str { "list_dir" }
    fn description(&self) -> &str {
        "List files and directories. Parameters: {\"path\": \"<dir_path>\", \"recursive\": <bool>, \"max_entries\": <number>}"
    }
    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "The directory path to list (default: current directory)" },
                "recursive": { "type": "boolean", "description": "Whether to list recursively up to 3 levels deep (default: false)" },
                "max_entries": { "type": "integer", "description": "Maximum number of entries to return (default: 200)" }
            },
            "required": []
        })
    }
}

/// 需要跳过的目录名（构建产物、版本控制、依赖等）
const SKIP_DIRS: &[&str] = &[
    "target", ".git", "node_modules", ".next", "dist", "build",
    "__pycache__", ".cache", ".idea", ".vscode", "vendor",
    ".cargo", "out", "coverage", ".nyc_output",
];

fn should_skip(name: &str) -> bool {
    SKIP_DIRS.contains(&name)
}

fn collect_recursive(
    dir: &std::path::Path,
    entries: &mut Vec<serde_json::Value>,
    depth: usize,
    max_depth: usize,
    max_entries: usize,
) -> anyhow::Result<()> {
    if depth > max_depth || entries.len() >= max_entries { return Ok(()); }
    let read = std::fs::read_dir(dir)?;
    let mut children: Vec<_> = read.flatten().collect();
    // 排序：目录优先，然后按名称
    children.sort_by(|a, b| {
        let a_dir = a.path().is_dir();
        let b_dir = b.path().is_dir();
        b_dir.cmp(&a_dir).then(a.file_name().cmp(&b.file_name()))
    });
    for entry in children {
        if entries.len() >= max_entries { break; }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        // 跳过隐藏文件（.开头）和构建产物目录
        if name_str.starts_with('.') || should_skip(&name_str) {
            continue;
        }
        let meta = entry.metadata().ok();
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let path_str = entry.path().to_string_lossy().to_string();
        entries.push(json!({
            "name": name_str,
            "path": path_str,
            "type": if is_dir { "dir" } else { "file" },
            "size": size,
            "depth": depth,
        }));
        if is_dir {
            collect_recursive(&entry.path(), entries, depth + 1, max_depth, max_entries)?;
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

        // 合并 stdout + stderr 为 content 字段，方便 agent 读取
        let content = if stderr.is_empty() {
            stdout.clone()
        } else if stdout.is_empty() {
            format!("[stderr]\n{}", stderr)
        } else {
            format!("{}\n[stderr]\n{}", stdout, stderr)
        };

        Ok(ToolResult {
            success: output.status.success(),
            data: json!({
                "content": content,
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": exit_code,
                "command": command,
            }),
            error: if output.status.success() { None } else {
                Some(format!("Command exited with code {}: {}", exit_code, stderr.trim()))
            },
        })
    }

    fn name(&self) -> &str { "shell" }
    fn description(&self) -> &str {
        "Execute a shell command and return stdout/stderr. Parameters: {\"command\": \"<cmd>\", \"cwd\": \"<optional_dir>\", \"timeout\": <optional_seconds>}"
    }
    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The shell command to execute" },
                "cwd": { "type": "string", "description": "Working directory for the command (default: current directory)" },
                "timeout": { "type": "integer", "description": "Timeout in seconds (default: 30)" }
            },
            "required": ["command"]
        })
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

        // 格式化为易读的 content 字段
        let content = if matches.is_empty() {
            format!("No matches found for pattern '{}' in '{}'", pattern, path)
        } else {
            matches.iter().map(|m| {
                if let (Some(f), Some(l), Some(c)) = (
                    m["file"].as_str(),
                    m["line"].as_u64(),
                    m["content"].as_str(),
                ) {
                    format!("{}:{}: {}", f, l, c)
                } else {
                    m["raw"].as_str().unwrap_or("").to_string()
                }
            }).collect::<Vec<_>>().join("\n")
        };

        Ok(ToolResult {
            success: true,
            data: json!({
                "content": content,
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
        "Search for a pattern in code files using grep. Parameters: {\"pattern\": \"<regex>\", \"path\": \"<dir>\", \"file_pattern\": \"*.rs\", \"max_results\": <number>}"
    }
    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "The regex pattern to search for" },
                "path": { "type": "string", "description": "The directory to search in (default: current directory)" },
                "file_pattern": { "type": "string", "description": "File glob pattern to filter (e.g., '*.rs', '*.py')" },
                "max_results": { "type": "integer", "description": "Maximum number of results to return (default: 50)" }
            },
            "required": ["pattern"]
        })
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
        let files: Vec<String> = raw
            .lines()
            .filter(|l| !l.is_empty())
            .map(|s| s.to_string())
            .collect();

        let content = if files.is_empty() {
            format!("No files found matching '{}' in '{}'", pattern, path)
        } else {
            files.join("\n")
        };

        Ok(ToolResult {
            success: true,
            data: json!({
                "content": content,
                "pattern": pattern,
                "path": path,
                "files": files,
                "count": files.len(),
            }),
            error: None,
        })
    }

    fn name(&self) -> &str { "find_files" }
    fn description(&self) -> &str {
        "Find files by name pattern. Parameters: {\"pattern\": \"*.rs\", \"path\": \"<dir>\"}"
    }
    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Glob pattern to match files (e.g., '*.rs', 'main.*')" },
                "path": { "type": "string", "description": "The directory to search in (default: current directory)" }
            },
            "required": ["pattern"]
        })
    }
}

// ─────────────────────────────────────────────
// http_get — 发起 HTTP GET 请求
// ─────────────────────────────────────────────

pub struct HttpGetTool;

#[async_trait]
impl ToolExecutor for HttpGetTool {
    async fn execute(&self, params: serde_json::Value) -> anyhow::Result<ToolResult> {
        let url = params["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("http_get: missing 'url' parameter"))?;
        let max_bytes = params["max_bytes"].as_u64().unwrap_or(32768) as usize;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent("Numina-Agent/0.1")
            .build()
            .map_err(|e| anyhow::anyhow!("http_get: failed to build client: {}", e))?;

        match client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let headers: std::collections::HashMap<String, String> = resp
                    .headers()
                    .iter()
                    .filter_map(|(k, v)| {
                        v.to_str().ok().map(|vs| (k.to_string(), vs.to_string()))
                    })
                    .collect();

                let body = resp.text().await.unwrap_or_default();
                let truncated = body.len() > max_bytes;
                let content: String = body.chars().take(max_bytes).collect();

                Ok(ToolResult {
                    success: status < 400,
                    data: json!({
                        "content": content,
                        "status": status,
                        "url": url,
                        "truncated": truncated,
                        "content_type": headers.get("content-type").cloned().unwrap_or_default(),
                    }),
                    error: if status >= 400 {
                        Some(format!("HTTP {} for {}", status, url))
                    } else {
                        None
                    },
                })
            }
            Err(e) => Ok(ToolResult {
                success: false,
                data: json!(null),
                error: Some(format!("http_get failed for '{}': {}", url, e)),
            }),
        }
    }

    fn name(&self) -> &str { "http_get" }
    fn description(&self) -> &str {
        "Make an HTTP GET request and return the response body. Parameters: {\"url\": \"<url>\", \"max_bytes\": <optional_number>}"
    }
    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "The URL to fetch" },
                "max_bytes": { "type": "integer", "description": "Maximum response body size in bytes (default: 32768)" }
            },
            "required": ["url"]
        })
    }
}

// ─────────────────────────────────────────────
// http_post — 发起 HTTP POST 请求（支持 MCP JSON-RPC）
// ─────────────────────────────────────────────

pub struct HttpPostTool;

#[async_trait]
impl ToolExecutor for HttpPostTool {
    async fn execute(&self, params: serde_json::Value) -> anyhow::Result<ToolResult> {
        let url = params["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("http_post: missing 'url' parameter"))?;
        // 默认 512KB，MCP 响应数据可能很大（如 search_alert 返回 ~30KB+）
        // 截断会导致 AI 收到不完整 JSON，误以为调用失败而反复重试
        let max_bytes = params["max_bytes"].as_u64().unwrap_or(524288) as usize;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("Numina-Agent/0.1")
            .build()
            .map_err(|e| anyhow::anyhow!("http_post: failed to build client: {}", e))?;

        // 构建请求
        let mut req = client.post(url);

        // 设置 Content-Type（默认 application/json）
        let content_type = params["content_type"]
            .as_str()
            .unwrap_or("application/json");
        req = req.header("Content-Type", content_type);

        // 设置自定义 headers
        if let Some(headers) = params["headers"].as_object() {
            for (k, v) in headers {
                if let Some(vs) = v.as_str() {
                    req = req.header(k.as_str(), vs);
                }
            }
        }

        // 设置请求体
        // 辅助函数：对 JSON-RPC body 按标准字段顺序重排（jsonrpc → id → method → params）
        // 避免字段顺序不确定导致服务端解析 method 为空（-32601 "Method not found: "）
        let normalize_jsonrpc = |val: &serde_json::Value| -> String {
            if val.get("jsonrpc").is_some() && val.get("method").is_some() {
                let jsonrpc = val.get("jsonrpc").cloned().unwrap_or(serde_json::Value::Null);
                let id = val.get("id").cloned().unwrap_or(serde_json::Value::Null);
                let method = val.get("method").cloned().unwrap_or(serde_json::Value::Null);
                let rpc_params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
                let ordered = serde_json::json!({
                    "jsonrpc": jsonrpc,
                    "id": id,
                    "method": method,
                    "params": rpc_params
                });
                serde_json::to_string(&ordered).unwrap_or_default()
            } else {
                serde_json::to_string(val).unwrap_or_default()
            }
        };

        let body_str = if params["body"].is_null() || params.get("body").is_none() {
            String::new()
        } else if params["body"].is_string() {
            // body 是字符串形式：尝试解析为 JSON 后重排字段顺序
            let s = params["body"].as_str().unwrap_or("");
            // 多轮尝试解析：
            // 1. 直接解析
            // 2. trim 后解析（处理首尾空白/BOM）
            // 3. 去除控制字符后解析
            // 4. 处理双重转义（\\\" → \"）后解析
            let cleaned: String = s.chars().filter(|c| !c.is_control() || *c == '\n' || *c == '\r' || *c == '\t').collect();
            let unescaped = s.replace("\\\"", "\"").replace("\\'", "'");
            let parsed_opt = serde_json::from_str::<serde_json::Value>(s).ok()
                .or_else(|| serde_json::from_str::<serde_json::Value>(s.trim()).ok())
                .or_else(|| serde_json::from_str::<serde_json::Value>(&cleaned).ok())
                .or_else(|| serde_json::from_str::<serde_json::Value>(cleaned.trim()).ok())
                .or_else(|| serde_json::from_str::<serde_json::Value>(&unescaped).ok())
                .or_else(|| serde_json::from_str::<serde_json::Value>(unescaped.trim()).ok());
            if let Some(parsed) = parsed_opt {
                // 解析成功：normalize（统一字段顺序）
                normalize_jsonrpc(&parsed)
            } else if s.trim_start().starts_with('{') || s.trim_start().starts_with('[') {
                // 看起来是 JSON 但解析失败（最常见原因：AI 生成的 JSON 字符串不完整，缺结尾 }）
                // 尝试自动补全：统计 { 和 } 的数量，补足缺失的 }
                let open = cleaned.chars().filter(|&c| c == '{').count();
                let close = cleaned.chars().filter(|&c| c == '}').count();
                let missing = open.saturating_sub(close);
                let repaired = if missing > 0 {
                    let suffix: String = "}".repeat(missing);
                    format!("{}{}", cleaned.trim_end_matches(|c: char| c.is_whitespace() || c == ','), suffix)
                } else {
                    cleaned.trim().to_string()
                };
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&repaired) {
                    // 修复成功，使用修复后的 JSON
                    normalize_jsonrpc(&parsed)
                } else {
                    // 无法修复，直接把字符串作为 body 发送（让服务端处理）
                    s.to_string()
                }
            } else {
                // 非 JSON 字符串（如 form-encoded 等），直接发送
                s.to_string()
            }
        } else {
            // body 是 JSON 对象（推荐方式）：直接 normalize
            normalize_jsonrpc(&params["body"])
        };

        if !body_str.is_empty() {
            req = req.body(body_str);
        }

        match req.send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let resp_content_type = resp
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();

                let body = resp.text().await.unwrap_or_default();
                let truncated = body.len() > max_bytes;
                let content: String = body.chars().take(max_bytes).collect();

                Ok(ToolResult {
                    success: status < 400,
                    data: json!({
                        "content": content,
                        "status": status,
                        "url": url,
                        "truncated": truncated,
                        "content_type": resp_content_type,
                    }),
                    error: if status >= 400 {
                        Some(format!("HTTP {} for {}", status, url))
                    } else {
                        None
                    },
                })
            }
            Err(e) => Ok(ToolResult {
                success: false,
                data: json!(null),
                error: Some(format!("http_post failed for '{}': {}", url, e)),
            }),
        }
    }

    fn name(&self) -> &str { "http_post" }
    fn description(&self) -> &str {
        "Make an HTTP POST request and return the response. Use this for APIs, MCP servers (JSON-RPC over POST), webhooks, etc. Parameters: {\"url\": \"<url>\", \"body\": <json_or_string>, \"headers\": {\"key\": \"value\"}, \"content_type\": \"application/json\", \"max_bytes\": <optional>}"
    }
    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "The URL to POST to" },
                "body": {
                    "type": "object",
                    "description": "Request body as a JSON object (NOT a string). For MCP JSON-RPC use: {\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"tool_name\",\"arguments\":{...}}}"
                },
                "headers": { "type": "object", "description": "Additional HTTP headers as key-value pairs" },
                "content_type": { "type": "string", "description": "Content-Type header (default: application/json)" },
                "max_bytes": { "type": "integer", "description": "Maximum response body size in bytes (default: 32768)" }
            },
            "required": ["url"]
        })
    }
}

// ─────────────────────────────────────────────
// task_complete — 标记任务完成
// ─────────────────────────────────────────────

pub struct TaskCompleteTool;

#[async_trait]
impl ToolExecutor for TaskCompleteTool {
    async fn execute(&self, params: serde_json::Value) -> anyhow::Result<ToolResult> {
        let result = params["result"]
            .as_str()
            .unwrap_or("Task completed successfully.");
        let summary = params["summary"].as_str().unwrap_or(result);

        Ok(ToolResult {
            success: true,
            data: json!({
                "content": result,
                "result": result,
                "summary": summary,
                "completed": true,
            }),
            error: None,
        })
    }

    fn name(&self) -> &str { "task_complete" }
    fn description(&self) -> &str {
        "Signal that the task is complete and provide the final result. Use this when you have finished all necessary steps. Parameters: {\"result\": \"<final_answer>\", \"summary\": \"<optional_summary>\"}"
    }
    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "result": { "type": "string", "description": "The final result or answer to the task" },
                "summary": { "type": "string", "description": "A brief summary of what was accomplished" }
            },
            "required": ["result"]
        })
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
    let _ = registry.register(Arc::new(EditFileTool));
    let _ = registry.register(Arc::new(ListDirTool));
    let _ = registry.register(Arc::new(ShellTool));
    let _ = registry.register(Arc::new(SearchCodeTool));
    let _ = registry.register(Arc::new(FindFilesTool));
    let _ = registry.register(Arc::new(HttpGetTool));
    let _ = registry.register(Arc::new(HttpPostTool));
    let _ = registry.register(Arc::new(TaskCompleteTool));
    registry
}
