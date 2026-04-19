/// 飞书（Lark/Feishu）Channel 实现
///
/// 通过调用 `lark-cli event +subscribe` 订阅飞书 WebSocket 事件，
/// 过滤出需要处理的消息（私聊 或 @机器人），转换为统一的 IncomingMessage 格式。
///
/// 过滤规则：
/// - 保留：私聊消息（chat_type == "p2p"）
/// - 保留：群聊中 @机器人 的消息（mentions 包含 bot）
/// - 屏蔽：群聊中未 @机器人 的消息
/// - 屏蔽：系统消息、通知等非用户消息

pub mod event;
pub mod intent;

pub use event::LarkChannel;
pub use intent::{IntentAnalyzer, IntentAnalysis, IntentOption, LarkIntentClarifier};
