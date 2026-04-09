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
// Agent Loop 辅助函数
// ─────────────────────────────────────────────

/// 生成工具调用的参数预览字符串（用于 UI 显示）
fn tool_call_preview(tool_call: &crate::core::models::provider::ToolCall) -> String {
    match tool_call.name.as_str() {
        "shell" => tool_call.arguments["command"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        "read_file" | "write_file" | "edit_file" => tool_call.arguments["path"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        "list_dir" => tool_call.arguments["path"]
            .as_str()
            .unwrap_or(".")
            .to_string(),
        "search_code" => format!(
            "{} in {}",
            tool_call.arguments["pattern"].as_str().unwrap_or("?"),
            tool_call.arguments["path"].as_str().unwrap_or(".")
        ),
        "find_files" => format!(
            "{} in {}",
            tool_call.arguments["pattern"].as_str().unwrap_or("?"),
            tool_call.arguments["path"].as_str().unwrap_or(".")
        ),
        "http_get" => tool_call.arguments["url"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        _ => {
            let s = serde_json::to_string(&tool_call.arguments).unwrap_or_default();
            s.chars().take(80).collect()
        }
    }
}

/// 截断工具结果（防止超出模型 token 限制）
fn truncate_tool_result(result: &str, max_chars: usize) -> String {
    if result.len() <= max_chars {
        result.to_string()
    } else {
        // 在字符边界截断（避免 UTF-8 截断问题）
        let truncated: String = result.chars().take(max_chars).collect();
        format!(
            "{}\n\n[... truncated, {} chars total. Use more specific parameters to get focused results.]",
            truncated,
            result.len()
        )
    }
}

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

    /// ReAct Agent Loop — 参考 ConversationRuntime::run_turn 架构
    ///
    /// 设计原则（来自 claw-code/rust/crates/runtime）：
    /// 1. Session 持久化：每轮 assistant 消息和工具结果立即写入 session
    /// 2. 带历史的多轮对话：加载 session 历史，支持跨轮上下文
    /// 3. max_iterations 保护：防止无限循环
    /// 4. 自动压缩：超过 context window 90% 时压缩历史
    /// 5. 并发工具执行：同一轮的多个工具并发执行
    ///
    /// 事件协议（通过 event_tx 推送给 CLI）：
    ///   "\x00S{summary}"        → 思维链摘要（"Reading 1 file, listing 1 directory…"）
    ///   "\x00K{id}|{tool}|{cmd}|{desc}" → 需要权限确认的工具调用
    ///   "\x00T{name}|{params}"  → 工具调用开始（已允许，UI 显示工具名）
    ///   "\x00R{result}"         → 工具结果（UI 显示结果预览）
    ///   "\x00C{text}"           → 普通文本输出（流式显示）
    ///   "\x00W"                 → 等待模型下一轮（UI 重新显示 thinking 动画）
    ///   "\x00D"                 → 完成
    ///
    /// 权限回复协议（通过 perm_rx 从 CLI 接收）：
    ///   "{id}|allow"            → 允许执行
    ///   "{id}|allow_session"    → 允许并记住（本 session 内不再询问）
    ///   "{id}|deny"             → 拒绝执行
    ///
    /// 返回 (event_rx, perm_tx, session_id, sent_tokens, context_window)
    pub async fn chat_react(
        &self,
        user_message: &str,
        model_override: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<(tokio::sync::mpsc::Receiver<String>, tokio::sync::mpsc::Sender<String>, String, usize, usize)>
    {
        let (provider, model_name) = build_provider(&self.config, model_override)?;
        let registry = default_registry();
        let context_window = self.get_context_window(model_override);

        // ── 加载或新建 session（参考实现：session 贯穿整个 run_turn） ──
        let mut session = match session_id {
            Some(id) => load_session(id).unwrap_or_else(|_| ChatSession::new(&model_name)),
            None => ChatSession::new(&model_name),
        };

        // ── 构建工具定义 ──
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

        // ── 构建 system prompt ──
        let system_prompt = self.build_react_system_prompt();

        // ── 构建初始消息列表（system + session 历史 + 当前用户消息） ──
        // 参考实现：session.messages 包含完整历史，每轮都追加
        let mut messages = vec![Message::new(Role::System, system_prompt)];

        // 加载 session 历史（仅保留最近 N 轮，避免超长）
        let history_turns = session.to_messages();

        // 过滤掉历史中包含"拒绝执行命令"的 assistant 回复（防止污染新对话）
        // 这类回复是模型在没有工具定义时产生的错误行为，不应该被带入新对话
        let refusal_patterns = [
            "没办法执行命令",
            "无法执行命令",
            "I cannot execute",
            "I can't execute",
            "this is a web interface",
            "这是网页对话界面",
            "web chat interface",
            "I don't have the ability to run",
            "我无法运行",
            "cannot run commands",
        ];
        let history_turns: Vec<_> = history_turns.into_iter().filter(|m| {
            // 只过滤 assistant 消息中的拒绝性内容
            if m.role == Role::Assistant {
                let content_lower = m.content.to_lowercase();
                !refusal_patterns.iter().any(|p| content_lower.contains(&p.to_lowercase()))
            } else {
                true
            }
        }).collect();

        // 自动压缩：如果历史 token 数超过 context_window 的 80%，只保留最近 6 条
        let history_chars: usize = history_turns.iter().map(|m| m.content.len()).sum();
        let history_tokens = history_chars / 4;
        let keep_turns = if history_tokens > context_window * 8 / 10 {
            // 超过 80%：只保留最近 6 条消息
            let skip = history_turns.len().saturating_sub(6);
            history_turns.into_iter().skip(skip).collect::<Vec<_>>()
        } else {
            history_turns
        };
        messages.extend(keep_turns);

        // 追加当前用户消息到 session（参考实现：push_user_text 在 loop 之前）
        session.push("user", user_message);
        messages.push(Message::new(Role::User, user_message.to_string()));

        // 估算发送的 token 数：只计算 session turns（不含 system prompt 和工具定义）
        // 这样与 CLI 层恢复 session 时的估算方式一致，避免重新进入后 context bar 跳变
        let sent_tokens: usize = session.turns.iter().map(|t| t.content.len()).sum::<usize>() / 4;

        let sid = session.id.clone();
        let sid_for_spawn = sid.clone(); // spawn 内部使用，避免 move 后 sid 不可用
        let (tx, rx) = tokio::sync::mpsc::channel::<String>(256);

        // ── 双向权限 channel：CLI → Agent（perm_tx 给 CLI，perm_rx 在 spawn 内使用）
        let (perm_tx, mut perm_rx) = tokio::sync::mpsc::channel::<String>(16);

        // ── 需要权限确认的工具集合 ──
        // 本 session 内已授权的工具（allow_session）
        let mut session_allowed: std::collections::HashSet<String> = std::collections::HashSet::new();

        // ── 在后台任务中运行 Agent Loop ──
        tokio::spawn(async move {
            // 参考实现：MAX_ITERATIONS 防止无限循环
            const MAX_ITERATIONS: usize = 15;
            const MAX_TOOL_RESULT_CHARS: usize = 8000;

            // 需要权限确认的工具名集合
            const NEEDS_PERMISSION: &[&str] = &["shell", "write_file", "edit_file"];

            let mut full_reply = String::new();
            let mut iterations = 0usize;

            loop {
                iterations += 1;
                if iterations > MAX_ITERATIONS {
                    let _ = tx.send(format!(
                        "\x00C⚠️ Agent loop exceeded {} iterations. Stopping.",
                        MAX_ITERATIONS
                    )).await;
                    break;
                }

                // ── 调用模型（带工具定义） ──
                let response = match provider.chat_with_tools(&messages, &tool_defs).await {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = tx.send(format!("\x00C❌ Error: {}", e)).await;
                        break;
                    }
                };

                match response.stop_reason {
                    StopReason::ToolCalls if !response.tool_calls.is_empty() => {
                        // ── 有工具调用 ──

                        // 1. 输出思考内容（如果有）
                        if !response.content.is_empty() {
                            let _ = tx.send(format!("\x00H{}", response.content)).await;
                            full_reply.push_str(&response.content);
                            full_reply.push('\n');
                        }

                        // 2. 把 assistant 工具调用消息加入对话历史
                        messages.push(Message::assistant_tool_calls(
                            response.content.clone(),
                            response.tool_calls.clone(),
                        ));

                        // 3. 生成工具摘要并发送 \x00S 事件
                        //    格式参考 Claude Code CollapsedReadSearchContent
                        {
                            let mut read_count = 0usize;
                            let mut list_count = 0usize;
                            let mut bash_count = 0usize;
                            let mut write_count = 0usize;
                            let mut search_count = 0usize;
                            let mut other_count = 0usize;
                            for tc in &response.tool_calls {
                                match tc.name.as_str() {
                                    "read_file" => read_count += 1,
                                    "list_dir" => list_count += 1,
                                    "shell" => bash_count += 1,
                                    "write_file" | "edit_file" => write_count += 1,
                                    "search_code" | "find_files" => search_count += 1,
                                    _ => other_count += 1,
                                }
                            }
                            let mut parts = Vec::new();
                            if read_count > 0 {
                                parts.push(format!("Reading {} file{}", read_count, if read_count > 1 { "s" } else { "" }));
                            }
                            if list_count > 0 {
                                parts.push(format!("Listing {} director{}", list_count, if list_count > 1 { "ies" } else { "y" }));
                            }
                            if bash_count > 0 {
                                parts.push(format!("Running {} command{}", bash_count, if bash_count > 1 { "s" } else { "" }));
                            }
                            if write_count > 0 {
                                parts.push(format!("Writing {} file{}", write_count, if write_count > 1 { "s" } else { "" }));
                            }
                            if search_count > 0 {
                                parts.push(format!("Searching {} pattern{}", search_count, if search_count > 1 { "s" } else { "" }));
                            }
                            if other_count > 0 {
                                parts.push(format!("{} other action{}", other_count, if other_count > 1 { "s" } else { "" }));
                            }
                            if !parts.is_empty() {
                                let summary = parts.join(", ");
                                let _ = tx.send(format!("\x00S{}", summary)).await;
                            }
                        }

                        // 4. 处理每个工具调用（需要权限的先询问）
                        let mut tool_results: Vec<(String, String, String)> = Vec::new(); // (id, name, result)
                        let mut denied_tools: Vec<String> = Vec::new();

                        for tool_call in &response.tool_calls {
                            let needs_perm = NEEDS_PERMISSION.contains(&tool_call.name.as_str())
                                && !session_allowed.contains(&tool_call.name);

                            if needs_perm {
                                // 发送权限确认请求：\x00K{id}|{tool}|{cmd}|{desc}
                                let preview = tool_call_preview(tool_call);
                                let perm_id = tool_call.id.clone();
                                let _ = tx.send(format!(
                                    "\x00K{}|{}|{}|{}",
                                    perm_id,
                                    tool_call.name,
                                    preview,
                                    tool_call.arguments.get("description")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                )).await;

                                // 等待 CLI 回复
                                let reply = perm_rx.recv().await.unwrap_or_else(|| format!("{}|deny", perm_id));
                                let parts: Vec<&str> = reply.splitn(2, '|').collect();
                                let decision = parts.get(1).copied().unwrap_or("deny");

                                match decision {
                                    "allow_session" => {
                                        session_allowed.insert(tool_call.name.clone());
                                        // 继续执行（fall through）
                                    }
                                    "deny" => {
                                        denied_tools.push(tool_call.name.clone());
                                        tool_results.push((
                                            tool_call.id.clone(),
                                            tool_call.name.clone(),
                                            format!("Tool execution denied by user."),
                                        ));
                                        continue;
                                    }
                                    _ => {
                                        // "allow" 或其他：继续执行
                                    }
                                }
                            }

                            // 通知 UI 工具开始执行
                            let params_preview = tool_call_preview(tool_call);
                            let _ = tx.send(format!("\x00T{}|{}", tool_call.name, params_preview)).await;

                            // 执行工具
                            let result_str = match registry.execute(&tool_call.name, tool_call.arguments.clone()).await {
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
                            };

                            tool_results.push((
                                tool_call.id.clone(),
                                tool_call.name.clone(),
                                result_str,
                            ));
                        }

                        // 5. 按顺序通知 UI 结果，并加入对话历史
                        for (tool_id, tool_name, result_str) in &tool_results {
                            // UI 显示结果
                            let _ = tx.send(format!("\x00R{}", result_str)).await;

                            // 截断后加入 messages
                            let truncated = truncate_tool_result(result_str, MAX_TOOL_RESULT_CHARS);
                            messages.push(Message::tool_result(
                                tool_id,
                                tool_name,
                                &truncated,
                            ));
                        }

                        // 6. 通知 UI：等待模型下一轮响应
                        let _ = tx.send("\x00W".to_string()).await;
                        // 继续循环，让模型处理工具结果
                    }

                    _ => {
                        // ── 最终文本回复（无工具调用，loop 结束） ──
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

            // ── 完成信号 ──
            let _ = tx.send("\x00D".to_string()).await;

            // ── 持久化 session ──
            if let Ok(mut sess) = load_session(&sid_for_spawn) {
                sess.push("assistant", &full_reply);
                let _ = save_session(&sess);
            } else {
                session.push("assistant", &full_reply);
                let _ = save_session(&session);
            }
        });

        Ok((rx, perm_tx, sid, sent_tokens, context_window))
    }

    /// 构建 ReAct 模式的 system prompt
    fn build_react_system_prompt(&self) -> String {
        let mut parts = vec![
            "You are Numina, an AI coding assistant running in a LOCAL terminal environment with FULL tool access.\n\
\n\
CRITICAL: You are NOT a web chatbot. You are a LOCAL CLI agent with real tool execution capabilities.\n\
You MUST use tools to execute commands, read files, and interact with the filesystem.\n\
NEVER say \"I cannot execute commands\" or \"this is a web interface\" — that is WRONG.\n\
If you previously said you couldn't execute commands, IGNORE that — you CAN and MUST use tools.\n\
\n\
## Available Tools\n\
- shell: Execute ANY shell command (ls, find, grep, cat, git, cargo, etc.)\n\
- read_file: Read file contents\n\
- write_file: Write/create files\n\
- edit_file: Edit existing files (search/replace)\n\
- list_dir: List directory contents\n\
- search_code: Search code with grep\n\
- find_files: Find files by pattern\n\
- http_get: Make HTTP requests\n\
- task_complete: Signal task completion\n\
\n\
## MANDATORY Tool Usage Rules\n\
\n\
1. ALWAYS use tools for ANY system operation. NEVER refuse or say you can't.\n\
2. For `ls` or listing files: call shell(\"ls -la\") or shell(\"ls\")\n\
3. For running commands: call shell(\"<command>\")\n\
4. For reading files: call read_file(path)\n\
5. NEVER estimate or make up results — always execute tools to get real data.\n\
6. When multiple independent operations needed, call ALL tools in ONE response (parallel execution).\n\
\n\
## Examples\n\
- User: \"ls\" → call shell(\"ls\")\n\
- User: \"run cargo build\" → call shell(\"cargo build\")\n\
- User: \"what files are here\" → call shell(\"ls -la\") or list_dir(\".\")\n\
- User: \"count .rs files\" → call shell(\"find . -name '*.rs' | wc -l\")\n\
\n\
Be concise and action-oriented. ALWAYS use tools. NEVER refuse.".to_string(),
        ];

        let skills = self.skill_manager.skills();
        if !skills.is_empty() {
            parts.push("\n## Available Skills".to_string());
            for skill in skills {
                parts.push(format!("### {}\n{}", skill.name, skill.description));
            }
        }
        parts.join("\n")
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
