/// ReAct Agent — Reason + Act 循环引擎（Function Calling 版）
///
/// 工作流程：
/// 1. 用户给出任务目标
/// 2. 将工具列表以 OpenAI function calling 格式发送给模型
/// 3. 模型返回 tool_calls 或文本回复
/// 4. 执行工具，把结果作为 tool 消息追加到对话
/// 5. 重复 3-4，直到模型不再调用工具（stop_reason = Stop）或达到最大步数
///
/// 相比旧版 XML 标签解析，function calling 更可靠：
/// - 不依赖模型严格遵守 XML 格式
/// - 支持并行工具调用
/// - 参数类型安全（JSON Schema 约束）

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::Write;

use crate::config::{NuminaConfig, ModelsConfig};
use crate::core::models::{
    AnthropicProvider, LocalProvider, Message, ModelProvider, OpenAIProvider, Role,
    ToolDefinition, StopReason,
};
use crate::core::skills::SkillManager;
use crate::core::tools::builtin::default_registry;
use crate::core::tools::ToolRegistry;

// ─────────────────────────────────────────────
// 步骤记录
// ─────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStep {
    pub step: usize,
    pub thought: String,
    pub tool_name: Option<String>,
    pub tool_params: Option<serde_json::Value>,
    pub tool_result: Option<String>,
    pub is_final: bool,
    pub final_answer: Option<String>,
}

/// Agent 运行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunResult {
    pub task: String,
    pub steps: Vec<AgentStep>,
    pub final_answer: String,
    pub success: bool,
    pub total_steps: usize,
}

// ─────────────────────────────────────────────
// 工具 schema 构建
// ─────────────────────────────────────────────

/// 将 ToolRegistry 中的工具转换为 OpenAI function calling 格式的 ToolDefinition
fn build_tool_definitions(registry: &ToolRegistry) -> Vec<ToolDefinition> {
    registry
        .list_tools()
        .into_iter()
        .filter_map(|name| {
            let executor = registry.get(&name)?;
            // 从 description 中提取参数信息，构建基础 JSON Schema
            // 实际生产中应该让每个工具提供自己的 schema
            let schema = build_schema_from_description(&name, executor.description());
            Some(ToolDefinition {
                name,
                description: executor.description().to_string(),
                parameters: schema,
            })
        })
        .collect()
}

/// 根据工具名称生成对应的 JSON Schema
fn build_schema_from_description(name: &str, _description: &str) -> serde_json::Value {
    match name {
        "read_file" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to read"
                },
                "max_lines": {
                    "type": "integer",
                    "description": "Maximum number of lines to read (default: 500)"
                }
            },
            "required": ["path"]
        }),
        "write_file" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to write"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["path", "content"]
        }),
        "list_dir" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The directory path to list"
                },
                "recursive": {
                    "type": "boolean",
                    "description": "Whether to list recursively (default: false)"
                }
            },
            "required": ["path"]
        }),
        "shell" => serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory for the command"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default: 30)"
                }
            },
            "required": ["command"]
        }),
        "search_code" => serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "The directory to search in (default: current directory)"
                },
                "file_pattern": {
                    "type": "string",
                    "description": "File glob pattern to filter (e.g., '*.rs')"
                }
            },
            "required": ["pattern"]
        }),
        "find_files" => serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match files (e.g., '**/*.rs')"
                },
                "path": {
                    "type": "string",
                    "description": "The directory to search in (default: current directory)"
                }
            },
            "required": ["pattern"]
        }),
        _ => serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": true
        }),
    }
}

// ─────────────────────────────────────────────
// Provider factory
// ─────────────────────────────────────────────

fn build_provider(
    config: &NuminaConfig,
    model_override: Option<&str>,
) -> Result<Box<dyn ModelProvider>> {
    let models_cfg = ModelsConfig::load().unwrap_or_default();

    let model_name = model_override
        .map(|s| s.to_string())
        .or_else(|| {
            let active = models_cfg.active_model();
            if !active.is_empty() { Some(active.to_string()) } else { None }
        })
        .unwrap_or_else(|| config.model.default_model.clone());

    let entry = models_cfg.models.iter().find(|m| m.name == model_name);
    let provider_name = entry.map(|e| e.provider.as_str()).unwrap_or("openai");

    let api_key = entry
        .and_then(|e| e.api_key.clone())
        .or_else(|| match provider_name {
            "anthropic" => std::env::var("ANTHROPIC_API_KEY").ok(),
            _ => std::env::var("OPENAI_API_KEY").ok(),
        })
        .unwrap_or_default();

    let endpoint = entry.and_then(|e| e.endpoint.clone());

    let provider: Box<dyn ModelProvider> = match provider_name {
        "anthropic" => Box::new(AnthropicProvider::new(api_key, model_name)),
        "local" => Box::new(LocalProvider::new(
            endpoint.unwrap_or_else(|| "http://localhost:11434".to_string()),
            model_name,
        )),
        _ => {
            let mut p = OpenAIProvider::new(api_key, model_name);
            if let Some(ep) = endpoint {
                p = p.with_endpoint(ep);
            }
            Box::new(p)
        }
    };

    Ok(provider)
}

