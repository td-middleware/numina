use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: Uuid,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub memory_type: MemoryType,
    pub content: String,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MemoryType {
    Task,
    Result,
    Observation,
    Reflection,
    UserMessage,
    SystemMessage,
}

pub struct AgentMemory {
    agent_id: Uuid,
    entries: Vec<MemoryEntry>,
}

impl AgentMemory {
    pub fn new(agent_id: &Uuid) -> Self {
        Self {
            agent_id: *agent_id,
            entries: Vec::new(),
        }
    }

    pub async fn add_entry(&mut self, memory_type: &str, content: &str) -> Result<Uuid> {
        let entry = MemoryEntry {
            id: Uuid::new_v4(),
            timestamp: chrono::Utc::now(),
            memory_type: match memory_type {
                "task" => MemoryType::Task,
                "result" => MemoryType::Result,
                "observation" => MemoryType::Observation,
                "reflection" => MemoryType::Reflection,
                "user_message" => MemoryType::UserMessage,
                "system_message" => MemoryType::SystemMessage,
                _ => MemoryType::Observation,
            },
            content: content.to_string(),
            metadata: HashMap::new(),
        };
        
        let id = entry.id;
        self.entries.push(entry);
        Ok(id)
    }

    pub fn get_entries(&self, limit: Option<usize>) -> Vec<&MemoryEntry> {
        let limit = limit.unwrap_or(self.entries.len());
        self.entries.iter().rev().take(limit).collect()
    }

    pub fn search(&self, query: &str) -> Vec<&MemoryEntry> {
        self.entries
            .iter()
            .filter(|e| e.content.contains(query))
            .collect()
    }
}
