/// ChannelDispatcher — 消息分发器
///
/// 负责：
/// 1. 管理所有已注册的 channel
/// 2. 为每个 channel 启动独立的 tokio task
/// 3. 汇聚所有 channel 的消息到统一队列
/// 4. 根据 ProcessingMode 分发给对应的处理器
///    - React 模式：立即触发 ReAct agent loop
///    - Buffer 模式：存入缓冲区，定期批量处理

use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::{mpsc, watch, Mutex};
use tracing::{error, info, warn};

use crate::channel::lark::intent::LarkIntentClarifier;
use crate::channel::types::{Channel, IncomingMessage, MessageSource, ProcessingMode};
use crate::core::chat::ChatEngine;
use crate::core::intent::{clarify_intent, IntentAnalyzer};

// ─────────────────────────────────────────────
// 消息处理器 trait
// ─────────────────────────────────────────────

/// 消息处理器 trait
/// React 模式下每条消息都会调用 handle_message
#[async_trait::async_trait]
pub trait MessageHandler: Send + Sync {
    async fn handle_message(&self, msg: IncomingMessage) -> anyhow::Result<()>;
}

// ─────────────────────────────────────────────
// 默认 React 处理器（调用 ChatEngine::chat_react）
// ─────────────────────────────────────────────

/// 默认 React 处理器：收到消息后先做意图澄清，再调用 ChatEngine 进行 ReAct 流程，
/// 并通过飞书 API 回复结果。
///
/// 意图澄清流程：
/// 1. 收到消息，先检查是否有待确认的意图选择（用户在回复之前的选项）
/// 2. 如果没有待确认，用模型分析意图是否明确
/// 3. 意图明确 → 直接执行 ReAct
/// 4. 意图不明确 → 通过 LarkIntentClarifier 发送选项，等待选择后再执行
pub struct ReactHandler {
    engine: Arc<ChatEngine>,
    /// 飞书意图澄清器（按 chat_id 隔离，每个会话独立）
    intent_clarifiers: Arc<Mutex<std::collections::HashMap<String, Arc<LarkIntentClarifier>>>>,
}

