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
/// 返回格式化后的参数字符串，供 runner.rs 渲染
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
        "http_post" => {
            // 展示 URL + 请求体 JSON（格式化）
            let url = tool_call.arguments["url"].as_str().unwrap_or("");
            // 返回完整 JSON 供 UI 格式化展示，前缀 URL 用 \x01 分隔
            let body_json = serde_json::to_string_pretty(&tool_call.arguments)
                .unwrap_or_default();
            format!("{}\x01{}", url, body_json)
        }
        "task_complete" => {
            // task_complete：只显示 result 的前 80 字符作为预览，不展示 JSON 块
            let result = tool_call.arguments["result"]
                .as_str()
                .unwrap_or("Task completed.");
            let preview: String = result.chars().take(80).collect();
            let ellipsis = if result.len() > 80 { "…" } else { "" };
            format!("{}{}", preview, ellipsis)
        }
        _ => {
            // 其他工具：返回格式化 JSON 供 UI 展示
            serde_json::to_string_pretty(&tool_call.arguments).unwrap_or_default()
        }
    }
}

/// 截断工具结果（防止超出模型 token 限制）
/// UI 层会折叠显示，这里只在极端情况下截断（超过 200k 字符）
fn truncate_tool_result(result: &str, _max_chars: usize) -> String {
    const HARD_LIMIT: usize = 200_000;
    if result.len() <= HARD_LIMIT {
        result.to_string()
    } else {
        let truncated: String = result.chars().take(HARD_LIMIT).collect();
        format!(
            "{}\n\n[... truncated at {} chars (hard limit). Result was {} chars total.]",
            truncated,
            HARD_LIMIT,
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

    /// 构建 system prompt（包含 skills 摘要 + 记忆注入）
    fn build_system_prompt(&self) -> String {
        self.build_system_prompt_with_query("")
    }

    /// 构建 system prompt（带查询词，用于相关记忆检索）
    fn build_system_prompt_with_query(&self, query: &str) -> String {
        let mut parts = vec![
            "You are Numina, an AI coding assistant. You help developers write, review, debug, and understand code.".to_string(),
            "Be concise, accurate, and helpful. When writing code, prefer idiomatic patterns.".to_string(),
        ];

        // 注入记忆（如果有）
        let memory_block = crate::memory::build_memory_prompt(query);
        if !memory_block.is_empty() {
            parts.push(String::new());
            parts.push(memory_block);
        }

        // 方案一：只注入轻量摘要（~50 tokens/skill），完整内容按需展开
        let skills_block = self.skill_manager.summary_prompt_block();
        if !skills_block.is_empty() {
            parts.push(String::new());
            parts.push(skills_block);
        }

        parts.join("\n")
    }

    /// 【方案一】根据用户输入，按需展开命中的 skill 完整内容
    /// 返回空字符串表示没有命中任何 skill
    pub fn expand_skills_for_input(&self, user_input: &str) -> String {
        self.skill_manager.expand_matched_skills(user_input)
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
    ) -> Result<(tokio::sync::mpsc::Receiver<String>, tokio::sync::mpsc::Sender<String>, String, usize, usize)> {
        self.chat_react_inner(user_message, model_override, session_id, false).await
    }

    /// 跳过 intent routing，并将 skill 内容和用户意图分开注入
    /// skill_content 作为 user/assistant 对（背景知识），user_intent 作为最终任务指令
    pub async fn chat_react_with_skill(
        &self,
        skill_content: &str,
        user_intent: &str,
        model_override: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<(tokio::sync::mpsc::Receiver<String>, tokio::sync::mpsc::Sender<String>, String, usize, usize)> {
        // 构建组合消息：skill 内容 + 用户意图，用特殊分隔符让模型明确区分
        // 格式：skill 内容作为背景，然后明确告知用户的实际任务
        let combined = format!(
            "{}\n\n---\nNow execute the following task using the skill instructions above:\n{}",
            skill_content, user_intent
        );
        self.chat_react_inner(&combined, model_override, session_id, true).await
    }

    async fn chat_react_inner(
        &self,
        user_message: &str,
        model_override: Option<&str>,
        session_id: Option<&str>,
        skip_intent_routing: bool,
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
            "~/.claude",
            "claude code",
            "claude.json",
            "anthropic api key",
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

        // ── 意图路由：自动匹配并展开 skill 完整内容 ──
        // skip_intent_routing=true 时跳过（避免 skill prompt 内容误触发其他 skill）
        if !skip_intent_routing {
            // 1. 优先检测直接引用（用户输入中包含 skill 名称，如 "lark-sheets" 或 "/lark-sheets"）
            let referenced = self.skill_manager.extract_referenced_skills(user_message);
            let expansion = if !referenced.is_empty() {
                // 直接引用：展开所有被引用的 skill 完整内容
                let mut lines = vec![
                    "## Referenced Skills (Full Instructions)".to_string(),
                    String::new(),
                    "The following skills were explicitly referenced in your request:".to_string(),
                    String::new(),
                ];
                for skill in &referenced {
                    lines.push(format!("### Skill: `{}` — {}", skill.name, skill.description));
                    lines.push(String::new());
                    lines.push(skill.content.clone());
                    lines.push(String::new());
                    lines.push("---".to_string());
                    lines.push(String::new());
                }
                lines.join("\n")
            } else {
                // 2. 退回到 when_to_use 关键词意图匹配
                self.skill_manager.expand_matched_skills(user_message)
            };

            if !expansion.is_empty() {
                messages.push(Message::new(Role::User, expansion));
                messages.push(Message::new(
                    Role::Assistant,
                    "Understood. I will follow the skill instructions above.".to_string(),
                ));
            }
        }

        // 估算发送的 token 数：只计算 session turns（不含 system prompt 和工具定义）
        // 这样与 CLI 层恢复 session 时的估算方式一致，避免重新进入后 context bar 跳变
        let sent_tokens: usize = session.turns.iter().map(|t| t.content.len()).sum::<usize>() / 4;

        let sid = session.id.clone();
        let sid_for_spawn = sid.clone(); // spawn 内部使用，避免 move 后 sid 不可用
        let user_message_owned = user_message.to_string(); // 转为 owned，供 spawn 内部使用
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
            const MAX_TOOL_RESULT_CHARS: usize = 10000;

            // 需要权限确认的工具名集合
            const NEEDS_PERMISSION: &[&str] = &["shell", "write_file", "edit_file", "http_post", "http_get"];

            // 辅助宏：检查 perm_rx 是否有取消信号（非阻塞）
            // 如果收到 deny_abort，立即退出 agent loop
            macro_rules! check_cancel {
                () => {
                    match perm_rx.try_recv() {
                        Ok(msg) if msg.contains("deny_abort") => {
                            let _ = tx.send("\x00D".to_string()).await;
                            save_session(&session).ok();
                            return;
                        }
                        _ => {}
                    }
                };
            }

            // ── 预取所有 MCP 服务器的 tools/list，注入到对话上下文 ──
            // 这样 AI 在第一次调用前就知道真实工具名和参数，不需要猜测
            if let Ok(mcp_cfg) = crate::config::mcp::McpConfig::load() {
                let http_servers: Vec<_> = mcp_cfg.servers.iter()
                    .filter(|s| s.enabled && (s.server_type == "http" || s.server_type == "websocket"))
                    .cloned()
                    .collect();

                if !http_servers.is_empty() {
                    let mut tools_context = String::from("\n\n[MCP Tools Available — use EXACT names below]\n");
                    for srv in &http_servers {
                        // 构建 headers
                        let mut headers_map = serde_json::Map::new();
                        for kv in &srv.env {
                            if let Some((k, v)) = kv.split_once('=') {
                                headers_map.insert(k.to_string(), serde_json::Value::String(v.to_string()));
                            }
                        }
                        let headers = serde_json::Value::Object(headers_map);

                        // 调用 tools/list
                        let list_body = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": 1,
                            "method": "tools/list",
                            "params": {}
                        });
                        let list_args = serde_json::json!({
                            "url": srv.command_or_url,
                            "body": list_body.to_string(),
                            "headers": headers
                        });

                        if let Ok(r) = registry.execute("http_post", list_args).await {
                            if r.success {
                                let content = r.data.get("content")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| serde_json::to_string_pretty(&r.data).unwrap_or_default());

                                // 解析 tools/list 响应，提取工具名和描述
                                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                                    let tools = json.get("result")
                                        .and_then(|r| r.get("tools"))
                                        .and_then(|t| t.as_array());

                                    if let Some(tool_list) = tools {
                                        tools_context.push_str(&format!("\n### MCP Server: {} ({})\n", srv.name, srv.command_or_url));
                                        for tool in tool_list {
                                            let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                                            let desc = tool.get("description").and_then(|v| v.as_str()).unwrap_or("");

                                            // ── 【方案二】检测工具描述中的 skill 路径引用 ──
                                            // 如果描述中包含 "skills/" 路径引用，提取 skill 名并注入实际内容
                                            // 支持格式：
                                            //   "优先使用~/.claude/skills/xxx/yyy.md"
                                            //   "先调用skills再执行"
                                            //   "skills是log-query"
                                            let enriched_desc = {
                                                let mut d = desc.to_string();
                                                // 提取 skill 名称（从路径或 "skills是xxx" 格式）
                                                let skill_name = if let Some(pos) = desc.find("skills/") {
                                                    // 从路径提取：skills/<name>/xxx.md 或 skills/<name>
                                                    let after = &desc[pos + 7..];
                                                    let end = after.find(|c: char| c == '/' || c == ',' || c == '，' || c == ' ' || c == '"' || c == '\'')
                                                        .unwrap_or(after.len());
                                                    Some(after[..end].to_string())
                                                } else if let Some(pos) = desc.find("skills是") {
                                                    let after = &desc[pos + "skills是".len()..];
                                                    let end = after.find(|c: char| c == ',' || c == '，' || c == ' ' || c == '"' || c == '\'')
                                                        .unwrap_or(after.len());
                                                    Some(after[..end].to_string())
                                                } else {
                                                    None
                                                };

                                                if let Some(sname) = skill_name {
                                                    // 尝试从 skill_manager 找到对应 skill
                                                    // 注意：这里在 spawn 内部，需要通过 skill_expansion 传入
                                                    // 简化处理：直接读取 ~/.numina/skills/<name>/SKILL.md
                                                    let skill_path = dirs::home_dir()
                                                        .map(|h| h.join(".numina").join("skills").join(&sname).join("SKILL.md"));
                                                    if let Some(path) = skill_path {
                                                        if let Ok(content) = std::fs::read_to_string(&path) {
                                                            // 提取 frontmatter 后的正文（跳过 --- 块）
                                                            let body = if content.starts_with("---") {
                                                                if let Some(end) = content[3..].find("\n---") {
                                                                    content[3 + end + 4..].trim_start_matches('\n').to_string()
                                                                } else {
                                                                    content.clone()
                                                                }
                                                            } else {
                                                                content.clone()
                                                            };
                                                            // 截取前 500 字符作为摘要注入工具描述
                                                            let summary: String = body.chars().take(500).collect();
                                                            d = format!("{}\n[Skill `{}` instructions preview]:\n{}", desc, sname, summary);
                                                        }
                                                    }
                                                }
                                                d
                                            };

                                            tools_context.push_str(&format!("- Tool: `{}`  — {}\n", name, enriched_desc));
                                            // 显示参数 schema
                                            if let Some(schema) = tool.get("inputSchema") {
                                                if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
                                                    let required: Vec<&str> = schema.get("required")
                                                        .and_then(|r| r.as_array())
                                                        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                                                        .unwrap_or_default();
                                                    for (param, info) in props {
                                                        let param_desc = info.get("description").and_then(|v| v.as_str()).unwrap_or("");
                                                        let req = if required.contains(&param.as_str()) { "*" } else { "" };
                                                        tools_context.push_str(&format!("  - {}{}: {}\n", param, req, param_desc));
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    // 将工具列表注入到对话上下文（作为 user 消息，让 AI 知道可用工具）
                    if tools_context.len() > 60 {
                        messages.push(Message::new(
                            Role::User,
                            format!("{}Use ONLY the tool names listed above. Do NOT invent or guess tool names.", tools_context),
                        ));
                        messages.push(Message::new(
                            Role::Assistant,
                            "I understand. I will use only the exact tool names listed above when calling MCP servers.".to_string(),
                        ));
                    }
                }
            }

            let mut full_reply = String::new();
            let mut iterations = 0usize;
            // 是否已注入"强制汇总"消息（只注入一次）
            let mut forced_summary = false;

            loop {
                // ── 每次迭代开始时检查 Esc 取消信号 ──
                check_cancel!();

                iterations += 1;
                if iterations > MAX_ITERATIONS {
                    // 超过最大迭代次数：强制注入一条 user 消息要求 AI 汇总，再调用一次模型
                    if !forced_summary {
                        forced_summary = true;
                        messages.push(Message::new(
                            Role::User,
                            "You have used many tool calls. Please STOP calling tools now and provide a complete summary of everything you have found so far. Do NOT call any more tools.".to_string(),
                        ));
                        // 重置计数，给 AI 最后一次机会输出汇总
                        iterations = 0;
                        continue;
                    }
                    // 已经强制汇总过了还没结束，直接退出
                    let _ = tx.send(
                        "\x00C⚠️ Agent loop exceeded maximum iterations. Stopping.".to_string()
                    ).await;
                    break;
                }

                // ── 调用模型（带工具定义），同时监听取消信号 ──
                let response = match tokio::select! {
                    res = provider.chat_with_tools(&messages, &tool_defs) => res,
                    cancel_msg = perm_rx.recv() => {
                        // 收到取消信号（Esc）：立即中止
                        let _ = cancel_msg; // 忽略具体内容
                        let _ = tx.send("\x00D".to_string()).await;
                        save_session(&session).ok();
                        return;
                    }
                } {
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
                            // 判断用户消息语言（简单检测：含中文字符则用中文）
                            let use_chinese = user_message_owned.chars().any(|c| (c as u32) > 0x4E00 && (c as u32) < 0x9FFF);
                            let mut parts = Vec::new();
                            if use_chinese {
                                if read_count > 0 { parts.push(format!("读取 {} 个文件", read_count)); }
                                if list_count > 0 { parts.push(format!("列出 {} 个目录", list_count)); }
                                if bash_count > 0 { parts.push(format!("执行 {} 条命令", bash_count)); }
                                if write_count > 0 { parts.push(format!("写入 {} 个文件", write_count)); }
                                if search_count > 0 { parts.push(format!("搜索 {} 个模式", search_count)); }
                                if other_count > 0 { parts.push(format!("{} 个其他操作", other_count)); }
                            } else {
                                if read_count > 0 { parts.push(format!("Reading {} file{}", read_count, if read_count > 1 { "s" } else { "" })); }
                                if list_count > 0 { parts.push(format!("Listing {} director{}", list_count, if list_count > 1 { "ies" } else { "y" })); }
                                if bash_count > 0 { parts.push(format!("Running {} command{}", bash_count, if bash_count > 1 { "s" } else { "" })); }
                                if write_count > 0 { parts.push(format!("Writing {} file{}", write_count, if write_count > 1 { "s" } else { "" })); }
                                if search_count > 0 { parts.push(format!("Searching {} pattern{}", search_count, if search_count > 1 { "s" } else { "" })); }
                                if other_count > 0 { parts.push(format!("{} other action{}", other_count, if other_count > 1 { "s" } else { "" })); }
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
                                    "deny_abort" => {
                                        // Esc 强制中止：立即终止整个 agent loop，返回聊天输入
                                        let _ = tx.send("\x00D".to_string()).await;
                                        save_session(&session).ok();
                                        return;
                                    }
                                    "deny" => {
                                        denied_tools.push(tool_call.name.clone());
                                        tool_results.push((
                                            tool_call.id.clone(),
                                            tool_call.name.clone(),
                                            "Tool execution denied by user.".to_string(),
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
                            let mut result_str = match registry.execute(&tool_call.name, tool_call.arguments.clone()).await {
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
                                Err(e) => {
                                    let err_str = e.to_string();
                                    // 工具不存在：给模型可用工具列表 + 替代建议
                                    if err_str.contains("Tool not found") || err_str.contains("not found") {
                                        let available: Vec<String> = registry.list_tools();
                                        let mut available_sorted = available.clone();
                                        available_sorted.sort();
                                        format!(
                                            "Tool '{}' does not exist in Numina.\n\
                                            Available tools: {}\n\
                                            HINT: There is no get_time/current_time/datetime tool. \
                                            To get the current time, use: shell(command=\"date\") or shell(command=\"date '+%Y-%m-%d %H:%M:%S'\")\n\
                                            Please retry using one of the available tools above.",
                                            tool_call.name,
                                            available_sorted.join(", ")
                                        )
                                    } else {
                                        format!("Tool execution failed: {}", e)
                                    }
                                }
                            };

                            // 检测 MCP -32603 错误（unknown tool）
                            // 自动调用 tools/list 获取真实工具列表，注入到结果中
                            if result_str.contains("-32603") || result_str.contains("unknown tool") {
                                let url = tool_call.arguments.get("url")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let headers = tool_call.arguments.get("headers").cloned()
                                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

                                // 自动调用 tools/list
                                let list_body = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": 99,
                                    "method": "tools/list",
                                    "params": {}
                                });
                                let list_args = serde_json::json!({
                                    "url": url,
                                    "body": list_body.to_string(),
                                    "headers": headers
                                });
                                let tools_list = match registry.execute("http_post", list_args).await {
                                    Ok(r) if r.success => {
                                        r.data.get("content")
                                            .and_then(|v| v.as_str())
                                            .map(|s| s.to_string())
                                            .unwrap_or_else(|| serde_json::to_string_pretty(&r.data).unwrap_or_default())
                                    }
                                    _ => String::new(),
                                };

                                if !tools_list.is_empty() {
                                    result_str = format!(
                                        "{}\n\n[SYSTEM HINT] Tool name was wrong. Available tools from tools/list:\n{}\n\nPlease retry with the EXACT tool name from the list above.",
                                        result_str, tools_list
                                    );
                                }
                            }

                            // 检测 MCP -32601 错误（Method not found）
                            // 不自动重试（重试无法解决 body 格式问题），改为给 AI 提示正确的调用格式
                            if result_str.contains("-32601") || result_str.contains("Method not found") {
                                result_str = format!(
                                    "{}\n\n[SYSTEM HINT] MCP returned -32601 'Method not found'. \
                                    Most likely cause: 'body' was passed as a JSON string instead of a JSON object. \
                                    CORRECT format — pass body as a JSON object directly:\n\
                                    http_post(url=..., body={{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"<tool>\",\"arguments\":{{...}}}}}}, headers=...)\n\
                                    WRONG: body=\"{{\\\"jsonrpc\\\":\\\"2.0\\\",...}}\" (string form causes truncation)\n\
                                    Please retry with body as a JSON object.",
                                    result_str
                                );
                            }

                            tool_results.push((
                                tool_call.id.clone(),
                                tool_call.name.clone(),
                                result_str,
                            ));
                        }

                        // 5. 按顺序通知 UI 结果，并加入对话历史
                        let mut task_complete_result: Option<String> = None;
                        for (tool_id, tool_name, result_str) in &tool_results {
                            // 检测 task_complete：立即提取 result 内容，准备作为最终回复输出
                            if tool_name == "task_complete" {
                                // task_complete 的 result_str 就是 result 字段内容（已在工具执行时提取）
                                task_complete_result = Some(result_str.clone());
                            }

                            // UI 显示结果（折叠预览）
                            let _ = tx.send(format!("\x00R{}", result_str)).await;

                            // 截断后加入 messages
                            let truncated = truncate_tool_result(result_str, MAX_TOOL_RESULT_CHARS);
                            messages.push(Message::tool_result(
                                tool_id,
                                tool_name,
                                &truncated,
                            ));
                        }

                        // 如果有 task_complete，直接输出 result 内容并结束 loop
                        if let Some(final_result) = task_complete_result {
                            let _ = tx.send(format!("\x00C{}", final_result)).await;
                            full_reply.push_str(&final_result);
                            break;
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
            "You are Numina, an AI assistant running in a local terminal with full tool access.\n\
\n\
## Tools\n\
- shell: run any shell command\n\
- read_file / write_file / edit_file: read, create, or edit files\n\
- list_dir / find_files / search_code: explore the filesystem\n\
- http_get / http_post: make HTTP requests (use http_post for MCP/JSON-RPC)\n\
- task_complete: signal task completion\n\
\n\
## MCP Servers (JSON-RPC 2.0 over HTTP)\n\
To call an MCP server:\n\
1. http_post(url=URL, body={\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\",\"params\":{}}, headers={...})\n\
2. http_post(url=URL, body={\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"TOOL\",\"arguments\":{...}}}, headers={...})\n\
Note: body must be a JSON object, not a string.".to_string(),
        ];

        // 注入 skills 轻量摘要（只含 when_to_use 的意图触发型 skills）
        let skills_block = self.skill_manager.summary_prompt_block();
        if !skills_block.is_empty() {
            parts.push(skills_block);
        }

        // 注入 MCP 服务器配置（URL + 必要 headers）
        if let Ok(mcp_cfg) = crate::config::mcp::McpConfig::load() {
            let http_servers: Vec<_> = mcp_cfg.servers.iter()
                .filter(|s| s.enabled && (s.server_type == "http" || s.server_type == "websocket"))
                .collect();
            if !http_servers.is_empty() {
                let mut mcp_block = "\n## Configured MCP Servers\n".to_string();
                for srv in &http_servers {
                    mcp_block.push_str(&format!("\n### {}\n- URL: {}\n", srv.name, srv.command_or_url));
                    if let Some(desc) = &srv.description {
                        mcp_block.push_str(&format!("- Description: {}\n", desc));
                    }
                    if !srv.env.is_empty() {
                        mcp_block.push_str("- Headers:\n");
                        for kv in &srv.env {
                            if let Some((k, v)) = kv.split_once('=') {
                                mcp_block.push_str(&format!("  - {}: {}\n", k, v));
                            }
                        }
                    }
                }
                parts.push(mcp_block);
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
        self.skill_manager.count()
    }

    /// 检查输入是否是一个 skill 调用，返回展开后的 prompt
    pub fn expand_skill_command(&self, input: &str) -> Option<String> {
        self.skill_manager
            .match_slash_command(input)
            .map(|(skill, args)| skill.expand_prompt(&args))
    }

    /// 检测普通输入（非 / 开头）中是否包含句中 /skill-name 引用
    /// 例如："基于/lark-im这个skill，给我发消息"
    /// 返回 (skill_content, user_intent, skill_name)
    ///   skill_content = skill 完整内容（不含用户意图，用于注入为背景知识）
    ///   user_intent   = 用户实际意图（去掉 /skill-name 引用部分，作为最终任务指令）
    pub fn extract_inline_skill_reference(&self, input: &str) -> Option<(String, String, String)> {
        // 只处理不以 / 开头的输入（以 / 开头的由 expand_skill_command 处理）
        if input.starts_with('/') {
            return None;
        }
        let referenced = self.skill_manager.extract_referenced_skills(input);
        if referenced.is_empty() {
            return None;
        }
        // 取第一个命中的 skill（优先级：/skill-name 引用 > 直接名称引用）
        let skill = referenced[0];
        // 从输入中去掉 /skill-name 引用，保留用户的实际意图描述
        let user_intent = input
            .replace(&format!("/{}", skill.name), "")
            .replace(&skill.name, "")
            .trim()
            .to_string();
        let user_intent = if user_intent.is_empty() { input.to_string() } else { user_intent };
        // skill_content：展开 skill 内容（不含用户意图，只做 ${SKILL_DIR} 等替换）
        let skill_content = skill.expand_prompt("");
        Some((skill_content, user_intent, skill.name.clone()))
    }

    /// 返回所有已加载 skill 的名称和描述（用于补全）
    pub fn skill_names(&self) -> Vec<(String, String)> {
        self.skill_manager
            .skills()
            .iter()
            .map(|s| (s.name.clone(), s.description.clone()))
            .collect()
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