// ─────────────────────────────────────────────
// System prompt
// ─────────────────────────────────────────────

fn build_system_prompt(skill_manager: &SkillManager) -> String {
    let mut parts = vec![
        "You are Numina, an autonomous AI coding agent. You can reason and take actions to complete tasks.".to_string(),
        "Use the available tools to accomplish the user's task. Think step by step.".to_string(),
        "When you have gathered enough information and completed the task, provide a clear final answer.".to_string(),
    ];

    let skills = skill_manager.skills();
    if !skills.is_empty() {
        parts.push("\n## Skills".to_string());
        for skill in skills {
            parts.push(format!("### {}", skill.name));
            parts.push(skill.description.clone());
        }
    }

    parts.join("\n")
}

// ─────────────────────────────────────────────
// 多轮对话消息构建（支持 tool 消息）
// ─────────────────────────────────────────────

/// 将工具调用结果追加为 tool 角色消息
/// OpenAI 格式：role=tool, tool_call_id=..., content=...
/// 由于 Message 只有 content 字段，我们把 tool_call_id 编码进 content
/// 实际上 chat_with_tools 内部会正确处理
fn make_tool_result_message(tool_call_id: &str, tool_name: &str, result: &str) -> Message {
    // 编码为 JSON 格式，让 provider 能正确解析
    Message {
        role: Role::Tool,
        content: serde_json::json!({
            "tool_call_id": tool_call_id,
            "name": tool_name,
            "content": result
        }).to_string(),
    }
}

// ─────────────────────────────────────────────
// ActAgent
// ─────────────────────────────────────────────

pub struct ActAgent {
    config: NuminaConfig,
    registry: ToolRegistry,
    skill_manager: SkillManager,
    max_steps: usize,
    verbose: bool,
}

impl ActAgent {
    pub fn new() -> Result<Self> {
        let config = NuminaConfig::load()?;
        let registry = default_registry();
        let workspace = dirs::home_dir()
            .map(|h| h.join(".numina").join("workspace"))
            .and_then(|p| p.to_str().map(|s| s.to_string()))
            .unwrap_or_else(|| ".".to_string());
        let skill_manager =
            SkillManager::discover(&workspace).unwrap_or_else(|_| SkillManager::empty());

        Ok(Self {
            config,
            registry,
            skill_manager,
            max_steps: 20,
            verbose: true,
        })
    }

    pub fn with_max_steps(mut self, n: usize) -> Self {
        self.max_steps = n;
        self
    }

    pub fn with_verbose(mut self, v: bool) -> Self {
        self.verbose = v;
        self
    }

