use anyhow::Result;
use super::{Agent, AgentStatus};
use super::memory::AgentMemory;

pub struct AgentExecutor {
    agent: Agent,
    memory: AgentMemory,
}

impl AgentExecutor {
    pub fn new(agent: Agent) -> Self {
        let memory = AgentMemory::new(&agent.id);
        Self { agent, memory }
    }

    pub async fn execute_task(&mut self, task: &str) -> Result<String> {
        self.agent.set_status(AgentStatus::Busy);
        
        // Store task in memory
        self.memory.add_entry("task", task).await?;
        
        // Execute task (simplified)
        let result = format!("Task executed by {}: {}", self.agent.name, task);
        
        self.memory.add_entry("result", &result).await?;
        self.agent.set_status(AgentStatus::Idle);
        
        Ok(result)
    }

    pub fn agent(&self) -> &Agent {
        &self.agent
    }

    pub fn agent_mut(&mut self) -> &mut Agent {
        &mut self.agent
    }
}
