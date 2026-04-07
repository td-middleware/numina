use async_trait::async_trait;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::{ChatResponse, Message, ModelProvider, Role, Usage};
use super::provider::{ChatResponseWithTools, StopReason, ToolCall, ToolDefinition};

// ─────────────────────────────────────────────
// OpenAI request types
// ─────────────────────────────────────────────

#[derive(Serialize)]
struct OpenAIRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAITool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<serde_json::Value>,
}

#[derive(Serialize, Clone)]
struct OpenAIMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    /// 工具调用（assistant 消息）
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCallOut>>,
    /// 工具结果（tool 消息）
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    /// 工具名称（tool 消息）
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Serialize, Clone)]
struct OpenAIToolCallOut {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: OpenAIFunctionCall,
}

#[derive(Serialize, Clone)]
struct OpenAIFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Serialize)]
struct OpenAITool {
    #[serde(rename = "type")]
    tool_type: String,
    function: OpenAIFunctionDef,
}

#[derive(Serialize)]
struct OpenAIFunctionDef {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

// ─────────────────────────────────────────────
// OpenAI response types
// ─────────────────────────────────────────────

#[derive(Deserialize)]
struct OpenAIResponse {
    model: String,
    choices: Vec<OpenAIChoice>,
    usage: Option<OpenAIUsage>,
}

#[derive(Deserialize)]
struct OpenAIChoice {
    message: OpenAIChoiceMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct OpenAIChoiceMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAIToolCallIn>>,
}

#[derive(Deserialize)]
struct OpenAIToolCallIn {
    id: String,
    function: OpenAIFunctionCallIn,
}

#[derive(Deserialize)]
struct OpenAIFunctionCallIn {
    name: String,
    arguments: String,
}

#[derive(Deserialize)]
struct OpenAIUsage {
    prompt_tokens: usize,
    completion_tokens: usize,
    total_tokens: usize,
}

/// SSE chunk for streaming
#[derive(Deserialize)]
struct OpenAIStreamChunk {
    choices: Vec<OpenAIStreamChoice>,
}

#[derive(Deserialize)]
struct OpenAIStreamChoice {
    delta: OpenAIDelta,
}

#[derive(Deserialize)]
struct OpenAIDelta {
    /// 标准 OpenAI content 字段
    #[serde(default)]
    content: String,
    /// MiniMax 等模型的推理内容字段（reasoning_content）
    #[serde(default)]
    reasoning_content: String,
}

// ─────────────────────────────────────────────
// Provider
// ─────────────────────────────────────────────

pub struct OpenAIProvider {
    api_key: String,
    model: String,
    endpoint: String,
    client: reqwest::Client,
}

impl OpenAIProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            endpoint: "https://api.openai.com/v1".to_string(),
            client: reqwest::Client::new(),
        }
    }

    pub fn with_endpoint(mut self, endpoint: String) -> Self {
        self.endpoint = endpoint.trim_end_matches('/').to_string();
        self
    }

    fn role_str(role: &Role) -> &'static str {
        match role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }

    /// 将通用 Message 列表转换为 OpenAI 格式
    /// 正确处理三种消息类型：
    /// 1. 普通消息（system/user/assistant）：content 字段
    /// 2. Assistant 工具调用消息：tool_calls 字段（content 可为 null）
    /// 3. Tool 结果消息：role=tool + tool_call_id + name + content
    fn convert_messages(messages: &[Message]) -> Vec<OpenAIMessage> {
        messages
            .iter()
            .map(|m| {
                match m.role {
                    Role::Tool => {
                        // Tool 结果消息：必须有 tool_call_id
                        OpenAIMessage {
                            role: "tool".to_string(),
                            content: Some(m.content.clone()),
                            tool_calls: None,
                            tool_call_id: m.tool_call_id.clone(),
                            name: m.tool_name.clone(),
                        }
                    }
                    Role::Assistant if !m.tool_calls.is_empty() => {
                        // Assistant 工具调用消息：包含 tool_calls 字段
                        let oai_tool_calls: Vec<OpenAIToolCallOut> = m.tool_calls.iter().map(|tc| {
                            OpenAIToolCallOut {
                                id: tc.id.clone(),
                                call_type: "function".to_string(),
                                function: OpenAIFunctionCall {
                                    name: tc.name.clone(),
                                    arguments: serde_json::to_string(&tc.arguments)
                                        .unwrap_or_else(|_| "{}".to_string()),
                                },
                            }
                        }).collect();
                        OpenAIMessage {
                            role: "assistant".to_string(),
                            // content 可以为 null（OpenAI 规范允许）
                            content: if m.content.is_empty() { None } else { Some(m.content.clone()) },
                            tool_calls: Some(oai_tool_calls),
                            tool_call_id: None,
                            name: None,
                        }
                    }
                    _ => {
                        // 普通消息
                        OpenAIMessage {
                            role: Self::role_str(&m.role).to_string(),
                            content: Some(m.content.clone()),
                            tool_calls: None,
                            tool_call_id: None,
                            name: None,
                        }
                    }
                }
            })
            .collect()
    }

    /// 将 ToolDefinition 列表转换为 OpenAI tools 格式
    fn convert_tools(tools: &[ToolDefinition]) -> Vec<OpenAITool> {
        tools
            .iter()
            .map(|t| OpenAITool {
                tool_type: "function".to_string(),
                function: OpenAIFunctionDef {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                },
            })
            .collect()
    }
}

