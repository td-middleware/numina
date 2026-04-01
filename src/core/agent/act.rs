/// ReAct Agent — Reason + Act 循环引擎
///
/// 工作流程：
/// 1. 用户给出任务目标
/// 2. 构建包含工具描述的 system prompt
/// 3. 调用模型，解析响应中的 <tool_call> 块
/// 4. 执行工具，把结果作为 <tool_result> 追加到对话
/// 5. 重复 3-4，直到模型输出 <final_answer> 或达到最大步数
///
/// 工具调用格式（模型输出）：
/// ```
/// <tool_call>
/// {"tool": "read_file", "params": {"path": "src/main.rs"}}
/// </tool_call>
/// ```
///
/// 最终答案格式：
/// ```
/// <final_answer>
/// 这里是最终结论...
/// </final_answer>
/// ```

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::Write;

use crate::config::{NuminaConfig, ModelsConfig};
use crate::core::models::{Message, Role};
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
// 工具调用解析
// ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ToolCallPayload {
    tool: String,
    #[serde(default)]
    params: serde_json::Value,
}

fn extract_tool_call(text: &str) -> Option<ToolCallPayload> {
    let start = text.find("<tool_call>")?;
    let end = text.find("</tool_call>")?;
    let json_str = text[start + "<tool_call>".len()..end].trim();
    serde_json::from_str(json_str).ok()
}

fn extract_final_answer(text: &str) -> Option<String> {
    let start = text.find("<final_answer>")?;
    let end = text.find("</final_answer>")?;
    Some(text[start + "<final_answer>".len()..end].trim().to_string())
}

fn extract_thought(text: &str) -> String {
    // 提取 <tool_call> 或 <final_answer> 之前的文本作为 thought
    let end = text
        .find("<tool_call>")
        .or_else(|| text.find("<final_answer>"))
        .unwrap_or(text.len());
    text[..end].trim().to_string()
}

// ─────────────────────────────────────────────
// System prompt 构建
// ─────────────────────────────────────────────

fn build_act_system_prompt(registry: &ToolRegistry, skill_manager: &SkillManager) -> String {
    let mut parts = vec![
        "You are Numina, an autonomous AI coding agent. You can reason and take actions to complete tasks.".to_string(),
        "".to_string(),
        "## How to use tools".to_string(),
        "When you need to use a tool, output EXACTLY this format:".to_string(),
        "<tool_call>".to_string(),
        "{\"tool\": \"<tool_name>\", \"params\": {<parameters>}}".to_string(),
        "</tool_call>".to_string(),
        "".to_string(),
        "When you have completed the task and have a final answer, output EXACTLY:".to_string(),
        "<final_answer>".to_string(),
        "Your complete answer here...".to_string(),
        "</final_answer>".to_string(),
        "".to_string(),
        "Think step by step. Before each tool call, briefly explain your reasoning.".to_string(),
        "".to_string(),
        "## Available Tools".to_string(),
    ];

    for tool_name in registry.list_tools() {
        if let Some(executor) = registry.get(&tool_name) {
            parts.push(format!("### {}", tool_name));
            parts.push(executor.description().to_string());
            parts.push("".to_string());
        }
    }

    // 注入 skills
    let skills = skill_manager.skills();
    if !skills.is_empty() {
        parts.push("## Skills".to_string());
        for skill in skills {
            parts.push(format!("### {}", skill.name));
            parts.push(skill.description.clone());
        }
    }

    parts.join("\n")
}

// ─────────────────────────────────────────────
// Provider factory（复用 chat.rs 的逻辑）
// ─────────────────────────────────────────────

use crate::core::models::{AnthropicProvider, LocalProvider, ModelProvider, OpenAIProvider};

fn build_provider(
    config: &NuminaConfig,
    model_override: Option<&str>,
) -> Result<Box<dyn ModelProvider>> {
    // 从独立的 models.json 读取模型列表
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

    /// 运行 ReAct 循环
    pub async fn run(
        &self,
        task: &str,
        model_override: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<AgentRunResult> {
        let provider = build_provider(&self.config, model_override)?;
        let system_prompt = build_act_system_prompt(&self.registry, &self.skill_manager);

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
            println!("   model: {}  tools: {}  max_steps: {}\n",
                provider.name(), self.registry.list_tools().len(), self.max_steps);
        }

        for step_num in 1..=self.max_steps {
            if self.verbose {
                print!("⟳  Step {}... ", step_num);
                std::io::stdout().flush().ok();
            }

            // 调用模型
            let response = provider.chat(&messages).await?;
            let text = response.content.trim().to_string();

            if self.verbose {
                println!("done");
            }

            let thought = extract_thought(&text);

            // 检查是否有最终答案
            if let Some(answer) = extract_final_answer(&text) {
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

            // 检查是否有工具调用
            if let Some(tool_call) = extract_tool_call(&text) {
                if self.verbose {
                    println!("   🔧 Tool: {}  params: {}",
                        tool_call.tool,
                        serde_json::to_string(&tool_call.params).unwrap_or_default()
                    );
                }

                // 注入 cwd 到 shell 工具
                let mut params = tool_call.params.clone();
                if tool_call.tool == "shell" && cwd.is_some() {
                    if params.get("cwd").is_none() {
                        params["cwd"] = serde_json::Value::String(cwd.unwrap().to_string());
                    }
                }

                // 执行工具
                let tool_result = self.registry.execute(&tool_call.tool, params).await;

                let result_str = match &tool_result {
                    Ok(r) => {
                        if r.success {
                            serde_json::to_string_pretty(&r.data).unwrap_or_default()
                        } else {
                            format!("Error: {}", r.error.as_deref().unwrap_or("unknown"))
                        }
                    }
                    Err(e) => format!("Tool execution failed: {}", e),
                };

                if self.verbose {
                    let preview: String = result_str.chars().take(200).collect();
                    let suffix = if result_str.len() > 200 { "…" } else { "" };
                    println!("   📋 Result: {}{}", preview, suffix);
                }

                steps.push(AgentStep {
                    step: step_num,
                    thought: thought.clone(),
                    tool_name: Some(tool_call.tool.clone()),
                    tool_params: Some(tool_call.params),
                    tool_result: Some(result_str.clone()),
                    is_final: false,
                    final_answer: None,
                });

                // 把 assistant 回复和工具结果追加到对话
                messages.push(Message {
                    role: Role::Assistant,
                    content: text.clone(),
                });
                messages.push(Message {
                    role: Role::User,
                    content: format!(
                        "<tool_result>\ntool: {}\nresult: {}\n</tool_result>",
                        tool_call.tool, result_str
                    ),
                });
            } else {
                // 模型没有调用工具也没有给出最终答案，把回复当作最终答案
                if self.verbose {
                    println!("\n💬 Response:\n{}", text);
                }
                steps.push(AgentStep {
                    step: step_num,
                    thought: thought.clone(),
                    tool_name: None,
                    tool_params: None,
                    tool_result: None,
                    is_final: true,
                    final_answer: Some(text.clone()),
                });
                final_answer = text;
                success = true;
                break;
            }
        }

        if !success && self.verbose {
            println!("\n⚠️  Reached max steps ({}) without final answer.", self.max_steps);
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
