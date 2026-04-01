use async_trait::async_trait;
use super::{ModelProvider, Message, ChatResponse, Usage};
use anyhow::Result;

pub struct LocalProvider {
    model_path: String,
    model_name: String,
}

impl LocalProvider {
    pub fn new(model_path: String, model_name: String) -> Self {
        Self {
            model_path,
            model_name,
        }
    }
}

#[async_trait]
impl ModelProvider for LocalProvider {
    async fn chat(&self, messages: &[Message]) -> Result<ChatResponse> {
        // In production, use actual local model (llama.cpp, ollama, etc.)
        let content = format!("Local model {} response for {} messages", self.model_name, messages.len());
        
        Ok(ChatResponse {
            content,
            model: self.model_name.clone(),
            usage: Usage {
                prompt_tokens: 150,
                completion_tokens: 100,
                total_tokens: 250,
            },
        })
    }

    async fn chat_stream(&self, messages: &[Message]) -> Result<tokio::sync::mpsc::Receiver<String>> {
        let (tx, rx) = tokio::sync::mpsc::channel(10);
        
        let content = format!("Streamed local response from {}", self.model_name);
        for ch in content.chars() {
            let _ = tx.send(ch.to_string()).await;
        }
        
        Ok(rx)
    }

    fn name(&self) -> &str {
        &self.model_name
    }

    fn is_available(&self) -> bool {
        std::path::Path::new(&self.model_path).exists()
    }
}
