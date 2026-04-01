use async_trait::async_trait;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::{ChatResponse, Message, ModelProvider, Role, Usage};

// ─────────────────────────────────────────────
// Anthropic request / response types
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

    fn name(&self) -> &str {
        &self.model
    }

    fn is_available(&self) -> bool {
        !self.api_key.is_empty()
            || std::env::var("ANTHROPIC_API_KEY").is_ok()
    }
}
