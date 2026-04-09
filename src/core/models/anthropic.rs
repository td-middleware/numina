use async_trait::async_trait;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::{ChatResponse, Message, ModelProvider, Role, Usage};
use super::provider::{ChatResponseWithTools, StopReason, ToolCall, ToolDefinition};

// ─────────────────────────────────────────────
// Anthropic request / response types（普通对话）
// ─────────────────────────────────────────────

#[derive(Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
    messages: Vec<AnthropicMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Serialize)]
struct AnthropicMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    model: String,
    content: Vec<AnthropicContent>,
    usage: Option<AnthropicUsage>,
}

#[derive(Deserialize)]
struct AnthropicContent {
    #[serde(rename = "type")]
    content_type: String,
    text: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicUsage {
    input_tokens: usize,
    output_tokens: usize,
}

/// SSE event types for streaming
#[derive(Deserialize)]
struct AnthropicStreamEvent {
    #[serde(rename = "type")]
    event_type: String,
    delta: Option<AnthropicStreamDelta>,
}

#[derive(Deserialize)]
struct AnthropicStreamDelta {
    #[serde(rename = "type")]
    delta_type: Option<String>,
    text: Option<String>,
}

// ─────────────────────────────────────────────
// Anthropic tool use request / response types
// ─────────────────────────────────────────────

/// 带工具的请求体
#[derive(Serialize)]
struct AnthropicToolRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicToolMsg>,
    tools: Vec<AnthropicToolDef>,
}