impl ReactHandler {
    pub fn new(engine: Arc<ChatEngine>) -> Self {
        Self {
            engine,
            intent_clarifiers: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// 获取或创建指定 chat_id 的意图澄清器
    async fn get_clarifier(&self, chat_id: &str, message_id: &str) -> Arc<LarkIntentClarifier> {
        let mut map = self.intent_clarifiers.lock().await;
        // 每次新消息都用新的 message_id 创建澄清器（message_id 用于回复）
        let clarifier = Arc::new(LarkIntentClarifier::new(message_id.to_string()));
        map.insert(chat_id.to_string(), clarifier.clone());
        clarifier
    }

    /// 获取已存在的澄清器（用于处理意图选择回复）
    async fn existing_clarifier(&self, chat_id: &str) -> Option<Arc<LarkIntentClarifier>> {
        let map = self.intent_clarifiers.lock().await;
        map.get(chat_id).cloned()
    }
}

// ─────────────────────────────────────────────
// 终端彩色打印辅助
// ─────────────────────────────────────────────

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const BLUE: &str = "\x1b[34m";
const GRAY: &str = "\x1b[38;5;244m";

/// 打印收到消息的日志（带时间戳和彩色格式）
fn print_incoming(msg: &IncomingMessage) {
    use std::io::Write;
    let now = chrono::Local::now().format("%H:%M:%S");
    let chat_label = if msg.is_direct {
        format!("{}私聊{}", CYAN, RESET)
    } else {
        format!("{}群聊@{}", BLUE, RESET)
    };
    let sender = if msg.sender_id.len() > 12 {
        format!("{}…{}", &msg.sender_id[..8], &msg.sender_id[msg.sender_id.len()-4..])
    } else {
        msg.sender_id.clone()
    };
    // 内容预览（最多 80 字符）
    let preview: String = msg.content.chars().take(80).collect();
    let ellipsis = if msg.content.chars().count() > 80 { "…" } else { "" };

    println!();
    println!("  {}┌─ 📨 收到消息 ─────────────────────────────────────{}", CYAN, RESET);
    println!("  {}│{} {}时间{} {}  {}来源{} {}  {}发送者{} {}{}{}",
        CYAN, RESET,
        DIM, RESET, now,
        DIM, RESET, chat_label,
        DIM, RESET, GRAY, sender, RESET);
    println!("  {}│{}", CYAN, RESET);
    println!("  {}│{} {}{}{}{}", CYAN, RESET, BOLD, preview, ellipsis, RESET);
    println!("  {}└────────────────────────────────────────────────────{}", CYAN, RESET);
    println!();
    std::io::stdout().flush().ok();
}

/// 打印 AI 正在处理的状态
fn print_processing(content: &str) {
    use std::io::Write;
    let preview: String = content.chars().take(60).collect();
    let ellipsis = if content.chars().count() > 60 { "…" } else { "" };
    println!("  {}⏳ AI 处理中：{}{}{}{}", DIM, RESET, preview, ellipsis, RESET);
    std::io::stdout().flush().ok();
}

/// 打印工具调用信息
fn print_tool_call(tool_name: &str, params: &str) {
    use std::io::Write;
    let preview: String = params.chars().take(60).collect();
    let ellipsis = if params.chars().count() > 60 { "…" } else { "" };
    println!("  {}  ● {}{}{} {}{}{}{}{}",
        CYAN, RESET, BOLD, tool_name, RESET, DIM, preview, ellipsis, RESET);
    std::io::stdout().flush().ok();
}

/// 打印回复成功
fn print_reply_sent(response: &str) {
    use std::io::Write;
    let now = chrono::Local::now().format("%H:%M:%S");
    let preview: String = response.chars().take(80).collect();
    let ellipsis = if response.chars().count() > 80 { "…" } else { "" };
    println!();
    println!("  {}┌─ ✅ 已回复 ────────────────────────────────────────{}", GREEN, RESET);
    println!("  {}│{} {}时间{} {}  {}长度{} {} 字符",
        GREEN, RESET,
        DIM, RESET, now,
        DIM, RESET, response.len());
    println!("  {}│{}", GREEN, RESET);
    println!("  {}│{} {}{}{}{}", GREEN, RESET, DIM, preview, ellipsis, RESET);
    println!("  {}└────────────────────────────────────────────────────{}", GREEN, RESET);
    println!();
    std::io::stdout().flush().ok();
}

/// 打印错误
fn print_error(msg: &str) {
    use std::io::Write;
    println!("  {}❌ 处理失败：{}{}", YELLOW, msg, RESET);
    println!();
    std::io::stdout().flush().ok();
}

#[async_trait::async_trait]
impl MessageHandler for ReactHandler {
    async fn handle_message(&self, msg: IncomingMessage) -> anyhow::Result<()> {
        info!(
            source = %msg.source,
            sender = %msg.sender_id,
            chat = %msg.chat_id,
            content = %msg.content,
            "ReactHandler: processing message"
        );

        // ── 终端打印：收到消息 ──
        print_incoming(&msg);

        let message_id = msg.extra.get("message_id").cloned().unwrap_or_default();

        // ── 需求1：立即给消息打 "Get"（[了解]）表情，表示已收到 ──
        // 异步执行，不阻塞后续处理流程
        if !message_id.is_empty() && msg.source == MessageSource::Lark {
            let mid = message_id.clone();
            tokio::spawn(async move {
                react_lark_message(&mid, "Get").await;
            });
        }

        // ── 第一步：检查是否是意图选择回复 ──
        // 如果当前会话有待确认的意图选择，优先处理
        if let Some(clarifier) = self.existing_clarifier(&msg.chat_id).await {
            if clarifier.has_pending().await {
                let consumed = clarifier.handle_reply(&msg.content, &message_id).await;
                if consumed {
                    info!("ReactHandler: message consumed as intent choice reply");
                    println!("  {}→ 已处理为意图选择回复{}", DIM, RESET);
                    return Ok(());
                }
            }
        }

        // ── 第二步：意图分析 ──
        // 只对飞书消息做意图澄清（其他渠道直接执行）
        let refined_prompt = if msg.source == MessageSource::Lark {
            let analyzer = IntentAnalyzer::new(self.engine.clone());
            let clarifier = self.get_clarifier(&msg.chat_id, &message_id).await;
            match clarify_intent(&analyzer, clarifier.as_ref(), &msg.content).await {
                Ok(Some(prompt)) => {
                    info!("ReactHandler: intent resolved, executing");
                    prompt
                }
                Ok(None) => {
                    // 用户取消或超时
                    info!("ReactHandler: user cancelled intent selection");
                    println!("  {}→ 用户取消意图选择{}", DIM, RESET);
                    return Ok(());
                }
                Err(e) => {
                    // 意图分析失败，直接用原始消息
                    warn!("ReactHandler: intent analysis failed: {}, using raw message", e);
                    msg.content.clone()
                }
            }
        } else {
            // 非飞书渠道，直接用原始消息
            msg.content.clone()
        };

        // ── 第三步：执行 ReAct agent loop ──
        let user_input = format!(
            "[来自 {} {} 的消息] {}",
            msg.source,
            if msg.is_direct { "私聊" } else { "群聊@" },
            refined_prompt
        );

        // 打印处理状态
        print_processing(&refined_prompt);

        // ── 需求2：飞书 channel 使用 chat_react_auto（自动批准所有工具，无需用户确认）
        // 每步操作通过 tracing::info! 记录日志，便于问题定位
        match self.engine.chat_react_auto(&user_input, None, None).await {
            Ok((mut rx, _perm_tx, _sid, _sent, _ctx)) => {
                let mut full_response = String::new();
                let mut step = 0usize;
                while let Some(event) = rx.recv().await {
                    if event == "\x00D" {
                        break;
                    } else if let Some(text) = event.strip_prefix("\x00C") {
                        full_response.push_str(text);
                    } else if let Some(tool_info) = event.strip_prefix("\x00T") {
                        // 打印工具调用 + 记录日志
                        step += 1;
                        let parts: Vec<&str> = tool_info.splitn(2, '|').collect();
                        let tool_name = parts.first().copied().unwrap_or("?");
                        let params = parts.get(1).copied().unwrap_or("");
                        print_tool_call(tool_name, params);
                        info!(
                            step = step,
                            tool = tool_name,
                            params = params,
                            "channel: tool executed (auto-approved)"
                        );
                    } else if let Some(summary) = event.strip_prefix("\x00S") {
                        // 打印工具摘要 + 记录日志
                        println!("  {}  ⏺ {}{}", DIM, summary, RESET);
                        info!(summary = summary, "channel: agent step summary");
                    } else if let Some(result) = event.strip_prefix("\x00R") {
                        // 记录工具结果日志（不打印到终端，避免刷屏）
                        let preview: String = result.chars().take(200).collect();
                        info!(result_preview = %preview, "channel: tool result received");
                    }
                    // 其他事件静默处理
                }

                if !full_response.is_empty() {
                    info!(
                        response_len = full_response.len(),
                        total_steps = step,
                        "ReactHandler: got response, sending reply"
                    );
                    // 打印回复内容
                    print_reply_sent(&full_response);
                    if !message_id.is_empty() {
                        reply_lark_message(&message_id, &full_response).await;
                    }
                } else {
                    println!("  {}→ AI 无回复内容{}", DIM, RESET);
                }
            }
            Err(e) => {
                error!("ReactHandler: chat_react_auto failed: {}", e);
                print_error(&e.to_string());
                if !message_id.is_empty() {
                    reply_lark_message(&message_id, &format!("❌ 处理失败：{}", e)).await;
                }
            }
        }

        Ok(())
    }
}

/// 通过 lark-cli 给飞书消息打表情（表示"已收到/Get"）
/// emoji_type: 飞书表情类型，如 "EYES"（👀）、"OK"（👌）、"THUMBSUP"（👍）
///
/// 正确命令格式：
///   lark-cli im reactions create \
///     --params '{"message_id": "om_xxx"}' \
///     --data '{"reaction_type": {"emoji_type": "EYES"}}' \
///     --as bot
async fn react_lark_message(message_id: &str, emoji_type: &str) {
    let params = serde_json::json!({"message_id": message_id}).to_string();
    let data = serde_json::json!({"reaction_type": {"emoji_type": emoji_type}}).to_string();

    let output = tokio::process::Command::new("lark-cli")
        .args([
            "im",
            "reactions",
            "create",
            "--params",
            &params,
            "--data",
            &data,
            "--as",
            "bot",
        ])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            info!("reaction '{}' added to message {}", emoji_type, message_id);
        }
        Ok(out) => {
            warn!(
                "lark-cli react failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Err(e) => {
            warn!("lark-cli not available for react: {}", e);
        }
    }
}

/// 通过 lark-cli 回复飞书消息
async fn reply_lark_message(message_id: &str, content: &str) {
    // 构建回复 JSON
    let reply_json = serde_json::json!({
        "msg_type": "text",
        "content": serde_json::json!({"text": content}).to_string()
    });
    let reply_str = reply_json.to_string();

    let output = tokio::process::Command::new("lark-cli")
        .args([
            "api",
            "POST",
            &format!("/open-apis/im/v1/messages/{}/reply", message_id),
            "--data",
            &reply_str,
            "--as",
            "bot",
            "--format",
            "data",
        ])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            info!("reply sent to message {}", message_id);
        }
        Ok(out) => {
            warn!(
                "lark-cli reply failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Err(e) => {
            warn!("lark-cli not available: {}", e);
        }
    }
}

