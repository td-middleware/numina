/// MCP 客户端 — 通过 stdio 或 HTTP/HTTPS 连接外部 MCP 服务器
///
/// 协议：JSON-RPC 2.0 over stdio 或 HTTP POST (Streamable HTTP transport)
/// 参考：https://modelcontextprotocol.io/docs/concepts/transports

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
// McpClient (stdio)
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
// HTTP/HTTPS MCP 客户端（Streamable HTTP transport）
// ─────────────────────────────────────────────

/// 通过 HTTP POST 发送 JSON-RPC 请求到 MCP 服务器
async fn http_send_request(
    client: &reqwest::Client,
    url: &str,
    method: &str,
    params: Option<Value>,
    id: u64,
) -> Result<Value> {
    let req_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params.unwrap_or(serde_json::json!({}))
    });

    let resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&req_body)
        .send()
        .await
        .with_context(|| format!("HTTP request to MCP server failed: {}", url))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("MCP HTTP server returned {}: {}", status, body);
    }

    let content_type = resp.headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // 处理 SSE (text/event-stream) 响应
    if content_type.contains("text/event-stream") {
        let body = resp.text().await?;
        // 解析 SSE 格式：找到 data: 行
        for line in body.lines() {
            let line = line.trim();
            if let Some(data) = line.strip_prefix("data:") {
                let data = data.trim();
                if data == "[DONE]" || data.is_empty() {
                    continue;
                }
                if let Ok(json_resp) = serde_json::from_str::<JsonRpcResponse>(data) {
                    if let Some(err) = json_resp.error {
                        anyhow::bail!("MCP HTTP error: [{}] {}", err.code, err.message);
                    }
                    if let Some(result) = json_resp.result {
                        return Ok(result);
                    }
                }
            }
        }
        anyhow::bail!("No valid JSON-RPC response found in SSE stream");
    } else {
        // 普通 JSON 响应
        let json_resp: JsonRpcResponse = resp.json().await
            .with_context(|| "Failed to parse MCP HTTP response as JSON")?;

        if let Some(err) = json_resp.error {
            anyhow::bail!("MCP HTTP error: [{}] {}", err.code, err.message);
        }

        json_resp.result.ok_or_else(|| anyhow::anyhow!("MCP HTTP response has no result"))
    }
}

/// 通过 HTTP/HTTPS 获取 MCP 工具列表
pub async fn fetch_tools_http(
    server_name: &str,
    url: &str,
    headers: &HashMap<String, String>,
    timeout_secs: u64,
) -> Result<Vec<McpToolInfo>> {
    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs));

    // 对 https 启用 TLS（rustls）
    // reqwest 默认已启用，这里显式设置
    let client = builder.build()
        .with_context(|| "Failed to build HTTP client")?;

    // 构建带自定义 headers 的请求
    let mut default_headers = reqwest::header::HeaderMap::new();
    for (k, v) in headers {
        if let (Ok(name), Ok(val)) = (
            reqwest::header::HeaderName::from_bytes(k.as_bytes()),
            reqwest::header::HeaderValue::from_str(v),
        ) {
            default_headers.insert(name, val);
        }
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .default_headers(default_headers)
        .build()
        .with_context(|| "Failed to build HTTP client with headers")?;

    // Step 1: initialize
    let init_params = serde_json::json!({
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

    http_send_request(&client, url, "initialize", Some(init_params), 1).await
        .with_context(|| format!("HTTP MCP initialize failed for '{}'", server_name))?;

    // Step 2: initialized notification（忽略错误）
    let _ = client
        .post(url)
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }))
        .send()
        .await;

    // Step 3: tools/list
    let result = http_send_request(&client, url, "tools/list", None, 2).await
        .with_context(|| format!("HTTP MCP tools/list failed for '{}'", server_name))?;

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

/// 检测 HTTP/HTTPS MCP 服务器是否可达（快速 ping）
pub async fn check_http_reachable(url: &str, headers: &HashMap<String, String>, timeout_secs: u64) -> bool {
    let mut header_map = reqwest::header::HeaderMap::new();
    for (k, v) in headers {
        if let (Ok(name), Ok(val)) = (
            reqwest::header::HeaderName::from_bytes(k.as_bytes()),
            reqwest::header::HeaderValue::from_str(v),
        ) {
            header_map.insert(name, val);
        }
    }

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .default_headers(header_map)
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };

    // 发送一个简单的 initialize 请求来检测连通性
    let req_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "numina", "version": "0.1.0" }
        }
    });

    match client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&req_body)
        .send()
        .await
    {
        Ok(resp) => resp.status().is_success() || resp.status().as_u16() == 405,
        Err(_) => false,
    }
}

// ─────────────────────────────────────────────
// 快速连接（用于 CLI 工具列表展示，超时保护）
// ─────────────────────────────────────────────

/// 尝试连接 MCP 服务器并获取工具列表，带超时保护
/// 支持 stdio、http、https 类型
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

/// 通过 HTTP/HTTPS 获取工具列表（带超时保护）
pub async fn fetch_tools_http_with_timeout(
    server_name: &str,
    url: &str,
    headers: &HashMap<String, String>,
    timeout_secs: u64,
) -> Vec<McpToolInfo> {
    let timeout = tokio::time::Duration::from_secs(timeout_secs);
    let result = tokio::time::timeout(
        timeout,
        fetch_tools_http(server_name, url, headers, timeout_secs),
    )
    .await;

    match result {
        Ok(Ok(tools)) => tools,
        Ok(Err(e)) => {
            tracing::warn!("Failed to fetch tools from HTTP MCP server '{}': {}", server_name, e);
            vec![]
        }
        Err(_) => {
            tracing::warn!("Timeout fetching tools from HTTP MCP server '{}'", server_name);
            vec![]
        }
    }
}
