/// Channel 核心类型定义

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─────────────────────────────────────────────
// 消息来源
// ─────────────────────────────────────────────

/// 消息来源渠道
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageSource {
    /// 飞书（Lark/Feishu）
    Lark,
    /// 微信
    Wechat,
    /// 自定义渠道
    Custom(String),
}

impl std::fmt::Display for MessageSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageSource::Lark => write!(f, "lark"),
            MessageSource::Wechat => write!(f, "wechat"),
            MessageSource::Custom(name) => write!(f, "custom:{}", name),
        }
    }
}

// ─────────────────────────────────────────────
// 消息类型
// ─────────────────────────────────────────────

/// 消息类型
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    /// 纯文本
    Text,
    /// 富文本（post）
    RichText,
    /// 图片
    Image,
    /// 文件
    File,
    /// 卡片消息
    Card,
    /// 其他
    Other(String),
}

impl std::fmt::Display for MessageKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageKind::Text => write!(f, "text"),
            MessageKind::RichText => write!(f, "rich_text"),
            MessageKind::Image => write!(f, "image"),
            MessageKind::File => write!(f, "file"),
            MessageKind::Card => write!(f, "card"),
            MessageKind::Other(t) => write!(f, "{}", t),
        }
    }
}

// ─────────────────────────────────────────────
// 统一消息结构
// ─────────────────────────────────────────────

/// 从各渠道接收到的统一消息结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingMessage {
    /// 消息唯一 ID（渠道内唯一）
    pub id: String,
    /// 消息来源渠道
    pub source: MessageSource,
    /// 消息类型
    pub kind: MessageKind,
    /// 消息文本内容（已解析为纯文本）
    pub content: String,
    /// 发送者 ID（渠道内的用户标识）
    pub sender_id: String,
    /// 发送者名称（可选）
    pub sender_name: Option<String>,
    /// 会话/群组 ID
    pub chat_id: String,
    /// 会话类型：p2p（私聊）/ group（群聊）
    pub chat_type: ChatType,
    /// 是否是 @机器人 的消息（群聊中）
    pub is_mention: bool,
    /// 是否是私聊消息
    pub is_direct: bool,
    /// 消息时间戳
    pub timestamp: DateTime<Utc>,
    /// 原始事件数据（JSON）
    pub raw: serde_json::Value,
    /// 扩展字段
    pub extra: HashMap<String, String>,
}

impl IncomingMessage {
    /// 判断是否需要处理（私聊 或 @机器人）
    pub fn should_process(&self) -> bool {
        self.is_direct || self.is_mention
    }
}

/// 会话类型
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatType {
    /// 私聊
    P2p,
    /// 群聊
    Group,
    /// 其他
    Other(String),
}

impl std::fmt::Display for ChatType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChatType::P2p => write!(f, "p2p"),
            ChatType::Group => write!(f, "group"),
            ChatType::Other(t) => write!(f, "{}", t),
        }
    }
}

// ─────────────────────────────────────────────
// 消息处理模式
// ─────────────────────────────────────────────

/// 消息处理模式
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingMode {
    /// React 模式：来一条消息立即触发 ReAct agent loop
    React,
    /// Buffer 模式：无脑接收存储，定期批量处理
    Buffer {
        /// 批量处理间隔（秒）
        interval_secs: u64,
    },
}

impl Default for ProcessingMode {
    fn default() -> Self {
        ProcessingMode::React
    }
}

// ─────────────────────────────────────────────
// Channel 配置
// ─────────────────────────────────────────────

/// 单个 channel 的配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    /// 渠道名称（唯一标识）
    pub name: String,
    /// 渠道类型
    pub source: MessageSource,
    /// 是否启用
    pub enabled: bool,
    /// 消息处理模式
    pub mode: ProcessingMode,
    /// 渠道特定配置（key-value）
    pub settings: HashMap<String, String>,
}

impl ChannelConfig {
    /// 获取配置项
    pub fn get(&self, key: &str) -> Option<&str> {
        self.settings.get(key).map(|s| s.as_str())
    }
}

// ─────────────────────────────────────────────
// Channel Trait
// ─────────────────────────────────────────────

/// Channel 抽象 trait
///
/// 每个渠道实现此 trait，在独立 tokio task 中运行，
/// 通过 mpsc channel 向 dispatcher 推送消息。
#[async_trait::async_trait]
pub trait Channel: Send + Sync {
    /// 渠道名称
    fn name(&self) -> &str;

    /// 渠道来源类型
    fn source(&self) -> MessageSource;

    /// 启动渠道监听（阻塞直到停止）
    /// 通过 tx 推送接收到的消息
    async fn run(
        &self,
        tx: tokio::sync::mpsc::Sender<IncomingMessage>,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> anyhow::Result<()>;
}