// ─────────────────────────────────────────────
// Buffer 处理器
// ─────────────────────────────────────────────

/// Buffer 处理器：将消息存入缓冲区，定期批量处理
pub struct BufferHandler {
    buffer: Arc<Mutex<VecDeque<IncomingMessage>>>,
    interval_secs: u64,
    engine: Arc<ChatEngine>,
}

impl BufferHandler {
    pub fn new(engine: Arc<ChatEngine>, interval_secs: u64) -> Self {
        Self {
            buffer: Arc::new(Mutex::new(VecDeque::new())),
            interval_secs,
            engine,
        }
    }

    /// 启动定期批量处理任务
    pub fn start_batch_processor(&self, mut shutdown: watch::Receiver<bool>) {
        let buffer = self.buffer.clone();
        let interval = self.interval_secs;
        let engine = self.engine.clone();

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(
                tokio::time::Duration::from_secs(interval)
            );
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        let msgs: Vec<IncomingMessage> = {
                            let mut buf = buffer.lock().await;
                            buf.drain(..).collect()
                        };
                        if msgs.is_empty() {
                            continue;
                        }
                        info!("BufferHandler: processing {} buffered messages", msgs.len());
                        if let Err(e) = process_batch(&engine, msgs).await {
                            error!("BufferHandler: batch processing failed: {}", e);
                        }
                    }
                    _ = shutdown.changed() => {
                        if *shutdown.borrow() {
                            info!("BufferHandler: shutting down batch processor");
                            break;
                        }
                    }
                }
            }
        });
    }
}

