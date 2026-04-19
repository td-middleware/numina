/// Channel — 用户输入渠道抽象层
///
/// 每个渠道（飞书、微信等）都实现 Channel trait，
/// 并在独立线程中接收消息，通过统一的 IncomingMessage 结构传递给处理器。
///
/// 消息处理模式：
/// 1. React 模式：来一条消息立即触发 ReAct agent loop（与 TUI 一样）
/// 2. Buffer 模式：无脑接收存储，定期批量交给模型或 skills 处理

pub mod lark;
pub mod types;
pub mod dispatcher;

pub use types::{IncomingMessage, MessageSource, ProcessingMode, ChannelConfig};
pub use dispatcher::ChannelDispatcher;
pub use lark::LarkChannel;