/// 工具定义（Anthropic 格式）
#[derive(Serialize)]
struct AnthropicToolDef {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

/// 消息（content 可以是字符串或 content block 数组）
#[derive(Serialize)]
struct AnthropicToolMsg {
    role: String,
    content: serde_json::Value,
}

/// 带工具的响应体
#[derive(Deserialize)]
struct AnthropicToolResponse {
    model: String,
    stop_reason: Option<String>,
    content: Vec<AnthropicToolBlock>,
    usage: Option<AnthropicUsage>,
}

/// 响应中的 content block
#[derive(Deserialize)]
struct AnthropicToolBlock {
    #[serde(rename = "type")]
    block_type: String,
    /// text block
    text: Option<String>,
    /// tool_use block
    id: Option<String>,
    name: Option<String>,
    input: Option<serde_json::Value>,
}

// ─────────────────────────────────────────────
// Provider
// ─────────────────────────────────────────────

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            client: reqwest::Client::new(),
        }
    }

    /// 将 messages 分离为 system prompt 和 user/assistant 轮次
    fn split_messages<'a>(messages: &'a [Message]) -> (Option<&'a str>, Vec<AnthropicMessage<'a>>) {
        let mut system: Option<&'a str> = None;
        let mut turns: Vec<AnthropicMessage<'a>> = Vec::new();

        for msg in messages {
            match msg.role {
                Role::System => {
                    system = Some(&msg.content);
                }
                Role::User => {
                    turns.push(AnthropicMessage {
                        role: "user",
                        content: &msg.content,
                    });
                }
                Role::Assistant => {
                    turns.push(AnthropicMessage {
                        role: "assistant",
                        content: &msg.content,
                    });
                }
                Role::Tool => {
                    turns.push(AnthropicMessage {
                        role: "user",
                        content: &msg.content,
                    });
                }
            }
        }

        (system, turns)
    }

    /// 将通用 Message 列表转换为 Anthropic tool use 格式的消息列表
    /// 正确处理：
    /// 1. system → 单独的 system 字段
    /// 2. user/assistant 普通消息 → {"role": "...", "content": "..."}
    /// 3. assistant 工具调用消息 → content 为 block 数组（text + tool_use）
    /// 4. tool 结果消息 → role=user, content 为 tool_result block 数组
    fn convert_messages_for_tools(messages: &[Message]) -> (Option<String>, Vec<AnthropicToolMsg>) {
        let mut system: Option<String> = None;
        let mut turns: Vec<AnthropicToolMsg> = Vec::new();

        for msg in messages {
            match msg.role {
                Role::System => {
                    system = Some(msg.content.clone());
                }
                Role::User => {
                    turns.push(AnthropicToolMsg {
                        role: "user".to_string(),
                        content: serde_json::Value::String(msg.content.clone()),
                    });
                }
                Role::Assistant if !msg.tool_calls.is_empty() => {
                    // Assistant 工具调用消息：content 为 block 数组
                    let mut blocks: Vec<serde_json::Value> = Vec::new();
                    // 先加文本内容（如果有）
                    if !msg.content.is_empty() {
                        blocks.push(serde_json::json!({
                            "type": "text",
                            "text": msg.content
                        }));
                    }
                    // 再加工具调用 blocks
                    for tc in &msg.tool_calls {
                        blocks.push(serde_json::json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.name,
                            "input": tc.arguments
                        }));
                    }
                    turns.push(AnthropicToolMsg {
                        role: "assistant".to_string(),
                        content: serde_json::Value::Array(blocks),
                    });
                }
                Role::Assistant => {
                    turns.push(AnthropicToolMsg {
                        role: "assistant".to_string(),
                        content: serde_json::Value::String(msg.content.clone()),
                    });
                }
                Role::Tool => {
                    // Tool 结果消息：role=user, content 为 tool_result block
                    let block = serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": msg.tool_call_id.as_deref().unwrap_or(""),
                        "content": msg.content
                    });
                    turns.push(AnthropicToolMsg {
                        role: "user".to_string(),
                        content: serde_json::Value::Array(vec![block]),
                    });
                }
            }
        }

        (system, turns)
    }
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    async fn chat(&self, messages: &[Message]) -> Result<ChatResponse> {
        if self.api_key.is_empty() {
            anyhow::bail!(
                "Anthropic API key not set. Use `numina model add {} --provider anthropic --api-key <KEY>` \
                 or set ANTHROPIC_API_KEY environment variable.",
                self.model
            );
        }

        let (system, turns) = Self::split_messages(messages);

        let req_body = AnthropicRequest {
            model: &self.model,
            max_tokens: 4096,
            system,
            messages: turns,
            stream: None,
        };

        let resp = self
            .client
            .post(ANTHROPIC_API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&req_body)
            .send()
            .await
            .context("Failed to send request to Anthropic")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Anthropic API error {}: {}", status, body);
        }

        let ant_resp: AnthropicResponse = resp
            .json()
            .await
            .context("Failed to parse Anthropic response")?;

        let content = ant_resp
            .content
            .into_iter()
            .filter(|c| c.content_type == "text")
            .filter_map(|c| c.text)
            .collect::<Vec<_>>()
            .join("");

        let usage = ant_resp.usage.map(|u| Usage {
            prompt_tokens: u.input_tokens,
            completion_tokens: u.output_tokens,
            total_tokens: u.input_tokens + u.output_tokens,
        }).unwrap_or(Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        });

        Ok(ChatResponse {
            content,
            model: ant_resp.model,
            usage,
        })
    }

    async fn chat_stream(&self, messages: &[Message]) -> Result<mpsc::Receiver<String>> {
        if self.api_key.is_empty() {
            anyhow::bail!(
                "Anthropic API key not set. Set ANTHROPIC_API_KEY or use `numina model add`."
            );
        }

        let (system, turns) = Self::split_messages(messages);

        let req_body = AnthropicRequest {
            model: &self.model,
            max_tokens: 4096,
            system,
            messages: turns,
            stream: Some(true),
        };

        let resp = self
            .client
            .post(ANTHROPIC_API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&req_body)
            .send()
            .await
            .context("Failed to send streaming request to Anthropic")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Anthropic streaming API error {}: {}", status, body);
        }

        let (tx, rx) = mpsc::channel::<String>(256);

        tokio::spawn(async move {
            use futures::StreamExt;
            let mut stream = resp.bytes_stream();
            let mut buf = String::new();

            while let Some(chunk) = stream.next().await {
                let bytes = match chunk { Ok(b) => b, Err(_) => break };
                buf.push_str(&String::from_utf8_lossy(&bytes));

                // Anthropic SSE: "event: ...\ndata: {...}\n\n"
                while let Some(pos) = buf.find('\n') {
                    let line = buf[..pos].trim().to_string();
                    buf = buf[pos + 1..].to_string();

                    if line.starts_with("data: ") {
                        let data = &line["data: ".len()..];
                        if let Ok(event) = serde_json::from_str::<AnthropicStreamEvent>(data) {
                            if event.event_type == "content_block_delta" {
                                if let Some(delta) = event.delta {
                                    if delta.delta_type.as_deref() == Some("text_delta") {
                                        if let Some(text) = delta.text {
                                            if !text.is_empty() {
                                                let _ = tx.send(text).await;
                                            }
                                        }
                                    }
                                }
                            } else if event.event_type == "message_stop" {
                                return;
                            }
                        }
                    }
                }
            }
        });

        Ok(rx)
    }

    /// 带工具的对话（Anthropic tool use API）
    async fn chat_with_tools(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<ChatResponseWithTools> {
        if self.api_key.is_empty() {
            anyhow::bail!(
                "Anthropic API key not set. Use `numina model add` or set ANTHROPIC_API_KEY."
            );
        }

        // 如果没有工具，退化为普通 chat
        if tools.is_empty() {
            let resp = self.chat(messages).await?;
            return Ok(ChatResponseWithTools {
                content: resp.content,
                tool_calls: vec![],
                stop_reason: StopReason::Stop,
                usage: resp.usage,
            });
        }

        // 转换消息格式
        let (system, turns) = Self::convert_messages_for_tools(messages);

        // 转换工具定义
        let ant_tools: Vec<AnthropicToolDef> = tools.iter().map(|t| {
            AnthropicToolDef {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.parameters.clone(),
            }
        }).collect();

        let req_body = AnthropicToolRequest {
            model: self.model.clone(),
            max_tokens: 4096,
            system,
            messages: turns,
            tools: ant_tools,
        };

        let resp = self
            .client
            .post(ANTHROPIC_API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&req_body)
            .send()
            .await
            .context("Failed to send tool-calling request to Anthropic")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Anthropic tool-calling API error {}: {}", status, body);
        }

        let ant_resp: AnthropicToolResponse = resp
            .json()
            .await
            .context("Failed to parse Anthropic tool-calling response")?;

        let usage = ant_resp.usage.map(|u| Usage {
            prompt_tokens: u.input_tokens,
            completion_tokens: u.output_tokens,
            total_tokens: u.input_tokens + u.output_tokens,
        }).unwrap_or_default();

        // 解析 stop_reason
        let stop_reason = match ant_resp.stop_reason.as_deref() {
            Some("tool_use") => StopReason::ToolCalls,
            Some("end_turn") => StopReason::Stop,
            Some("max_tokens") => StopReason::Length,
            _ => StopReason::Stop,
        };

        // 从 content blocks 中提取文本和工具调用
        let mut text_parts: Vec<String> = Vec::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        for block in ant_resp.content {
            match block.block_type.as_str() {
                "text" => {
                    if let Some(text) = block.text {
                        if !text.is_empty() {
                            text_parts.push(text);
                        }
                    }
                }
                "tool_use" => {
                    tool_calls.push(ToolCall {
                        id: block.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                        name: block.name.unwrap_or_default(),
                        arguments: block.input.unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
                    });
                }
                _ => {}
            }
        }

        Ok(ChatResponseWithTools {
            content: text_parts.join(""),
            tool_calls,
            stop_reason,
            usage,
        })
    }

    fn name(&self) -> &str {
        &self.model
    }

    fn is_available(&self) -> bool {
        !self.api_key.is_empty()
            || std::env::var("ANTHROPIC_API_KEY").is_ok()
    }
}
