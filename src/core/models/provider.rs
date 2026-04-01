use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    async fn chat(&self, messages: &[Message]) -> anyhow::Result<super::ChatResponse>;
    
    async fn chat_stream(&self, messages: &[Message]) -> anyhow::Result<tokio::sync::mpsc::Receiver<String>>;
    
    fn name(&self) -> &str;
    
    fn is_available(&self) -> bool;
}
