/// MCP 客户端 — 通过 stdio 连接外部 MCP 服务器
///
/// 协议：JSON-RPC 2.0 over stdio
/// 参考：https://modelcontextprotocol.io/docs/concepts/transports
///
/// 工作流程：
/// 1. 启动 MCP 服务器子进程（command + args）
/// 2. 发送 initialize 请求，完成握手
/// 3. 发送 tools/list 获取工具列表
/// 4. 发送 tools/call 执行工具

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use std::sync::Arc;
use std::collections::HashMap;

// ─────────────────────────────────────────────
// JSON-RPC 2.0 types
// ─────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    id: Option<Value>,
    result: Option<Value>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[allow(dead_code)]
    data: Option<Value>,
}

// ─────────────────────────────────────────────
// MCP Tool 类型
// ─────────────────────────────────────────────

/// MCP 工具定义（从服务器获取）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolInfo {
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: Option<Value>,
}

/// MCP 工具调用结果
#[derive(Debug, Clone)]
pub struct McpCallResult {
    pub content: Vec<McpContent>,
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: Option<String>,
}

impl McpCallResult {
    /// 将结果转换为字符串
    pub fn to_string(&self) -> String {
        self.content
            .iter()
            .filter_map(|c| c.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// ─────────────────────────────────────────────
// McpClient
// ─────────────────────────────────────────────

struct McpInner {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    #[allow(dead_code)]
    child: Child,
}

pub struct McpClient {
    inner: Arc<Mutex<McpInner>>,
    pub server_name: String,
    pub tools: Vec<McpToolInfo>,
}

impl McpClient {
    /// 连接到 MCP 服务器（启动子进程并完成握手）
    pub async fn connect(
        server_name: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()); // 忽略 stderr，避免干扰

        // 注入环境变量
        for (k, v) in env {
            cmd.env(k, v);
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to start MCP server '{}': {}", server_name, command))?;

        let stdin = child.stdin.take()
            .ok_or_else(|| anyhow::anyhow!("Failed to get stdin for MCP server '{}'", server_name))?;
        let stdout = child.stdout.take()
            .ok_or_else(|| anyhow::anyhow!("Failed to get stdout for MCP server '{}'", server_name))?;

        let inner = Arc::new(Mutex::new(McpInner {
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
            child,
        }));

        let mut client = Self {
            inner,
            server_name: server_name.to_string(),
            tools: vec![],
        };

        // 完成 MCP 握手
        client.initialize().await
            .with_context(|| format!("MCP initialize failed for '{}'", server_name))?;

        // 获取工具列表
        client.tools = client.list_tools().await
            .unwrap_or_default();

        Ok(client)
    }

    /// 发送 JSON-RPC 请求并等待响应
    async fn send_request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let mut inner = self.inner.lock().await;

        let id = inner.next_id;
        inner.next_id += 1;

        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };

        let req_str = serde_json::to_string(&req)?;
        inner.stdin.write_all(req_str.as_bytes()).await?;
        inner.stdin.write_all(b"\n").await?;
        inner.stdin.flush().await?;

        // 读取响应（按行读取，跳过空行和通知消息）
        loop {
            let mut line = String::new();
            let n = inner.stdout.read_line(&mut line).await?;
            if n == 0 {
                anyhow::bail!("MCP server '{}' closed connection", self.server_name);
            }

            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // 尝试解析为 JSON-RPC 响应
            let resp: JsonRpcResponse = match serde_json::from_str(line) {
                Ok(r) => r,
                Err(_) => continue, // 跳过无法解析的行（可能是日志）
            };

            // 检查是否是通知消息（无 id）
            if resp.id.is_none() {
                continue;
            }

            // 检查 id 是否匹配
            let resp_id = match &resp.id {
                Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
                _ => 0,
            };
            if resp_id != id {
                continue; // 不是我们等待的响应
            }

            if let Some(err) = resp.error {
                anyhow::bail!(
                    "MCP error from '{}': [{}] {}",
                    self.server_name, err.code, err.message
                );
            }

            return resp.result.ok_or_else(|| {
                anyhow::anyhow!("MCP response from '{}' has no result", self.server_name)
            });
        }
    }

    /// MCP initialize 握手
    async fn initialize(&self) -> Result<()> {
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "roots": { "listChanged": false },
                "sampling": {}
            },
            "clientInfo": {
                "name": "numina",
                "version": "0.1.0"
            }
        });

        self.send_request("initialize", Some(params)).await?;

        // 发送 initialized 通知（无需等待响应）
        let notif = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        });
        let mut inner = self.inner.lock().await;
        let notif_str = serde_json::to_string(&notif)?;
        inner.stdin.write_all(notif_str.as_bytes()).await?;
        inner.stdin.write_all(b"\n").await?;
        inner.stdin.flush().await?;

        Ok(())
    }

    /// 获取工具列表
    pub async fn list_tools(&self) -> Result<Vec<McpToolInfo>> {
        let result = self.send_request("tools/list", None).await?;

        let tools = result
            .get("tools")
            .and_then(|t| t.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| serde_json::from_value::<McpToolInfo>(v.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();

        Ok(tools)
    }

    /// 调用工具
    pub async fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<McpCallResult> {
        let params = serde_json::json!({
            "name": tool_name,
            "arguments": arguments
        });

        let result = self.send_request("tools/call", Some(params)).await?;

        let is_error = result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let content = result
            .get("content")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| serde_json::from_value::<McpContent>(v.clone()).ok())
                    .collect()
            })
            .unwrap_or_else(|| {
                // 兼容旧格式：直接返回字符串
                if let Some(text) = result.as_str() {
                    vec![McpContent {
                        content_type: "text".to_string(),
                        text: Some(text.to_string()),
                    }]
                } else {
                    vec![]
                }
            });

        Ok(McpCallResult { content, is_error })
    }
}

// ─────────────────────────────────────────────
// 快速连接（用于 CLI 工具列表展示，超时保护）
// ─────────────────────────────────────────────

/// 尝试连接 MCP 服务器并获取工具列表，带超时保护
/// 用于 /mcp 命令的工具浏览器
pub async fn fetch_tools_with_timeout(
    server_name: &str,
    command: &str,
    args: &[String],
    env: &HashMap<String, String>,
    timeout_secs: u64,
) -> Vec<McpToolInfo> {
    let timeout = tokio::time::Duration::from_secs(timeout_secs);
    let result = tokio::time::timeout(
        timeout,
        McpClient::connect(server_name, command, args, env),
    )
    .await;

    match result {
        Ok(Ok(client)) => client.tools,
        Ok(Err(e)) => {
            tracing::warn!("Failed to connect to MCP server '{}': {}", server_name, e);
            vec![]
        }
        Err(_) => {
            tracing::warn!("Timeout connecting to MCP server '{}'", server_name);
            vec![]
        }
    }
}
