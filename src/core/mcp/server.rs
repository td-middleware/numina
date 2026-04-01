use anyhow::Result;
use std::collections::HashMap;
use super::{McpMessage, McpResponse};

pub struct McpServer {
    name: String,
    tools: HashMap<String, McpTool>,
}

struct McpTool {
    name: String,
    description: String,
}

impl McpServer {
    pub fn new(name: String) -> Self {
        Self {
            name,
            tools: HashMap::new(),
        }
    }

    pub fn register_tool(&mut self, name: String, description: String) {
        self.tools.insert(name.clone(), McpTool { name, description });
    }

    pub async fn handle_message(&self, message: &McpMessage) -> Result<McpResponse> {
        match message.method.as_str() {
            "tools/list" => self.handle_list_tools(message),
            "tools/call" => self.handle_call_tool(message),
            "initialize" => self.handle_initialize(message),
            _ => Ok(McpResponse {
                id: message.id.clone(),
                result: None,
                error: Some(super::McpError {
                    code: -32601,
                    message: format!("Method not found: {}", message.method),
                    data: None,
                }),
            }),
        }
    }

    fn handle_list_tools(&self, message: &McpMessage) -> Result<McpResponse> {
        let tools: Vec<_> = self.tools.values().map(|t| {
            serde_json::json!({
                "name": t.name,
                "description": t.description
            })
        }).collect();

        Ok(McpResponse {
            id: message.id.clone(),
            result: Some(serde_json::json!({ "tools": tools })),
            error: None,
        })
    }

    fn handle_call_tool(&self, message: &McpMessage) -> Result<McpResponse> {
        // Simplified - in production, execute actual tool
        Ok(McpResponse {
            id: message.id.clone(),
            result: Some(serde_json::json!({ "result": "Tool executed" })),
            error: None,
        })
    }

    fn handle_initialize(&self, message: &McpMessage) -> Result<McpResponse> {
        Ok(McpResponse {
            id: message.id.clone(),
            result: Some(serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "serverInfo": {
                    "name": self.name,
                    "version": "1.0.0"
                }
            })),
            error: None,
        })
    }
}
