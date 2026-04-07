pub mod registry;
pub mod builtin;
pub mod mcp;

pub use registry::ToolRegistry;
pub use builtin::BuiltinTool;
pub use mcp::McpTool;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub tool_type: ToolType,
    pub parameters: serde_json::Value,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolType {
    Builtin,
    MCP,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub data: serde_json::Value,
    pub error: Option<String>,
}

#[async_trait::async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute(&self, parameters: serde_json::Value) -> anyhow::Result<ToolResult>;
    fn name(&self) -> &str;
    fn description(&self) -> &str;

    /// JSON Schema for the tool's parameters (used for function calling)
    /// Default implementation returns a permissive schema.
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": true
        })
    }
}
