/// 飞书事件订阅实现
///
/// 通过启动子进程 `lark-cli event +subscribe --event-types im.message.receive_v1 --compact --quiet`
/// 读取 NDJSON 输出，解析并过滤消息，推送到统一队列。
///
/// compact 格式示例：
/// {"type":"im.message.receive_v1","id":"om_xxx","message_id":"om_xxx",
///  "chat_id":"oc_xxx","chat_type":"p2p","message_type":"text",
///  "content":"Hello","sender_id":"ou_xxx","create_time":"1773491924409",
///  "timestamp":"1773491924409"}

use std::collections::HashMap;
use std::process::Stdio;

use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};

use crate::channel::types::{
    Channel, ChatType, IncomingMessage, MessageKind, MessageSource,
};

// ─────────────────────────────────────────────
// 飞书 compact 事件结构
// ─────────────────────────────────────────────

/// lark-cli event +subscribe --compact 输出的消息结构
#[derive(Debug, Deserialize)]
struct LarkCompactEvent {
    /// 事件类型，如 "im.message.receive_v1"
    #[serde(rename = "type")]
    event_type: String,

    /// 消息 ID（om_xxx）
    #[serde(default)]
    message_id: String,

    /// 会话 ID（oc_xxx）
    #[serde(default)]
    chat_id: String,

    /// 会话类型："p2p" 或 "group"
    #[serde(default)]
    chat_type: String,

    /// 消息类型："text", "post", "image", "file", "interactive" 等
    #[serde(default)]
    message_type: String,

    /// 消息内容（已解析为纯文本）
    #[serde(default)]
    content: String,

    /// 发送者 open_id（ou_xxx）
    #[serde(default)]
    sender_id: String,

    /// 时间戳（毫秒字符串）
    #[serde(default)]
    timestamp: String,

    /// @提及列表（原始 JSON，可能是数组）
    #[serde(default)]
    mentions: Option<serde_json::Value>,
}

impl LarkCompactEvent {
    /// 判断是否是私聊消息
    fn is_p2p(&self) -> bool {
        self.chat_type == "p2p"
    }

