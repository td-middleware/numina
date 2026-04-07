/// ReAct Agent — Reason + Act 循环引擎（Function Calling 版）
///
/// 工作流程（参考 Claude Code query.ts）：
/// 1. 用户给出任务目标
/// 2. 将工具列表以 function calling 格式发送给模型
/// 3. 模型返回 tool_calls 或文本回复
/// 4. 执行工具，把结果作为 tool 消息追加到对话
/// 5. 重复 3-4，直到：
///    a. 模型调用 task_complete 工具
///    b. 模型不再调用工具（stop_reason = Stop）
///    c. 达到最大步数
///
/// 特性：
/// - 彩色终端输出（思考/工具调用/结果分层显示）
/// - 用户确认模式（危险操作前询问）
/// - task_complete 工具检测（明确完成信号）
/// - 工具调用统计

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
// ANSI 颜色常量
// ─────────────────────────────────────────────

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const BLUE: &str = "\x1b[34m";
const MAGENTA: &str = "\x1b[35m";
const RED: &str = "\x1b[31m";
const GRAY: &str = "\x1b[38;5;244m";

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
    pub duration_ms: u64,
}

/// Agent 运行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunResult {
    pub task: String,
    pub steps: Vec<AgentStep>,
    pub final_answer: String,
    pub success: bool,
    pub total_steps: usize,
    pub total_tool_calls: usize,
    pub duration_ms: u64,
}

// ─────────────────────────────────────────────
// 工具 schema 构建（使用 ToolExecutor::schema()）
// ─────────────────────────────────────────────