#[async_trait::async_trait]
impl MessageHandler for BufferHandler {
    async fn handle_message(&self, msg: IncomingMessage) -> anyhow::Result<()> {
        let mut buf = self.buffer.lock().await;
        buf.push_back(msg);
        Ok(())
    }
}

/// 批量处理消息（将多条消息合并后发给模型）
async fn process_batch(
    engine: &ChatEngine,
    msgs: Vec<IncomingMessage>,
) -> anyhow::Result<()> {
    // 将多条消息合并成一个摘要请求
    let summary = msgs
        .iter()
        .map(|m| format!("[{}] {}: {}", m.source, m.sender_id, m.content))
        .collect::<Vec<_>>()
        .join("\n");

    let prompt = format!(
        "以下是最近收到的 {} 条消息，请分析并给出处理建议：\n\n{}",
        msgs.len(),
        summary
    );

    match engine.chat_once(&prompt, None, None).await {
        Ok((response, _sid, _tokens, _ctx)) => {
            info!("BufferHandler batch response: {} chars", response.len());
            // TODO: 可以将批量处理结果写入飞书文档或发送通知
        }
        Err(e) => {
            error!("BufferHandler: batch chat_once failed: {}", e);
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────
// ChannelDispatcher
// ─────────────────────────────────────────────

/// Channel 分发器
///
/// 管理所有 channel 的生命周期，汇聚消息并分发给处理器
pub struct ChannelDispatcher {
    /// 已注册的 channel 列表
    channels: Vec<(Box<dyn Channel>, ProcessingMode)>,
    /// 全局消息队列容量
    queue_capacity: usize,
}

impl ChannelDispatcher {
    pub fn new() -> Self {
        Self {
            channels: Vec::new(),
            queue_capacity: 1024,
        }
    }

    /// 注册一个 channel
    pub fn register(
        &mut self,
        channel: Box<dyn Channel>,
        mode: ProcessingMode,
    ) -> &mut Self {
        info!("ChannelDispatcher: registering channel '{}'", channel.name());
        self.channels.push((channel, mode));
        self
    }

    /// 启动所有 channel，开始接收和分发消息
    ///
    /// 此方法会阻塞直到收到 shutdown 信号
    pub async fn run(self, engine: Arc<ChatEngine>) -> anyhow::Result<()> {
        if self.channels.is_empty() {
            warn!("ChannelDispatcher: no channels registered");
            return Ok(());
        }

        // 全局消息队列
        let (tx, mut rx) = mpsc::channel::<IncomingMessage>(self.queue_capacity);

        // shutdown 信号
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

        // 为每个 channel 启动独立 task
        let mut handles = Vec::new();
        for (channel, _mode) in self.channels.iter() {
            let tx_clone = tx.clone();
            let shutdown_clone = shutdown_rx.clone();
            let channel_name = channel.name().to_string();

            info!("ChannelDispatcher: starting channel '{}'", channel_name);

            // 注意：Channel trait 需要 Arc 包装才能在多个 task 中共享
            // 这里通过 unsafe 的方式绕过生命周期限制（实际上 channel 的生命周期由 dispatcher 管理）
            // 更好的方式是将 channel 包装成 Arc<dyn Channel>
            let _ = (tx_clone, shutdown_clone, channel_name);
        }

        // 重新设计：将 channels 转换为 Arc 并启动 task
        let channels_arc: Vec<(Arc<dyn Channel>, ProcessingMode)> = self
            .channels
            .into_iter()
            .map(|(ch, mode)| {
                let ch: Arc<dyn Channel> = Arc::from(ch);
                (ch, mode)
            })
            .collect();

        for (channel, mode) in &channels_arc {
            let tx_clone = tx.clone();
            let shutdown_clone = shutdown_rx.clone();
            let channel_arc = channel.clone();
            let channel_name = channel.name().to_string();

            let handle = tokio::spawn(async move {
                info!("Channel '{}' task started", channel_name);
                if let Err(e) = channel_arc.run(tx_clone, shutdown_clone).await {
                    error!("Channel '{}' error: {}", channel_name, e);
                }
                info!("Channel '{}' task ended", channel_name);
            });
            handles.push(handle);

            // 如果是 Buffer 模式，启动批量处理器
            if let ProcessingMode::Buffer { interval_secs } = mode {
                let handler = BufferHandler::new(engine.clone(), *interval_secs);
                handler.start_batch_processor(shutdown_rx.clone());
            }
        }

        // 构建处理器映射（按 channel 名称）
        let react_handler = Arc::new(ReactHandler::new(engine.clone()));

        // 主消息分发循环
        info!("ChannelDispatcher: message dispatch loop started");

        // 监听 Ctrl+C
        let shutdown_tx_ctrlc = shutdown_tx.clone();
        tokio::spawn(async move {
            if let Ok(()) = tokio::signal::ctrl_c().await {
                info!("ChannelDispatcher: received Ctrl+C, shutting down");
                let _ = shutdown_tx_ctrlc.send(true);
            }
        });

        loop {
            tokio::select! {
                Some(msg) = rx.recv() => {
                    // 只处理需要处理的消息（私聊 或 @机器人）
                    if !msg.should_process() {
                        continue;
                    }

                    info!(
                        source = %msg.source,
                        sender = %msg.sender_id,
                        chat_type = %msg.chat_type,
                        is_direct = msg.is_direct,
                        is_mention = msg.is_mention,
                        "ChannelDispatcher: dispatching message"
                    );

                    // 找到对应 channel 的处理模式
                    let mode = channels_arc
                        .iter()
                        .find(|(ch, _)| ch.source() == msg.source)
                        .map(|(_, m)| m.clone())
                        .unwrap_or(ProcessingMode::React);

                    match mode {
                        ProcessingMode::React => {
                            let handler = react_handler.clone();
                            let msg_clone = msg.clone();
                            tokio::spawn(async move {
                                if let Err(e) = handler.handle_message(msg_clone).await {
                                    error!("ReactHandler error: {}", e);
                                }
                            });
                        }
                        ProcessingMode::Buffer { interval_secs } => {
                            // Buffer 模式：找到对应的 BufferHandler 并存入
                            // 简化实现：直接用 React 处理（完整实现需要 handler 注册表）
                            let handler = BufferHandler::new(engine.clone(), interval_secs);
                            if let Err(e) = handler.handle_message(msg).await {
                                error!("BufferHandler error: {}", e);
                            }
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("ChannelDispatcher: shutdown signal received");
                        break;
                    }
                }
            }
        }

        // 等待所有 channel task 结束
        for handle in handles {
            let _ = handle.await;
        }

        info!("ChannelDispatcher: all channels stopped");
        Ok(())
    }
}

impl Default for ChannelDispatcher {
    fn default() -> Self {
        Self::new()
    }
}