    /// 判断是否 @了机器人
    /// mentions 字段是一个数组，每个元素包含 id_type, id, name, tenant_key
    /// 机器人的 id_type 通常是 "app" 或者 name 包含 bot 标识
    /// 实际上飞书 compact 格式中 mentions 包含被 @ 的用户/机器人列表
    fn is_mention_bot(&self) -> bool {
        if let Some(mentions) = &self.mentions {
            if let Some(arr) = mentions.as_array() {
                for item in arr {
                    // 检查是否有 "key" 字段为 "@_all" 或者 id_type 为 "app"
                    let id_type = item.get("id_type").and_then(|v| v.as_str()).unwrap_or("");
                    let key = item.get("key").and_then(|v| v.as_str()).unwrap_or("");
                    // 飞书机器人被 @ 时，mentions 中会有 id_type = "app" 的条目
                    // 或者 key 以 "@_" 开头（@所有人等特殊情况）
                    if id_type == "app" || key.starts_with("@_") {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// 解析时间戳（毫秒 -> DateTime<Utc>）
    fn parse_timestamp(&self) -> chrono::DateTime<Utc> {
        self.timestamp
            .parse::<i64>()
            .ok()
            .and_then(|ms| Utc.timestamp_millis_opt(ms).single())
            .unwrap_or_else(Utc::now)
    }

    /// 转换为 MessageKind
    fn message_kind(&self) -> MessageKind {
        match self.message_type.as_str() {
            "text" => MessageKind::Text,
            "post" => MessageKind::RichText,
            "image" => MessageKind::Image,
            "file" | "audio" | "media" => MessageKind::File,
            "interactive" => MessageKind::Card,
            other => MessageKind::Other(other.to_string()),
        }
    }

    /// 转换为 ChatType
    fn chat_type_enum(&self) -> ChatType {
        match self.chat_type.as_str() {
            "p2p" => ChatType::P2p,
            "group" => ChatType::Group,
            other => ChatType::Other(other.to_string()),
        }
    }

    /// 转换为统一的 IncomingMessage
    fn into_incoming_message(self, raw: serde_json::Value) -> IncomingMessage {
        let is_direct = self.is_p2p();
        let is_mention = self.is_mention_bot();
        let timestamp = self.parse_timestamp();
        let kind = self.message_kind();
        let chat_type = self.chat_type_enum();

        let mut extra = HashMap::new();
        extra.insert("message_id".to_string(), self.message_id.clone());
        extra.insert("event_type".to_string(), self.event_type.clone());

        IncomingMessage {
            id: self.message_id.clone(),
            source: MessageSource::Lark,
            kind,
            content: self.content,
            sender_id: self.sender_id,
            sender_name: None,
            chat_id: self.chat_id,
            chat_type,
            is_mention,
            is_direct,
            timestamp,
            raw,
            extra,
        }
    }
}

// ─────────────────────────────────────────────
// LarkChannel
// ─────────────────────────────────────────────

/// 飞书 Channel
///
/// 通过 lark-cli 订阅飞书 WebSocket 事件，
/// 过滤出私聊和 @机器人 的消息推送到队列。
pub struct LarkChannel {
    /// channel 名称
    name: String,
    /// lark-cli 可执行文件路径（默认 "lark-cli"）
    lark_cli_path: String,
    /// 额外的 lark-cli 参数（如 --app-id, --app-secret）
    extra_args: Vec<String>,
}

impl LarkChannel {
    /// 创建默认 LarkChannel（使用系统 PATH 中的 lark-cli）
    pub fn new() -> Self {
        Self {
            name: "lark".to_string(),
            lark_cli_path: "lark-cli".to_string(),
            extra_args: Vec::new(),
        }
    }

    /// 指定 lark-cli 路径
    pub fn with_cli_path(mut self, path: impl Into<String>) -> Self {
        self.lark_cli_path = path.into();
        self
    }

    /// 添加额外参数（如 --app-id xxx --app-secret yyy）
    pub fn with_extra_args(mut self, args: Vec<String>) -> Self {
        self.extra_args = args;
        self
    }

    /// 构建 lark-cli 命令参数
    fn build_args(&self) -> Vec<String> {
        let mut args = vec![
            "event".to_string(),
            "+subscribe".to_string(),
            "--event-types".to_string(),
            "im.message.receive_v1".to_string(),
            "--compact".to_string(),
            "--quiet".to_string(),
            "--as".to_string(),
            "bot".to_string(),
        ];
        args.extend(self.extra_args.clone());
        args
    }

    /// 解析一行 NDJSON，返回 IncomingMessage（如果需要处理）
    fn parse_line(line: &str) -> Option<IncomingMessage> {
        if line.trim().is_empty() {
            return None;
        }

        // 解析原始 JSON
        let raw: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                debug!("LarkChannel: failed to parse JSON line: {} | error: {}", line, e);
                return None;
            }
        };

        // 解析为 compact 事件结构
        let event: LarkCompactEvent = match serde_json::from_value(raw.clone()) {
            Ok(e) => e,
            Err(e) => {
                debug!("LarkChannel: failed to deserialize event: {}", e);
                return None;
            }
        };

        // 只处理 im.message.receive_v1 事件
        if event.event_type != "im.message.receive_v1" {
            debug!("LarkChannel: ignoring event type: {}", event.event_type);
            return None;
        }

        // 过滤：只保留私聊 或 @机器人 的消息
        let is_direct = event.is_p2p();
        let is_mention = event.is_mention_bot();

        if !is_direct && !is_mention {
            debug!(
                "LarkChannel: ignoring group message without mention (chat_id: {})",
                event.chat_id
            );
            return None;
        }

        // 过滤空内容
        if event.content.trim().is_empty() {
            debug!("LarkChannel: ignoring empty content message");
            return None;
        }

        let msg = event.into_incoming_message(raw);
        info!(
            "LarkChannel: received message id={} chat_type={} is_direct={} is_mention={}",
            msg.id, msg.chat_type, msg.is_direct, msg.is_mention
        );

        Some(msg)
    }
}

impl Default for LarkChannel {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Channel for LarkChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn source(&self) -> MessageSource {
        MessageSource::Lark
    }

    /// 启动飞书事件订阅
    ///
    /// 通过子进程运行 `lark-cli event +subscribe --event-types im.message.receive_v1 --compact --quiet`
    /// 逐行读取 NDJSON 输出，解析并过滤消息，推送到 tx。
    ///
    /// 支持自动重连：子进程退出后等待 5 秒重启（除非收到 shutdown 信号）
    async fn run(
        &self,
        tx: mpsc::Sender<IncomingMessage>,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<()> {
        let args = self.build_args();
        let cli_path = self.lark_cli_path.clone();

        info!(
            "LarkChannel: starting lark-cli with args: {} {}",
            cli_path,
            args.join(" ")
        );

        loop {
            // 检查 shutdown
            if *shutdown.borrow() {
                info!("LarkChannel: shutdown signal received, stopping");
                break;
            }

            // 启动子进程
            let mut child = match tokio::process::Command::new(&cli_path)
                .args(&args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    error!(
                        "LarkChannel: failed to start lark-cli: {}. \
                         Make sure lark-cli is installed and configured.",
                        e
                    );
                    // 等待 10 秒后重试
                    tokio::select! {
                        _ = tokio::time::sleep(tokio::time::Duration::from_secs(10)) => {}
                        _ = shutdown.changed() => {
                            if *shutdown.borrow() { break; }
                        }
                    }
                    continue;
                }
            };

            let stdout = child.stdout.take().context("failed to get stdout")?;
            let stderr = child.stderr.take().context("failed to get stderr")?;

            let mut stdout_reader = BufReader::new(stdout).lines();
            let mut stderr_reader = BufReader::new(stderr).lines();

            info!("LarkChannel: lark-cli started, listening for events...");

            // 并发读取 stdout（事件）和 stderr（日志），同时监听 shutdown
            loop {
                tokio::select! {
                    // 读取 stdout（NDJSON 事件行）
                    line = stdout_reader.next_line() => {
                        match line {
                            Ok(Some(line)) => {
                                if let Some(msg) = Self::parse_line(&line) {
                                    if tx.send(msg).await.is_err() {
                                        warn!("LarkChannel: message queue closed, stopping");
                                        let _ = child.kill().await;
                                        return Ok(());
                                    }
                                }
                            }
                            Ok(None) => {
                                // stdout 关闭，子进程可能已退出
                                info!("LarkChannel: lark-cli stdout closed");
                                break;
                            }
                            Err(e) => {
                                error!("LarkChannel: stdout read error: {}", e);
                                break;
                            }
                        }
                    }

                    // 读取 stderr（lark-cli 的状态日志）
                    line = stderr_reader.next_line() => {
                        match line {
                            Ok(Some(line)) => {
                                debug!("LarkChannel [lark-cli stderr]: {}", line);
                            }
                            Ok(None) => {}
                            Err(_) => {}
                        }
                    }

                    // 监听 shutdown 信号
                    _ = shutdown.changed() => {
                        if *shutdown.borrow() {
                            info!("LarkChannel: shutdown signal, killing lark-cli");
                            let _ = child.kill().await;
                            return Ok(());
                        }
                    }
                }
            }

            // 等待子进程退出
            match child.wait().await {
                Ok(status) => {
                    info!("LarkChannel: lark-cli exited with status: {}", status);
                }
                Err(e) => {
                    warn!("LarkChannel: failed to wait for lark-cli: {}", e);
                }
            }

            // 检查是否需要 shutdown
            if *shutdown.borrow() {
                break;
            }

            // 自动重连：等待 5 秒
            warn!("LarkChannel: lark-cli exited, reconnecting in 5 seconds...");
            tokio::select! {
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(5)) => {}
                _ = shutdown.changed() => {
                    if *shutdown.borrow() { break; }
                }
            }
        }

        info!("LarkChannel: stopped");
        Ok(())
    }
}

// ─────────────────────────────────────────────
// 单元测试
// ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_p2p_message() {
        let line = r#"{"type":"im.message.receive_v1","id":"om_001","message_id":"om_001","chat_id":"oc_001","chat_type":"p2p","message_type":"text","content":"Hello Numina","sender_id":"ou_001","create_time":"1773491924409","timestamp":"1773491924409"}"#;

        let msg = LarkChannel::parse_line(line).expect("should parse p2p message");
        assert_eq!(msg.content, "Hello Numina");
        assert!(msg.is_direct);
        assert!(!msg.is_mention);
        assert!(msg.should_process());
        assert_eq!(msg.source, MessageSource::Lark);
    }

