use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use super::{Tool, ToolExecutor};

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn ToolExecutor>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, executor: Arc<dyn ToolExecutor>) -> Result<()> {
        let name = executor.name().to_string();
        self.tools.insert(name, executor);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn ToolExecutor>> {
        self.tools.get(name)
    }

    pub fn list_tools(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    pub async fn execute(&self, name: &str, parameters: serde_json::Value) -> Result<super::ToolResult> {
        let executor = self.get(name)
            .ok_or_else(|| anyhow::anyhow!("Tool not found: {}", name))?;
        executor.execute(parameters).await
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
