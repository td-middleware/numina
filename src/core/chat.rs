/// ChatEngine — 核心对话引擎
///
/// 负责：
/// 1. 从 claude.md 加载 skills，构建 system prompt
/// 2. 管理 session memory（持久化到 ~/.numina/workspace/sessions/<id>.json）
/// 3. 根据配置选择 ModelProvider 并发起调用
/// 4. 返回流式 / 非流式响应

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

use crate::config::{NuminaConfig, ModelsConfig};
use crate::core::skills::SkillManager;
use crate::core::models::{
    AnthropicProvider, ChatResponse, LocalProvider, Message, ModelProvider, OpenAIProvider, Role,
    ToolDefinition,
};
use crate::core::tools::builtin::default_registry;
use crate::core::models::provider::StopReason;

// ─────────────────────────────────────────────
// Session / Memory types
// ─────────────────────────────────────────────

/// 单轮对话记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatTurn {
    pub role: String,
    pub content: String,
    pub timestamp: String,
}

/// 一个完整的会话（对应一个 JSON 文件）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSession {
    pub id: String,
    pub created_at: String,
    pub model: String,
    pub turns: Vec<ChatTurn>,
}

impl ChatSession {
    pub fn new(model: &str) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            id: Uuid::new_v4().to_string(),
            created_at: now.clone(),
            model: model.to_string(),
            turns: Vec::new(),
        }
    }

    pub fn push(&mut self, role: &str, content: &str) {
        self.turns.push(ChatTurn {
            role: role.to_string(),
            content: content.to_string(),
            timestamp: Utc::now().to_rfc3339(),
        });
    }

    /// 把 session turns 转换为 provider 需要的 Message 列表（不含 system）
    pub fn to_messages(&self) -> Vec<Message> {
        self.turns
            .iter()
            .map(|t| Message::new(
                match t.role.as_str() {
                    "assistant" => Role::Assistant,
                    _ => Role::User,
                },
                t.content.clone(),
            ))
            .collect()
    }
}

// ─────────────────────────────────────────────
// Session persistence
// ─────────────────────────────────────────────

