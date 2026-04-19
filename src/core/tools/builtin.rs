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
// 路径修正：将模型猜测的错误路径修正为实际可用路径
// ─────────────────────────────────────────────

/// 修正文件路径：
/// - 如果路径以 `~` 开头，展开为 home 目录
/// - 如果路径是绝对路径但父目录不存在（如 /home/user/...），
///   提取文件名，重定向到当前工作目录
/// - 相对路径直接返回（相对于 cwd，OS 会自动处理）
fn resolve_path(path: &str) -> String {
    use std::path::Path;

    // 展开 ~ 前缀
    let expanded = if path == "~" {
        std::env::var("HOME").unwrap_or_else(|_| ".".to_string())
    } else if let Some(rest) = path.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        format!("{}/{}", home, rest)
    } else {
        path.to_string()
    };

    let p = Path::new(&expanded);

    // 相对路径直接返回
    if p.is_relative() {
        return expanded;
    }

    // 绝对路径：检查父目录是否存在
    // 如果父目录不存在，说明是模型猜测的错误路径（如 /home/user/）
    // 提取文件名，重定向到当前工作目录
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            // 父目录不存在：取文件名，放到 cwd
            if let Some(filename) = p.file_name() {
                let cwd = std::env::current_dir()
                    .map(|d| d.to_string_lossy().to_string())
                    .unwrap_or_else(|_| ".".to_string());
                return format!("{}/{}", cwd, filename.to_string_lossy());
            }
        }
    }

    expanded
}

// ─────────────────────────────────────────────
// read_file
// ─────────────────────────────────────────────

pub struct ReadFileTool;