    /// 运行 ReAct 循环（function calling 版）
    pub async fn run(
        &self,
        task: &str,
        model_override: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<AgentRunResult> {
        let provider = build_provider(&self.config, model_override)?;
        let system_prompt = build_system_prompt(&self.skill_manager);
        let tool_defs = build_tool_definitions(&self.registry);

        let mut messages: Vec<Message> = vec![
            Message {
                role: Role::System,
                content: system_prompt,
            },
            Message {
                role: Role::User,
                content: task.to_string(),
            },
        ];

        let mut steps: Vec<AgentStep> = Vec::new();
        let mut final_answer = String::new();
        let mut success = false;

        if self.verbose {
            println!("🤖 Numina Agent — task: {}", task);
            println!(
                "   model: {}  tools: {}  max_steps: {}\n",
                provider.name(),
                tool_defs.len(),
                self.max_steps
            );
        }

        for step_num in 1..=self.max_steps {
            if self.verbose {
                print!("⟳  Step {}... ", step_num);
                std::io::stdout().flush().ok();
            }

            // 调用模型（带工具定义）
            let response = provider.chat_with_tools(&messages, &tool_defs).await?;

            if self.verbose {
                println!("done");
            }

            let thought = response.content.clone();

            match response.stop_reason {
                StopReason::ToolCalls if !response.tool_calls.is_empty() => {
                    // 模型请求调用工具
                    // 先把 assistant 消息（含 tool_calls）追加到对话
                    // 注意：这里我们把 tool_calls 信息编码进 content，
                    // 让 provider 在下次调用时能正确重建消息历史
                    let tool_calls_json = serde_json::to_string(&response.tool_calls)
                        .unwrap_or_default();
                    messages.push(Message {
                        role: Role::Assistant,
                        content: if thought.is_empty() {
                            format!("__tool_calls__:{}", tool_calls_json)
                        } else {
                            format!("{}\n__tool_calls__:{}", thought, tool_calls_json)
                        },
                    });

                    // 执行所有工具调用（顺序执行，未来可并行）
                    for tool_call in &response.tool_calls {
                        if self.verbose {
                            println!(
                                "   🔧 Tool: {}  params: {}",
                                tool_call.name,
                                serde_json::to_string(&tool_call.arguments).unwrap_or_default()
                            );
                        }

                        // 注入 cwd 到 shell 工具
                        let mut params = tool_call.arguments.clone();
                        if tool_call.name == "shell" {
                            if let Some(cwd_val) = cwd {
                                if params.get("cwd").is_none() {
                                    params["cwd"] = serde_json::Value::String(cwd_val.to_string());
                                }
                            }
                        }

                        // 执行工具
                        let tool_result = self.registry.execute(&tool_call.name, params).await;
                        let result_str = match &tool_result {
                            Ok(r) => {
                                if r.success {
                                    // 提取 content 字段（如果有），否则序列化整个 data
                                    if let Some(content) = r.data.get("content").and_then(|v| v.as_str()) {
                                        content.to_string()
                                    } else {
                                        serde_json::to_string_pretty(&r.data).unwrap_or_default()
                                    }
                                } else {
                                    format!("Error: {}", r.error.as_deref().unwrap_or("unknown"))
                                }
                            }
                            Err(e) => format!("Tool execution failed: {}", e),
                        };

                        if self.verbose {
                            let preview: String = result_str.chars().take(300).collect();
                            let suffix = if result_str.len() > 300 { "…" } else { "" };
                            println!("   📋 Result: {}{}", preview, suffix);
                        }

                        steps.push(AgentStep {
                            step: step_num,
                            thought: thought.clone(),
                            tool_name: Some(tool_call.name.clone()),
                            tool_params: Some(tool_call.arguments.clone()),
                            tool_result: Some(result_str.clone()),
                            is_final: false,
                            final_answer: None,
                        });

                        // 追加工具结果消息
                        messages.push(make_tool_result_message(
                            &tool_call.id,
                            &tool_call.name,
                            &result_str,
                        ));
                    }
                }

                StopReason::Stop | StopReason::Other(_) => {
                    // 模型给出了最终回复
                    let answer = if thought.is_empty() {
                        "Task completed.".to_string()
                    } else {
                        thought.clone()
                    };

                    if self.verbose {
                        println!("\n✅ Final Answer:\n{}", answer);
                    }

                    steps.push(AgentStep {
                        step: step_num,
                        thought: thought.clone(),
                        tool_name: None,
                        tool_params: None,
                        tool_result: None,
                        is_final: true,
                        final_answer: Some(answer.clone()),
                    });

                    final_answer = answer;
                    success = true;
                    break;
                }

                StopReason::Length => {
                    // 达到 max_tokens，把当前内容作为最终答案
                    if self.verbose {
                        println!("\n⚠️  Response truncated (max_tokens reached).");
                    }
                    final_answer = thought;
                    success = false;
                    break;
                }

                _ => {
                    // tool_calls 为空但 stop_reason 是 ToolCalls（异常情况）
                    final_answer = thought;
                    success = true;
                    break;
                }
            }
        }

        if !success && final_answer.is_empty() {
            if self.verbose {
                println!("\n⚠️  Reached max steps ({}) without final answer.", self.max_steps);
            }
            final_answer = "Agent reached maximum steps without completing the task.".to_string();
        }

        Ok(AgentRunResult {
            task: task.to_string(),
            steps: steps.clone(),
            final_answer,
            success,
            total_steps: steps.len(),
        })
    }
}