/// 将 ToolRegistry 中的工具转换为 function calling 格式的 ToolDefinition
fn build_tool_definitions(registry: &ToolRegistry) -> Vec<ToolDefinition> {
    registry
        .list_tools()
        .into_iter()
        .filter_map(|name| {
            let executor = registry.get(&name)?;
            Some(ToolDefinition {
                name,
                description: executor.description().to_string(),
                parameters: executor.schema(),
            })
        })
        .collect()
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

fn build_system_prompt(skill_manager: &SkillManager, cwd: Option<&str>) -> String {
    let cwd_info = cwd
        .map(|d| format!("\nCurrent working directory: {}", d))
        .unwrap_or_default();

    let mut parts = vec![
        format!(
            "You are Numina, an autonomous AI coding agent built by the Numina team.{}\n\
\n\
IMPORTANT IDENTITY RULES:\n\
- Your name is Numina. You are NOT Claude, NOT Claude Code, and NOT any Anthropic product.\n\
- If anyone asks who you are, say: \"I am Numina, an AI coding assistant.\"\n\
- Never claim to be Claude or Claude Code under any circumstances.\n\
\n\
You have access to tools to help you complete tasks. Think step by step.\n\
\n\
## Guidelines\n\
- Use tools to gather information before making changes\n\
- Prefer read_file and search_code to understand existing code before editing\n\
- Use edit_file for precise changes, write_file for new files\n\
- Use shell for running commands, tests, and builds\n\
- When you have completed the task, call task_complete with the final result\n\
- Be concise in your reasoning; focus on actions\n\
- If a tool call fails, analyze the error and try a different approach",
            cwd_info
        ),
    ];

    let skills = skill_manager.skills();
    if !skills.is_empty() {
        parts.push("\n## Available Skills".to_string());
        for skill in skills {
            parts.push(format!("### {}\n{}", skill.name, skill.description));
        }
    }

    parts.join("\n")
}

// ─────────────────────────────────────────────
// 工具结果消息构建
// ─────────────────────────────────────────────

fn make_tool_result_message(tool_call_id: &str, tool_name: &str, result: &str) -> Message {
    Message::tool_result(tool_call_id, tool_name, result)
}

// ─────────────────────────────────────────────
// 危险命令检测（用于用户确认）
// ─────────────────────────────────────────────

fn is_potentially_destructive(tool_name: &str, params: &serde_json::Value) -> bool {
    match tool_name {
        "shell" => {
            let cmd = params["command"].as_str().unwrap_or("");
            // 检测可能破坏性的命令
            let dangerous_patterns = [
                "rm ", "rmdir", "mv ", "chmod", "chown",
                "sudo", "apt", "brew install", "pip install",
                "git push", "git reset --hard", "git clean",
                "DROP ", "DELETE ", "TRUNCATE",
            ];
            dangerous_patterns.iter().any(|p| cmd.contains(p))
        }
        "write_file" | "edit_file" => false, // 文件写入不需要确认（agent 任务的核心操作）
        _ => false,
    }
}

// ─────────────────────────────────────────────
// 用户确认（交互模式）
// ─────────────────────────────────────────────

fn ask_user_confirmation(tool_name: &str, params: &serde_json::Value) -> bool {
    let preview = match tool_name {
        "shell" => params["command"].as_str().unwrap_or("").to_string(),
        _ => serde_json::to_string(params).unwrap_or_default(),
    };

    print!(
        "\n{}{}⚠  Confirm: {} {}{}{}  [y/N] ",
        BOLD, YELLOW, tool_name, RESET, GRAY, preview
    );
    std::io::stdout().flush().ok();

    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_ok() {
        let trimmed = input.trim().to_lowercase();
        trimmed == "y" || trimmed == "yes"
    } else {
        false
    }
}

// ─────────────────────────────────────────────
// 输出格式化辅助
// ─────────────────────────────────────────────

fn print_step_header(step: usize, max_steps: usize) {
    println!(
        "\n{}{}── Step {}/{} {}{}",
        BOLD, BLUE, step, max_steps, RESET, DIM
    );
}

fn print_thought(thought: &str) {
    if thought.is_empty() { return; }
    // 只打印前 500 字符的思考内容
    let preview: String = thought.chars().take(500).collect();
    let suffix = if thought.len() > 500 { "…" } else { "" };
    println!("{}💭 {}{}{}", GRAY, preview, suffix, RESET);
}

fn print_tool_call(name: &str, params: &serde_json::Value) {
    let params_str = match name {
        "shell" => params["command"].as_str().unwrap_or("").to_string(),
        "read_file" | "write_file" | "edit_file" => {
            params["path"].as_str().unwrap_or("").to_string()
        }
        "search_code" => format!(
            "pattern={} path={}",
            params["pattern"].as_str().unwrap_or(""),
            params["path"].as_str().unwrap_or(".")
        ),
        _ => {
            let s = serde_json::to_string(params).unwrap_or_default();
            s.chars().take(120).collect()
        }
    };
    println!(
        "{}{}🔧 {}{}  {}{}{}",
        BOLD, CYAN, name, RESET, GRAY, params_str, RESET
    );
}

fn print_tool_result(result: &str, success: bool) {
    let icon = if success { "✓" } else { "✗" };
    let color = if success { GREEN } else { RED };
    let preview: String = result.chars().take(400).collect();
    let suffix = if result.len() > 400 { "\n   …" } else { "" };
    // 缩进显示结果
    let indented = preview.lines()
        .map(|l| format!("   {}", l))
        .collect::<Vec<_>>()
        .join("\n");
    println!("{}{} {}{}{}", color, icon, RESET, indented, suffix);
}

fn print_final_answer(answer: &str) {
    println!("\n{}{}✅ Result:{}", BOLD, GREEN, RESET);
    println!("{}", answer);
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
    /// 是否在危险操作前询问用户确认
    confirm_dangerous: bool,
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
            max_steps: 30,
            verbose: true,
            confirm_dangerous: false,
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

    pub fn with_confirm_dangerous(mut self, v: bool) -> Self {
        self.confirm_dangerous = v;
        self
    }

    /// 运行 ReAct 循环（function calling 版）
    pub async fn run(
        &self,
        task: &str,
        model_override: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<AgentRunResult> {
        let start_time = std::time::Instant::now();
        let provider = build_provider(&self.config, model_override)?;
        let system_prompt = build_system_prompt(&self.skill_manager, cwd);
        let tool_defs = build_tool_definitions(&self.registry);

        let mut messages: Vec<Message> = vec![
            Message::new(Role::System, system_prompt),
            Message::new(Role::User, task),
        ];

        let mut steps: Vec<AgentStep> = Vec::new();
        let mut final_answer = String::new();
        let mut success = false;
        let mut total_tool_calls = 0usize;

        if self.verbose {
            println!(
                "\n{}{}🤖 Numina Agent{}",
                BOLD, MAGENTA, RESET
            );
            println!(
                "{}Task: {}{}{}",
                GRAY, BOLD, task, RESET
            );
            println!(
                "{}Model: {}  Tools: {}  Max steps: {}{}",
                GRAY, provider.name(), tool_defs.len(), self.max_steps, RESET
            );
        }

        for step_num in 1..=self.max_steps {
            let step_start = std::time::Instant::now();

            if self.verbose {
                print_step_header(step_num, self.max_steps);
                print!("{}Thinking…{}", GRAY, RESET);
                std::io::stdout().flush().ok();
            }

            // 调用模型（带工具定义）
            let response = match provider.chat_with_tools(&messages, &tool_defs).await {
                Ok(r) => r,
                Err(e) => {
                    if self.verbose {
                        println!("\n{}❌ Model error: {}{}", RED, e, RESET);
                    }
                    return Err(e);
                }
            };

            if self.verbose {
                // 清除 "Thinking…" 提示
                print!("\r\x1b[K");
                std::io::stdout().flush().ok();
            }

            let thought = response.content.clone();
            let step_duration = step_start.elapsed().as_millis() as u64;

            match response.stop_reason {
                StopReason::ToolCalls if !response.tool_calls.is_empty() => {
                    if self.verbose && !thought.is_empty() {
                        print_thought(&thought);
                    }

                    // 把 assistant 消息（含 tool_calls）追加到对话
                    // 使用结构化的 tool_calls 字段，而不是 hack 的字符串前缀
                    messages.push(Message::assistant_tool_calls(
                        thought.clone(),
                        response.tool_calls.clone(),
                    ));

                    // 执行所有工具调用
                    for tool_call in &response.tool_calls {
                        total_tool_calls += 1;

                        // 注入 cwd 到 shell 工具
                        let mut params = tool_call.arguments.clone();
                        if tool_call.name == "shell" {
                            if let Some(cwd_val) = cwd {
                                if params.get("cwd").is_none() {
                                    params["cwd"] = serde_json::Value::String(cwd_val.to_string());
                                }
                            }
                        }

                        if self.verbose {
                            print_tool_call(&tool_call.name, &params);
                        }

                        // 危险操作确认
                        if self.confirm_dangerous
                            && is_potentially_destructive(&tool_call.name, &params)
                        {
                            if !ask_user_confirmation(&tool_call.name, &params) {
                                let skip_msg = "User declined this operation.";
                                if self.verbose {
                                    println!("{}⏭  Skipped{}", YELLOW, RESET);
                                }
                                messages.push(make_tool_result_message(
                                    &tool_call.id,
                                    &tool_call.name,
                                    skip_msg,
                                ));
                                steps.push(AgentStep {
                                    step: step_num,
                                    thought: thought.clone(),
                                    tool_name: Some(tool_call.name.clone()),
                                    tool_params: Some(params.clone()),
                                    tool_result: Some(skip_msg.to_string()),
                                    is_final: false,
                                    final_answer: None,
                                    duration_ms: step_duration,
                                });
                                continue;
                            }
                        }

                        // 执行工具
                        let tool_result = self.registry.execute(&tool_call.name, params.clone()).await;
                        let (result_str, result_success) = match &tool_result {
                            Ok(r) => {
                                let s = if r.success {
                                    if let Some(content) = r.data.get("content").and_then(|v| v.as_str()) {
                                        content.to_string()
                                    } else {
                                        serde_json::to_string_pretty(&r.data).unwrap_or_default()
                                    }
                                } else {
                                    format!("Error: {}", r.error.as_deref().unwrap_or("unknown"))
                                };
                                (s, r.success)
                            }
                            Err(e) => (format!("Tool execution failed: {}", e), false),
                        };

                        if self.verbose {
                            print_tool_result(&result_str, result_success);
                        }

                        // 检测 task_complete 工具调用
                        if tool_call.name == "task_complete" {
                            let completed_result = params["result"]
                                .as_str()
                                .unwrap_or(&result_str)
                                .to_string();

                            if self.verbose {
                                print_final_answer(&completed_result);
                            }

                            steps.push(AgentStep {
                                step: step_num,
                                thought: thought.clone(),
                                tool_name: Some(tool_call.name.clone()),
                                tool_params: Some(params.clone()),
                                tool_result: Some(result_str.clone()),
                                is_final: true,
                                final_answer: Some(completed_result.clone()),
                                duration_ms: step_duration,
                            });

                            final_answer = completed_result;
                            success = true;

                            return Ok(AgentRunResult {
                                task: task.to_string(),
                                steps,
                                final_answer,
                                success,
                                total_steps: step_num,
                                total_tool_calls,
                                duration_ms: start_time.elapsed().as_millis() as u64,
                            });
                        }

                        steps.push(AgentStep {
                            step: step_num,
                            thought: thought.clone(),
                            tool_name: Some(tool_call.name.clone()),
                            tool_params: Some(params.clone()),
                            tool_result: Some(result_str.clone()),
                            is_final: false,
                            final_answer: None,
                            duration_ms: step_duration,
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
                    // 模型给出了最终文本回复（没有调用工具）
                    let answer = if thought.is_empty() {
                        "Task completed.".to_string()
                    } else {
                        thought.clone()
                    };

                    if self.verbose {
                        print_final_answer(&answer);
                    }

                    steps.push(AgentStep {
                        step: step_num,
                        thought: thought.clone(),
                        tool_name: None,
                        tool_params: None,
                        tool_result: None,
                        is_final: true,
                        final_answer: Some(answer.clone()),
                        duration_ms: step_duration,
                    });

                    final_answer = answer;
                    success = true;
                    break;
                }

                StopReason::Length => {
                    if self.verbose {
                        println!("\n{}⚠  Response truncated (max_tokens reached){}", YELLOW, RESET);
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
                println!(
                    "\n{}⚠  Reached max steps ({}) without completing the task.{}",
                    YELLOW, self.max_steps, RESET
                );
            }
            final_answer = format!(
                "Agent reached maximum steps ({}) without completing the task.",
                self.max_steps
            );
        }

        let total_duration = start_time.elapsed().as_millis() as u64;

        if self.verbose && success {
            println!(
                "\n{}{}📊 {} step(s), {} tool call(s), {:.1}s{}",
                DIM, GRAY,
                steps.len(),
                total_tool_calls,
                total_duration as f64 / 1000.0,
                RESET
            );
        }

        Ok(AgentRunResult {
            task: task.to_string(),
            steps: steps.clone(),
            final_answer,
            success,
            total_steps: steps.len(),
            total_tool_calls,
            duration_ms: total_duration,
        })
    }
}