fn sessions_dir() -> Result<PathBuf> {
    let dir = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?
        .join(".numina")
        .join("workspace")
        .join("sessions");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn session_path(session_id: &str) -> Result<PathBuf> {
    Ok(sessions_dir()?.join(format!("{}.json", session_id)))
}

fn load_session(session_id: &str) -> Result<ChatSession> {
    let path = session_path(session_id)?;
    if path.exists() {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read session {}", session_id))?;
        let session: ChatSession = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse session {}", session_id))?;
        Ok(session)
    } else {
        Err(anyhow::anyhow!("Session '{}' not found", session_id))
    }
}

fn save_session(session: &ChatSession) -> Result<()> {
    let path = session_path(&session.id)?;
    let content = serde_json::to_string_pretty(session)?;
    std::fs::write(path, content)?;
    Ok(())
}

// ─────────────────────────────────────────────
// Provider factory
// ─────────────────────────────────────────────

/// 根据配置构建 ModelProvider
fn build_provider(
    config: &NuminaConfig,
    model_override: Option<&str>,
) -> Result<(Box<dyn ModelProvider>, String)> {
    // 从独立的 models.json 读取模型列表
    let models_cfg = ModelsConfig::load().unwrap_or_default();

    // 确定要使用的模型名
    let model_name = model_override
        .map(|s| s.to_string())
        .or_else(|| {
            let active = models_cfg.active_model();
            if !active.is_empty() { Some(active.to_string()) } else { None }
        })
        .unwrap_or_else(|| config.model.default_model.clone());

    // 在 models.json 里查找该模型的配置
    let entry = models_cfg.models.iter().find(|m| m.name == model_name);

    let provider_name = entry
        .map(|e| e.provider.as_str())
        .unwrap_or("openai");

    let api_key = entry
        .and_then(|e| e.api_key.clone())
        .or_else(|| match provider_name {
            "anthropic" => std::env::var("ANTHROPIC_API_KEY").ok(),
            _ => std::env::var("OPENAI_API_KEY").ok(),
        })
        .unwrap_or_default();

    let endpoint = entry.and_then(|e| e.endpoint.clone());

    let provider: Box<dyn ModelProvider> = match provider_name {
        "anthropic" => Box::new(AnthropicProvider::new(api_key, model_name.clone())),
        "local" => Box::new(LocalProvider::new(
            endpoint.unwrap_or_else(|| "http://localhost:11434".to_string()),
            model_name.clone(),
        )),
        _ => {
            let mut p = OpenAIProvider::new(api_key, model_name.clone());
            if let Some(ep) = endpoint {
                p = p.with_endpoint(ep);
            }
            Box::new(p)
        }
    };

    Ok((provider, model_name))
}

// ─────────────────────────────────────────────
// ChatEngine
// ─────────────────────────────────────────────

pub struct ChatEngine {
    config: NuminaConfig,
    skill_manager: SkillManager,
}

impl ChatEngine {
    /// 创建 ChatEngine，自动加载配置和 skills
    pub fn new() -> Result<Self> {
        let config = NuminaConfig::load()?;
        let workspace = dirs::home_dir()
            .map(|h| h.join(".numina").join("workspace"))
            .and_then(|p| p.to_str().map(|s| s.to_string()))
            .unwrap_or_else(|| ".".to_string());
        let skill_manager = SkillManager::discover(&workspace).unwrap_or_else(|_| SkillManager::empty());
        Ok(Self { config, skill_manager })
    }

    /// 构建 system prompt（包含 skills 描述）
    fn build_system_prompt(&self) -> String {
        let mut parts = vec![
            "You are Numina, an AI coding assistant. You help developers write, review, debug, and understand code.".to_string(),
            "Be concise, accurate, and helpful. When writing code, prefer idiomatic patterns.".to_string(),
        ];

        let skills = self.skill_manager.skills();
        if !skills.is_empty() {
            parts.push("\n## Available Skills\n".to_string());
            for skill in skills {
                parts.push(format!("### {}\n{}", skill.name, skill.description));
                if !skill.examples.is_empty() {
                    parts.push(format!("Examples: {}", skill.examples.join(", ")));
                }
            }
        }

        parts.join("\n")
    }

    /// 构建发送给模型的消息列表，自动压缩超长上下文
    ///
    /// 压缩策略：
    /// - 当历史 token 数超过 context_window * 90% 时触发压缩
    /// - 保留最近 KEEP_RECENT_TURNS 条消息
    /// - 把更早的消息压缩成一段文字摘要，插入到历史开头
    fn build_messages_with_compression(
        &self,
        session: &mut ChatSession,
        context_window: usize,
    ) {
        const KEEP_RECENT_TURNS: usize = 6; // 保留最近 6 条消息（约 3 轮对话）
        const COMPRESS_THRESHOLD: f64 = 0.90; // 超过 90% 触发压缩

        let threshold_tokens = (context_window as f64 * COMPRESS_THRESHOLD) as usize;

        // 估算当前历史 token 数
        let history_chars: usize = session.turns.iter().map(|t| t.content.len()).sum();
        let history_tokens = history_chars / 4;

        if history_tokens <= threshold_tokens || session.turns.len() <= KEEP_RECENT_TURNS {
            return; // 不需要压缩
        }

        // 分割：旧消息 + 最近消息
        let split_at = session.turns.len().saturating_sub(KEEP_RECENT_TURNS);
        let old_turns = &session.turns[..split_at];
        let recent_turns = session.turns[split_at..].to_vec();

        // 生成摘要文本（简单拼接旧消息的前 200 字符，不调用 API）
        let mut summary_parts = vec!["[Earlier conversation summary]:".to_string()];
        for turn in old_turns {
            let preview: String = turn.content.chars().take(200).collect();
            let ellipsis = if turn.content.len() > 200 { "..." } else { "" };
            summary_parts.push(format!("{}: {}{}", turn.role, preview, ellipsis));
        }
        let summary = summary_parts.join("\n");

        // 重建 turns：摘要作为 user 消息 + assistant 确认 + 最近消息
        let mut new_turns = vec![
            ChatTurn {
                role: "user".to_string(),
                content: summary,
                timestamp: chrono::Utc::now().to_rfc3339(),
            },
            ChatTurn {
                role: "assistant".to_string(),
                content: "I understand the conversation history. Let me continue from where we left off.".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
            },
        ];
        new_turns.extend(recent_turns);
        session.turns = new_turns;
    }

    /// 单次对话（非交互式）
    /// 返回 (response_text, session_id, used_tokens, context_window)
    pub async fn chat_once(
        &self,
        user_message: &str,
        model_override: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<(String, String, usize, usize)> {
        let (provider, model_name) = build_provider(&self.config, model_override)?;

        // 加载或新建 session
        let mut session = match session_id {
            Some(id) => load_session(id).unwrap_or_else(|_| ChatSession::new(&model_name)),
            None => ChatSession::new(&model_name),
        };

        // 追加用户消息
        session.push("user", user_message);

        // 获取 context window 大小
        let context_window = self.get_context_window(model_override);

        // 自动压缩超长上下文（超过 90% 时触发）
        self.build_messages_with_compression(&mut session, context_window);

        // 构建完整消息列表（system + history）
        let system_prompt = self.build_system_prompt();
        let mut messages = vec![Message::new(Role::System, system_prompt)];
        messages.extend(session.to_messages());

        // 估算发送的 token 数（字符数 / 4 粗略估算）
        let sent_chars: usize = messages.iter().map(|m| m.content.len()).sum();
        let sent_tokens = sent_chars / 4;

        // 调用模型
        let response: ChatResponse = provider.chat(&messages).await?;
        let reply = response.content.clone();

        // 计算实际使用的 token 数（优先用 API 返回值，否则用估算）
        let used_tokens = if response.usage.total_tokens > 0 {
            response.usage.total_tokens
        } else {
            sent_tokens + reply.len() / 4
        };

        // 追加 assistant 回复并持久化（压缩后的 session）
        session.push("assistant", &reply);
        save_session(&session)?;

        Ok((reply, session.id.clone(), used_tokens, context_window))
    }

    /// 流式对话（返回 channel receiver）
    /// 返回 (receiver, session_id, estimated_sent_tokens, context_window)
    pub async fn chat_stream(
        &self,
        user_message: &str,
        model_override: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<(tokio::sync::mpsc::Receiver<String>, String, usize, usize)> {
        let (provider, model_name) = build_provider(&self.config, model_override)?;

        let mut session = match session_id {
            Some(id) => load_session(id).unwrap_or_else(|_| ChatSession::new(&model_name)),
            None => ChatSession::new(&model_name),
        };

        session.push("user", user_message);

        // 获取 context window 大小
        let context_window = self.get_context_window(model_override);

        // 自动压缩超长上下文（超过 90% 时触发）
        self.build_messages_with_compression(&mut session, context_window);

        let system_prompt = self.build_system_prompt();
        let mut messages = vec![Message::new(Role::System, system_prompt)];
        messages.extend(session.to_messages());

        // 估算发送的 token 数（字符数 / 4 粗略估算）
        let sent_chars: usize = messages.iter().map(|m| m.content.len()).sum();
        let sent_tokens = sent_chars / 4;

        let rx = provider.chat_stream(&messages).await?;
        let sid = session.id.clone();

        // 注意：流式模式下 session 的 assistant turn 需要调用方在收完后追加
        // 这里先保存压缩后的 session（不含 assistant 回复），调用方负责调用 append_assistant_turn
        save_session(&session)?;

        Ok((rx, sid, sent_tokens, context_window))
    }

    /// ReAct 对话：带工具调用的对话循环
    /// 通过 channel 实时推送输出事件：
    ///   "\x00T{name}|{params}"  → 工具调用开始
    ///   "\x00R{result}"         → 工具结果
    ///   "\x00C{text}"           → 普通文本输出
    ///   "\x00D"                 → 完成
    /// 返回 (receiver, session_id, sent_tokens, context_window)
    pub async fn chat_react(
        &self,
        user_message: &str,
        model_override: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<(tokio::sync::mpsc::Receiver<String>, String, usize, usize)> {
        let (provider, model_name) = build_provider(&self.config, model_override)?;
        let registry = default_registry();

        // ReAct 模式：不加载旧 session 历史（旧历史是普通对话，没有工具上下文，会污染模型行为）
        // 每次 ReAct 调用都是独立的工具对话，只保留 session ID 用于持久化
        let session = match session_id {
            Some(id) => load_session(id).unwrap_or_else(|_| ChatSession::new(&model_name)),
            None => ChatSession::new(&model_name),
        };

        let context_window = self.get_context_window(model_override);

        // 构建工具定义
        let tool_defs: Vec<ToolDefinition> = registry
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
            .collect();

        // 构建带工具能力的 system prompt
        let system_prompt = {
            let mut parts = vec![
                "You are Numina, an AI coding assistant with tool use capabilities.\n\
You are NOT Claude, NOT Claude Code. You are Numina.\n\
\n\
You have access to tools: read_file, write_file, edit_file, list_dir, shell, search_code, find_files, http_get, task_complete.\n\
\n\
## Tool Usage Rules\n\
\n\
When the user asks you to do something that requires executing commands, reading files, or interacting with the system:\n\
- USE the tools directly. Do NOT say you cannot execute commands.\n\
- NEVER calculate or estimate results yourself — always use tools to get real data.\n\
- For counting lines of code: use shell with `find . -name \"*.rs\" -not -path \"*/target/*\" | xargs wc -l 2>/dev/null | tail -1` or similar.\n\
- For listing files: use shell with `find` or `ls` commands, NOT list_dir with recursive=true (too slow).\n\
- For analyzing a directory: use shell to run commands like `find . -name \"*.rs\" | wc -l` to count files.\n\
- When multiple independent tasks can be done in parallel, call multiple tools in a SINGLE response.\n\
- After getting tool results, summarize them clearly and accurately for the user.\n\
\n\
## Parallel Execution\n\
\n\
If the user asks to analyze multiple files or perform multiple independent operations, call ALL the tools at once in a single response (the system will execute them concurrently). For example:\n\
- To count lines in multiple files: call shell once with a command that counts all files at once.\n\
- To read multiple files: call read_file for each file in the same response.\n\
\n\
Be concise and action-oriented. Always use tools to get real data.".to_string(),
            ];
            let skills = self.skill_manager.skills();
            if !skills.is_empty() {
                parts.push("\n## Available Skills".to_string());
                for skill in skills {
                    parts.push(format!("### {}\n{}", skill.name, skill.description));
                }
            }
            parts.join("\n")
        };

        // 构建消息列表：system + 当前用户消息（不带旧历史，避免污染工具调用上下文）
        let mut messages = vec![
            Message::new(Role::System, system_prompt),
            Message::new(Role::User, user_message),
        ];

        let sid = session.id.clone();
        let sid2 = sid.clone();

        // 估算发送的 token 数（字符数 / 4 粗略估算）
        let sent_chars: usize = messages.iter().map(|m| m.content.len()).sum();
        let sent_tokens = sent_chars / 4;

        let (tx, rx) = tokio::sync::mpsc::channel::<String>(256);

        // 在后台任务中运行 ReAct 循环
        tokio::spawn(async move {
            const MAX_STEPS: usize = 10;
            let mut full_reply = String::new();

            for _step in 0..MAX_STEPS {
                // 调用模型（带工具）
                let response = match provider.chat_with_tools(&messages, &tool_defs).await {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = tx.send(format!("\x00C❌ Error: {}", e)).await;
                        break;
                    }
                };

                match response.stop_reason {
                    StopReason::ToolCalls if !response.tool_calls.is_empty() => {
                        // 有工具调用：先输出思考内容（如果有）
                        if !response.content.is_empty() {
                            let _ = tx.send(format!("\x00C{}\n", response.content)).await;
                            full_reply.push_str(&response.content);
                            full_reply.push('\n');
                        }

                        // 把 assistant 工具调用消息加入对话历史
                        messages.push(Message::assistant_tool_calls(
                            response.content.clone(),
                            response.tool_calls.clone(),
                        ));

                        // ── 并发执行所有工具调用 ──
                        // 先通知 UI 所有工具调用开始（顺序通知，避免乱序）
                        for tool_call in &response.tool_calls {
                            let params_preview = match tool_call.name.as_str() {
                                "shell" => tool_call.arguments["command"].as_str().unwrap_or("").to_string(),
                                "read_file" | "write_file" | "edit_file" => {
                                    tool_call.arguments["path"].as_str().unwrap_or("").to_string()
                                }
                                _ => {
                                    let s = serde_json::to_string(&tool_call.arguments).unwrap_or_default();
                                    s.chars().take(80).collect()
                                }
                            };
                            let _ = tx.send(format!("\x00T{}|{}", tool_call.name, params_preview)).await;
                        }

                        // 并发执行所有工具（tokio::join_all）
                        let tool_futures: Vec<_> = response.tool_calls.iter().map(|tc| {
                            let reg = &registry;
                            let name = tc.name.clone();
                            let args = tc.arguments.clone();
                            async move {
                                match reg.execute(&name, args).await {
                                    Ok(r) => {
                                        if r.success {
                                            r.data.get("content")
                                                .and_then(|v| v.as_str())
                                                .map(|s| s.to_string())
                                                .unwrap_or_else(|| serde_json::to_string_pretty(&r.data).unwrap_or_default())
                                        } else {
                                            format!("Error: {}", r.error.as_deref().unwrap_or("unknown"))
                                        }
                                    }
                                    Err(e) => format!("Tool execution failed: {}", e),
                                }
                            }
                        }).collect();

                        let results = futures::future::join_all(tool_futures).await;

                        // 按顺序通知 UI 结果，并加入对话历史
                        // 工具结果截断：防止超长内容（如大目录列表）撑爆模型上下文
                        const MAX_TOOL_RESULT_CHARS: usize = 8000;
                        for (tool_call, result_str) in response.tool_calls.iter().zip(results.iter()) {
                            let _ = tx.send(format!("\x00R{}", result_str)).await;
                            // 截断后再加入 messages，避免超出模型 token 限制
                            let truncated_result = if result_str.len() > MAX_TOOL_RESULT_CHARS {
                                format!(
                                    "{}\n\n[... truncated, {} chars total. Use more specific parameters to get focused results.]",
                                    &result_str[..MAX_TOOL_RESULT_CHARS],
                                    result_str.len()
                                )
                            } else {
                                result_str.clone()
                            };
                            messages.push(Message::tool_result(
                                &tool_call.id,
                                &tool_call.name,
                                &truncated_result,
                            ));
                        }
                        // 通知 UI：工具执行完毕，等待模型下一轮响应（重新显示 thinking 动画）
                        let _ = tx.send("\x00W".to_string()).await;
                        // 继续循环，让模型处理工具结果
                    }

                    _ => {
                        // 模型给出最终文本回复（无工具调用）
                        let text = if response.content.is_empty() {
                            "Done.".to_string()
                        } else {
                            response.content.clone()
                        };
                        let _ = tx.send(format!("\x00C{}", text)).await;
                        full_reply.push_str(&text);
                        break;
                    }
                }
            }

            // 完成信号
            let _ = tx.send("\x00D".to_string()).await;

            // 保存 session
            if let Ok(mut sess) = load_session(&sid2) {
                sess.push("assistant", &full_reply);
                let _ = save_session(&sess);
            }
        });

        Ok((rx, sid, sent_tokens, context_window))
    }

    /// 在流式输出完成后，将 assistant 回复追加到 session
    pub fn append_assistant_turn(session_id: &str, content: &str) -> Result<()> {
        let mut session = load_session(session_id)?;
        session.push("assistant", content);
        save_session(&session)
    }

    /// 列出所有 session（按修改时间倒序）
    pub fn list_sessions() -> Result<Vec<String>> {
        let dir = sessions_dir()?;
        let mut entries: Vec<(std::time::SystemTime, String)> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .map(|x| x == "json")
                    .unwrap_or(false)
            })
            .filter_map(|e| {
                let modified = e.metadata().ok()?.modified().ok()?;
                let name = e.path().file_stem()?.to_str()?.to_string();
                Some((modified, name))
            })
            .collect();
        entries.sort_by(|a, b| b.0.cmp(&a.0));
        Ok(entries.into_iter().map(|(_, name)| name).collect())
    }

    /// 获取 session 详情
    pub fn get_session(session_id: &str) -> Result<ChatSession> {
        load_session(session_id)
    }

    /// 返回当前加载的 skills 数量
    pub fn skill_count(&self) -> usize {
        self.skill_manager.skills().len()
    }

    /// 返回当前使用的模型名
    pub fn default_model(&self) -> String {
        ModelsConfig::load()
            .ok()
            .map(|mc| mc.active_model().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.config.model.default_model.clone())
    }

    /// 获取模型的 context window 大小（tokens）
    /// 优先从 models.json 的 max_tokens 读取，否则按模型名估算
    pub fn get_context_window(&self, model_override: Option<&str>) -> usize {
        let models_cfg = ModelsConfig::load().unwrap_or_default();
        let model_name = model_override
            .map(|s| s.to_string())
            .unwrap_or_else(|| self.default_model());

        // 先从配置文件里找 max_tokens
        if let Some(entry) = models_cfg.models.iter().find(|m| m.name == model_name) {
            if let Some(mt) = entry.max_tokens {
                return mt;
            }
        }

        // 按模型名估算
        let name_lower = model_name.to_lowercase();
        if name_lower.contains("claude-3-5") || name_lower.contains("claude-3.5") {
            200_000
        } else if name_lower.contains("claude-3") {
            200_000
        } else if name_lower.contains("gpt-4o") {
            128_000
        } else if name_lower.contains("gpt-4-turbo") {
            128_000
        } else if name_lower.contains("gpt-4") {
            8_192
        } else if name_lower.contains("gpt-3.5") {
            16_385
        } else if name_lower.contains("o1") || name_lower.contains("o3") {
            200_000
        } else {
            128_000
        }
    }
}