#[async_trait]
impl ToolExecutor for ReadFileTool {
    async fn execute(&self, params: serde_json::Value) -> anyhow::Result<ToolResult> {
        let raw_path = params["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("read_file: missing 'path' parameter"))?;
        let path = resolve_path(raw_path);
        let path = path.as_str();

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
        let raw_path = params["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("write_file: missing 'path' parameter"))?;
        let path = resolve_path(raw_path);
        let path = path.as_str();
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
        let raw_path = params["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("edit_file: missing 'path' parameter"))?;
        let path = resolve_path(raw_path);
        let path = path.as_str();
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

/// 判断 URL 是否为需要用户操作的授权链接（OAuth、设备授权等）
/// 过滤掉普通文档链接、帮助页面等
fn is_auth_url(url: &str) -> bool {
    let lower = url.to_lowercase();
    // 必须包含授权相关关键词
    lower.contains("oauth") ||
    lower.contains("authen") ||
    lower.contains("/auth") ||
    lower.contains("device/verify") ||
    lower.contains("flow_id") ||
    lower.contains("user_code") ||
    lower.contains("authorize") ||
    lower.contains("login") && lower.contains("feishu") ||
    lower.contains("accounts.feishu") ||
    lower.contains("accounts.larksuite")
}

/// 去掉字符串中的 ANSI 转义序列，返回纯文本
/// 用于实时打印命令输出时去掉颜色/光标控制码
fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // ESC 序列：跳过直到序列结束
            match chars.peek() {
                Some(&'[') => {
                    chars.next(); // 跳过 '['
                    // CSI 序列：跳过直到字母（序列终止符）
                    for c in chars.by_ref() {
                        if c.is_ascii_alphabetic() { break; }
                    }
                }
                Some(&']') => {
                    chars.next(); // 跳过 ']'
                    // OSC 序列：跳过直到 BEL 或 ST
                    loop {
                        match chars.next() {
                            None | Some('\x07') => break,
                            Some('\x1b') => {
                                if chars.peek() == Some(&'\\') { chars.next(); }
                                break;
                            }
                            _ => {}
                        }
                    }
                }
                _ => {} // 其他 ESC 序列，跳过
            }
        } else {
            result.push(ch);
        }
    }
    result
}

/// 从一行文本中提取干净的 URL（去掉末尾标点）
fn extract_url_from_line(line: &str) -> Option<String> {
    let pos = line.find("https://")?;
    let url_part = &line[pos..];
    // 截取到第一个空白字符
    let end = url_part.find(|c: char| c.is_whitespace()).unwrap_or(url_part.len());
    let raw = &url_part[..end];
    // 去掉末尾的标点符号（引号、逗号、句号、括号等）
    let clean = raw.trim_end_matches(|c: char| matches!(c, '"' | '\'' | ',' | '.' | ')' | '>' | ']' | ';'));
    if clean.len() < 10 { return None; }
    Some(clean.to_string())
}

/// 检测到 URL 时显示给用户，并提供一键打开浏览器的选项
/// 用于 lark-cli auth login 等需要用户在浏览器中完成操作的命令
/// is_auth: true 表示授权链接（显示"需要浏览器授权"），false 表示普通链接
/// 注意：此函数必须在 spawn_blocking 中调用，因为 crossterm::event::read() 是阻塞调用
fn show_url_and_open(url: &str, is_auth: bool) {
    use std::io::Write;
    use crossterm::event::{read as ev_read, Event, KeyCode, KeyEvent};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

    println!();
    if is_auth {
        println!("  \x1b[36m╭─ 🔗 需要浏览器授权 ────────────────────────────────\x1b[0m");
    } else {
        println!("  \x1b[36m╭─ 🔗 检测到链接 ────────────────────────────────────\x1b[0m");
    }
    println!("  \x1b[36m│\x1b[0m");
    println!("  \x1b[36m│\x1b[0m  \x1b[1m\x1b[97m{}\x1b[0m", url);
    println!("  \x1b[36m│\x1b[0m");
    println!("  \x1b[36m│\x1b[0m  \x1b[2m按 \x1b[0m\x1b[1mEnter\x1b[0m\x1b[2m 自动打开浏览器，或 \x1b[0m\x1b[1mEsc\x1b[0m\x1b[2m 跳过\x1b[0m");
    println!("  \x1b[36m╰────────────────────────────────────────────────────\x1b[0m");
    println!();
    std::io::stdout().flush().ok();

    // 确保 raw mode 在退出时一定被关闭（即使发生 panic）
    let open = {
        let _ = enable_raw_mode();
        let result = loop {
            match ev_read() {
                Ok(Event::Key(KeyEvent { code: KeyCode::Enter, .. })) => break true,
                Ok(Event::Key(KeyEvent { code: KeyCode::Esc, .. })) => break false,
                Ok(Event::Key(KeyEvent { code: KeyCode::Char('o'), .. })) => break true,
                Ok(Event::Key(KeyEvent { code: KeyCode::Char('q'), .. })) => break false,
                Err(_) => break false, // 读取失败时跳过
                _ => {}
            }
        };
        let _ = disable_raw_mode();
        result
    };

    if open {
        let _ = std::process::Command::new("open").arg(url).spawn();
        if is_auth {
            println!("  \x1b[32m✅ 已在浏览器中打开，请完成授权后等待命令继续...\x1b[0m");
        } else {
            println!("  \x1b[32m✅ 已在浏览器中打开\x1b[0m");
        }
    } else {
        println!("  \x1b[90m已跳过，可手动复制上方链接\x1b[0m");
    }
    println!();
    std::io::stdout().flush().ok();
}

/// 判断命令是否为需要 TTY 的交互式命令
/// 这类命令使用 TUI 选择菜单界面，必须继承终端 stdin/stdout/stderr
/// 注意：lark-cli auth login 不在此列，它输出 URL 到 stdout，走流式读取 + URL 检测路径
fn is_interactive_command(cmd: &str) -> bool {
    // 匹配真正的 TUI 选择菜单命令（需要 TTY 的交互式界面）
    // 规则：命令名后跟 init / setup / configure / wizard / tui 等子命令
    let interactive_patterns = [
        // lark-cli 配置初始化（TUI 选择菜单）
        "lark-cli config init",
        "lark config init",
        // 通用 TUI 子命令模式
        " init",       // 大多数 CLI 工具的 init 子命令都是交互式的
        " setup",
        " configure",
        " wizard",
        " tui",
        " interactive",
    ];
    // 先检查精确前缀匹配（避免误判 "git init" 等）
    let tui_exact = [
        "lark-cli config init",
        "lark config init",
    ];
    for pat in &tui_exact {
        if cmd.contains(pat) {
            return true;
        }
    }
    // 检查是否为已知的 TUI 工具（这些工具的所有子命令都需要 TTY）
    let tui_tools = [
        "fzf", "gum", "charm", "bubbletea", "lazygit", "lazydocker",
        "htop", "btop", "ncdu", "ranger", "nnn", "vifm",
        "tig", "gitui", "delta --interactive",
    ];
    for tool in &tui_tools {
        if cmd.starts_with(tool) || cmd.contains(&format!(" {}", tool)) {
            return true;
        }
    }
    let _ = interactive_patterns; // 避免 unused 警告
    false
}

/// 以继承 TTY 的方式运行交互式命令（stdin/stdout/stderr 直接连接到终端）
/// 命令的 TUI 界面会直接显示给用户，用户可以正常操作
async fn run_interactive_command(
    command: &str,
    working_dir: &str,
    timeout_secs: u64,
) -> anyhow::Result<ToolResult> {
    use tokio::process::Command as TokioCommand;

    println!();
    println!("  \x1b[36m╭─ 🖥  交互式命令 ─────────────────────────────────\x1b[0m");
    println!("  \x1b[36m│\x1b[0m  \x1b[2m{}\x1b[0m", command);
    println!("  \x1b[36m╰──────────────────────────────────────────────────\x1b[0m");
    println!();

    // 继承终端的 stdin/stdout/stderr，让命令直接与用户交互
    let mut child = TokioCommand::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(working_dir)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn interactive command: {}", e))?;

    // 等待命令完成（带超时）
    let deadline = std::time::Duration::from_secs(timeout_secs);
    match tokio::time::timeout(deadline, child.wait()).await {
        Ok(Ok(status)) => {
            let exit_code = status.code().unwrap_or(-1);
            println!();
            Ok(ToolResult {
                success: status.success(),
                data: json!({
                    "content": if status.success() {
                        format!("Interactive command completed successfully (exit code {})", exit_code)
                    } else {
                        format!("Interactive command exited with code {}", exit_code)
                    },
                    "exit_code": exit_code,
                    "command": command,
                    "interactive": true,
                }),
                error: if status.success() { None } else {
                    Some(format!("Command exited with code {}", exit_code))
                },
            })
        }
        Ok(Err(e)) => Err(anyhow::anyhow!("wait failed: {}", e)),
        Err(_) => {
            let _ = child.kill().await;
            Ok(ToolResult {
                success: false,
                data: json!({
                    "content": format!("Interactive command timed out after {}s", timeout_secs),
                    "exit_code": -1,
                    "command": command,
                    "interactive": true,
                }),
                error: Some(format!("Interactive command timed out after {}s", timeout_secs)),
            })
        }
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

        // 检测是否为交互式命令（需要 TTY 的命令，如 lark-cli config init）
        // 这类命令使用 TUI 界面，必须继承终端的 stdin/stdout/stderr，不能 pipe
        if is_interactive_command(command) {
            return run_interactive_command(command, working_dir, timeout_secs).await;
        }

        // 使用流式读取：spawn 子进程，逐行读取 stdout/stderr
        // 实时打印每行输出给用户，同时检测 URL 并提供一键打开浏览器
        use tokio::io::{AsyncBufReadExt, BufReader};
        use tokio::process::Command as TokioCommand;

        let mut child = TokioCommand::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(working_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to spawn command: {}", e))?;

        let stdout_pipe = child.stdout.take().unwrap();
        let stderr_pipe = child.stderr.take().unwrap();

        let mut stdout_lines = BufReader::new(stdout_pipe).lines();
        let mut stderr_lines = BufReader::new(stderr_pipe).lines();

        let mut stdout_buf = String::new();
        let mut stderr_buf = String::new();
        let mut url_shown = false; // 避免重复弹出 URL 对话框

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

        loop {
            tokio::select! {
                line = stdout_lines.next_line() => {
                    match line {
                        Ok(Some(l)) => {
                            stdout_buf.push_str(&l);
                            stdout_buf.push('\n');
                            // 实时打印输出给用户（去掉 ANSI 控制序列后显示）
                            let display = strip_ansi(&l);
                            if !display.trim().is_empty() {
                                println!("{}", display);
                            }
                            // 只对授权 URL 弹窗（OAuth/飞书登录等），普通链接不打断输出
                            if !url_shown {
                                if let Some(url) = extract_url_from_line(&l) {
                                    if is_auth_url(&url) {
                                        url_shown = true;
                                        // 必须用 spawn_blocking，因为 crossterm::event::read() 是阻塞调用
                                        // 直接在 tokio 异步任务中调用会阻塞工作线程，导致键盘事件无响应
                                        let url_clone = url.clone();
                                        tokio::task::spawn_blocking(move || {
                                            show_url_and_open(&url_clone, true);
                                        }).await.ok();
                                    }
                                }
                            }
                        }
                        Ok(None) => break, // stdout 关闭
                        Err(_) => break,
                    }
                }
                line = stderr_lines.next_line() => {
                    match line {
                        Ok(Some(l)) => {
                            stderr_buf.push_str(&l);
                            stderr_buf.push('\n');
                            // 实时打印 stderr（用暗色区分）
                            let display = strip_ansi(&l);
                            if !display.trim().is_empty() {
                                eprintln!("\x1b[2m{}\x1b[0m", display);
                            }
                            // stderr 也只对授权 URL 弹窗，普通链接不打断输出
                            if !url_shown {
                                if let Some(url) = extract_url_from_line(&l) {
                                    if is_auth_url(&url) {
                                        url_shown = true;
                                        let url_clone = url.clone();
                                        tokio::task::spawn_blocking(move || {
                                            show_url_and_open(&url_clone, true);
                                        }).await.ok();
                                    }
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(_) => {}
                    }
                }
                _ = tokio::time::sleep_until(deadline) => {
                    // 超时：杀掉子进程
                    let _ = child.kill().await;
                    let stdout = stdout_buf.trim_end().to_string();
                    let stderr = stderr_buf.trim_end().to_string();
                    let content = if stderr.is_empty() { stdout.clone() }
                        else if stdout.is_empty() { format!("[stderr]\n{}", stderr) }
                        else { format!("{}\n[stderr]\n{}", stdout, stderr) };
                    return Ok(ToolResult {
                        success: false,
                        data: json!({
                            "content": content,
                            "stdout": stdout,
                            "stderr": stderr,
                            "exit_code": -1,
                            "command": command,
                        }),
                        error: Some(format!("Command timed out after {}s", timeout_secs)),
                    });
                }
            }
        }

        // 等待进程退出
        let status = child.wait().await.map_err(|e| anyhow::anyhow!("wait failed: {}", e))?;
        let exit_code = status.code().unwrap_or(-1);

        let stdout = stdout_buf.trim_end().to_string();
        let stderr = stderr_buf.trim_end().to_string();

        // 合并 stdout + stderr 为 content 字段，方便 agent 读取
        let content = if stderr.is_empty() {
            stdout.clone()
        } else if stdout.is_empty() {
            format!("[stderr]\n{}", stderr)
        } else {
            format!("{}\n[stderr]\n{}", stdout, stderr)
        };

        Ok(ToolResult {
            success: status.success(),
            data: json!({
                "content": content,
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": exit_code,
                "command": command,
            }),
            error: if status.success() { None } else {
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