#[async_trait]
impl ModelProvider for OpenAIProvider {
    async fn chat(&self, messages: &[Message]) -> Result<ChatResponse> {
        if self.api_key.is_empty() {
            anyhow::bail!(
                "OpenAI API key not set. Use `numina model add {} --provider openai --api-key <KEY>` \
                 or set OPENAI_API_KEY environment variable.",
                self.model
            );
        }

        let req_body = OpenAIRequest {
            model: self.model.clone(),
            messages: Self::convert_messages(messages),
            stream: None,
            max_tokens: Some(4096),
            temperature: Some(0.7),
            tools: None,
            tool_choice: None,
        };

        let url = format!("{}/chat/completions", self.endpoint);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&req_body)
            .send()
            .await
            .context("Failed to send request to OpenAI")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI API error {}: {}", status, body);
        }

        let oai_resp: OpenAIResponse = resp
            .json()
            .await
            .context("Failed to parse OpenAI response")?;

        let content = oai_resp
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .unwrap_or_default();

        let usage = oai_resp.usage.map(|u| Usage {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
        }).unwrap_or_default();

        Ok(ChatResponse {
            content,
            model: oai_resp.model,
            usage,
        })
    }

    async fn chat_stream(&self, messages: &[Message]) -> Result<mpsc::Receiver<String>> {
        if self.api_key.is_empty() {
            anyhow::bail!(
                "OpenAI API key not set. Set OPENAI_API_KEY or use `numina model add`."
            );
        }

        let req_body = OpenAIRequest {
            model: self.model.clone(),
            messages: Self::convert_messages(messages),
            stream: Some(true),
            max_tokens: Some(4096),
            temperature: Some(0.7),
            tools: None,
            tool_choice: None,
        };

        let url = format!("{}/chat/completions", self.endpoint);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&req_body)
            .send()
            .await
            .context("Failed to send streaming request to OpenAI")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI streaming API error {}: {}", status, body);
        }

        let (tx, rx) = mpsc::channel::<String>(256);

        tokio::spawn(async move {
            use futures::StreamExt;
            let mut stream = resp.bytes_stream();
            let mut buf = String::new();

            while let Some(chunk) = stream.next().await {
                let bytes = match chunk { Ok(b) => b, Err(_) => break };
                buf.push_str(&String::from_utf8_lossy(&bytes));

                loop {
                    let newline_pos = buf.find('\n');
                    match newline_pos {
                        None => break,
                        Some(pos) => {
                            let line = buf[..pos].trim_end_matches('\r').to_string();
                            buf = buf[pos + 1..].to_string();

                            // 兼容 "data: {...}" 和 "data:{...}" 两种格式
                            let data_opt = if line.starts_with("data: ") {
                                Some(line["data: ".len()..].trim())
                            } else if line.starts_with("data:") {
                                Some(line["data:".len()..].trim())
                            } else {
                                None
                            };

                            if let Some(data) = data_opt {
                                if data == "[DONE]" {
                                    return;
                                }
                                if !data.is_empty() {
                                    if let Ok(chunk) = serde_json::from_str::<OpenAIStreamChunk>(data) {
                                        for choice in chunk.choices {
                                            // reasoning_content 用暗灰色包裹，content 正常输出
                                            // 格式："\x00R" 前缀表示 reasoning，"\x00C" 前缀表示 content
                                            if !choice.delta.reasoning_content.is_empty() {
                                                let token = format!("\x00R{}", choice.delta.reasoning_content);
                                                let _ = tx.send(token).await;
                                            } else if !choice.delta.content.is_empty() {
                                                let token = format!("\x00C{}", choice.delta.content);
                                                let _ = tx.send(token).await;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });

        Ok(rx)
    }

    /// 带工具的对话（OpenAI function calling）
    async fn chat_with_tools(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<ChatResponseWithTools> {
        if self.api_key.is_empty() {
            anyhow::bail!(
                "OpenAI API key not set. Use `numina model add` or set OPENAI_API_KEY."
            );
        }

        let oai_tools = if tools.is_empty() {
            None
        } else {
            Some(Self::convert_tools(tools))
        };

        let tool_choice = if tools.is_empty() {
            None
        } else {
            Some(serde_json::json!("auto"))
        };

        let req_body = OpenAIRequest {
            model: self.model.clone(),
            messages: Self::convert_messages(messages),
            stream: None,
            max_tokens: Some(4096),
            temperature: Some(0.7),
            tools: oai_tools,
            tool_choice,
        };

        let url = format!("{}/chat/completions", self.endpoint);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&req_body)
            .send()
            .await
            .context("Failed to send tool-calling request to OpenAI")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI tool-calling API error {}: {}", status, body);
        }

        let oai_resp: OpenAIResponse = resp
            .json()
            .await
            .context("Failed to parse OpenAI tool-calling response")?;

        let usage = oai_resp.usage.map(|u| Usage {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
        }).unwrap_or_default();

        let choice = oai_resp.choices.into_iter().next();
        let finish_reason = choice
            .as_ref()
            .and_then(|c| c.finish_reason.as_deref())
            .unwrap_or("stop");
        let stop_reason = StopReason::from_str(finish_reason);

        let content = choice
            .as_ref()
            .and_then(|c| c.message.content.clone())
            .unwrap_or_default();

        let tool_calls = choice
            .and_then(|c| c.message.tool_calls)
            .unwrap_or_default()
            .into_iter()
            .map(|tc| {
                let arguments = serde_json::from_str(&tc.function.arguments)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                ToolCall {
                    id: tc.id,
                    name: tc.function.name,
                    arguments,
                }
            })
            .collect();

        Ok(ChatResponseWithTools {
            content,
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
            || std::env::var("OPENAI_API_KEY").is_ok()
    }
}
