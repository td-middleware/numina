use async_trait::async_trait;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::{ChatResponse, Message, ModelProvider, Role, Usage};

// ─────────────────────────────────────────────
// OpenAI request / response types
// ─────────────────────────────────────────────

#[derive(Serialize)]
struct OpenAIRequest<'a> {
    model: &'a str,
    messages: Vec<OpenAIMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Serialize)]
struct OpenAIMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct OpenAIResponse {
    model: String,
    choices: Vec<OpenAIChoice>,
    usage: Option<OpenAIUsage>,
}

#[derive(Deserialize)]
struct OpenAIChoice {
    message: OpenAIChoiceMessage,
}

#[derive(Deserialize)]
struct OpenAIChoiceMessage {
    content: String,
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
        // 去掉末尾斜杠，统一格式
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

        let oai_messages: Vec<OpenAIMessage> = messages
            .iter()
            .map(|m| OpenAIMessage {
                role: Self::role_str(&m.role),
                content: &m.content,
            })
            .collect();

        let req_body = OpenAIRequest {
            model: &self.model,
            messages: oai_messages,
            stream: None,
            max_tokens: Some(4096),
            temperature: Some(0.7),
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
            .map(|c| c.message.content)
            .unwrap_or_default();

        let usage = oai_resp.usage.map(|u| Usage {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
        }).unwrap_or(Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        });

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

        let oai_messages: Vec<OpenAIMessage> = messages
            .iter()
            .map(|m| OpenAIMessage {
                role: Self::role_str(&m.role),
                content: &m.content,
            })
            .collect();

        let req_body = OpenAIRequest {
            model: &self.model,
            messages: oai_messages,
            stream: Some(true),
            max_tokens: Some(4096),
            temperature: Some(0.7),
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

                // SSE 格式：每个事件以 \n\n 或 \r\n\r\n 结尾
                // 按行处理，兼容 \r\n 和 \n
                loop {
                    // 找到第一个换行符位置
                    let newline_pos = buf.find('\n');
                    match newline_pos {
                        None => break,
                        Some(pos) => {
                            // 提取一行，去掉末尾的 \r
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
                                            // 优先使用 content，其次使用 reasoning_content
                                            let token = if !choice.delta.content.is_empty() {
                                                choice.delta.content
                                            } else if !choice.delta.reasoning_content.is_empty() {
                                                choice.delta.reasoning_content
                                            } else {
                                                continue;
                                            };
                                            let _ = tx.send(token).await;
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

    fn name(&self) -> &str {
        &self.model
    }

    fn is_available(&self) -> bool {
        !self.api_key.is_empty()
            || std::env::var("OPENAI_API_KEY").is_ok()
    }
}
