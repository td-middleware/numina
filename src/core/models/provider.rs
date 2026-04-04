use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────
// 基础消息类型
// ─────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

// ─────────────────────────────────────────────
// Function Calling / Tool Use 类型
// ─────────────────────────────────────────────

/// 工具定义（发送给模型的 schema）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// 工具名称
    pub name: String,
    /// 工具描述
    pub description: String,
    /// 参数 JSON Schema
    pub parameters: serde_json::Value,
}

/// 模型请求调用某个工具
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// 工具调用 ID（用于匹配 tool result）
    pub id: String,
    /// 工具名称
    pub name: String,
    /// 工具参数（JSON）
    pub arguments: serde_json::Value,
}

/// 工具执行结果（回传给模型）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// 对应的工具调用 ID
    pub tool_call_id: String,
    /// 工具名称
    pub name: String,
    /// 执行结果内容
    pub content: String,
}

/// 带工具调用的响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponseWithTools {
    /// 文本内容（可能为空，如果模型只调用工具）
    pub content: String,
    /// 工具调用列表（可能为空）
    pub tool_calls: Vec<ToolCall>,
    /// 停止原因：stop | tool_calls | length
    pub stop_reason: StopReason,
    /// token 使用量
    pub usage: super::Usage,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StopReason {
    /// 正常结束
    Stop,
    /// 模型请求调用工具
    ToolCalls,
    /// 达到 max_tokens
    Length,
    /// 其他
    Other(String),
}

impl StopReason {
    pub fn from_str(s: &str) -> Self {
        match s {
            "stop" => Self::Stop,
            "tool_calls" | "tool_use" => Self::ToolCalls,
            "length" | "max_tokens" => Self::Length,
            other => Self::Other(other.to_string()),
        }
    }
}

// ─────────────────────────────────────────────
// ModelProvider trait
// ─────────────────────────────────────────────

#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// 普通对话（无工具）
    async fn chat(&self, messages: &[Message]) -> anyhow::Result<super::ChatResponse>;

    /// 流式对话（无工具）
    async fn chat_stream(
        &self,
        messages: &[Message],
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<String>>;

    /// 带工具定义的对话（function calling）
    /// 默认实现：忽略工具，退化为普通 chat
    async fn chat_with_tools(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> anyhow::Result<ChatResponseWithTools> {
        let _ = tools;
        let resp = self.chat(messages).await?;
        Ok(ChatResponseWithTools {
            content: resp.content,
            tool_calls: vec![],
            stop_reason: StopReason::Stop,
            usage: resp.usage,
        })
    }

    fn name(&self) -> &str;

    fn is_available(&self) -> bool;
}