    #[test]
    fn test_parse_group_message_without_mention() {
        let line = r#"{"type":"im.message.receive_v1","id":"om_002","message_id":"om_002","chat_id":"oc_002","chat_type":"group","message_type":"text","content":"普通群消息","sender_id":"ou_002","timestamp":"1773491924409"}"#;

        // 群聊中未 @机器人，应该被过滤掉
        let msg = LarkChannel::parse_line(line);
        assert!(msg.is_none(), "group message without mention should be filtered");
    }

    #[test]
    fn test_parse_group_message_with_mention() {
        let line = r#"{"type":"im.message.receive_v1","id":"om_003","message_id":"om_003","chat_id":"oc_003","chat_type":"group","message_type":"text","content":"@Numina 帮我查一下","sender_id":"ou_003","timestamp":"1773491924409","mentions":[{"key":"@_user_1","id_type":"app","id":"cli_xxx","name":"Numina"}]}"#;

        let msg = LarkChannel::parse_line(line).expect("should parse group message with mention");
        assert!(!msg.is_direct);
        assert!(msg.is_mention);
        assert!(msg.should_process());
    }

    #[test]
    fn test_parse_empty_content() {
        let line = r#"{"type":"im.message.receive_v1","id":"om_004","message_id":"om_004","chat_id":"oc_004","chat_type":"p2p","message_type":"text","content":"","sender_id":"ou_004","timestamp":"1773491924409"}"#;

        // 空内容应该被过滤
        let msg = LarkChannel::parse_line(line);
        assert!(msg.is_none(), "empty content should be filtered");
    }

    #[test]
    fn test_parse_non_message_event() {
        let line = r#"{"type":"im.chat.updated_v1","chat_id":"oc_005","timestamp":"1773491924409"}"#;

        // 非消息事件应该被过滤
        let msg = LarkChannel::parse_line(line);
        assert!(msg.is_none(), "non-message event should be filtered");
    }

    #[test]
    fn test_lark_channel_build_args() {
        let channel = LarkChannel::new();
        let args = channel.build_args();
        assert!(args.contains(&"event".to_string()));
        assert!(args.contains(&"+subscribe".to_string()));
        assert!(args.contains(&"--compact".to_string()));
        assert!(args.contains(&"--quiet".to_string()));
        assert!(args.contains(&"im.message.receive_v1".to_string()));
    }
}
