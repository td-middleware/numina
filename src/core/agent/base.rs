use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: Uuid,
    pub name: String,
    pub role: String,
    pub description: Option<String>,
    pub model: String,
    pub capabilities: Vec<String>,
    pub status: AgentStatus,
    pub config: AgentConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentStatus {
    Idle,
    Active,
    Busy,
    Paused,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub temperature: f32,
    pub max_tokens: usize,
    pub timeout_seconds: u64,
    pub retry_count: u32,
}

impl Agent {
    pub fn new(name: String, role: String, model: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            role,
            description: None,
            model,
            capabilities: Vec::new(),
            status: AgentStatus::Idle,
            config: AgentConfig {
                temperature: 0.7,
                max_tokens: 4096,
                timeout_seconds: 300,
                retry_count: 3,
            },
        }
    }

    pub fn with_description(mut self, description: String) -> Self {
        self.description = Some(description);
        self
    }

    pub fn with_capability(mut self, capability: String) -> Self {
        self.capabilities.push(capability);
        self
    }

    pub fn set_status(&mut self, status: AgentStatus) {
        self.status = status;
    }
}
