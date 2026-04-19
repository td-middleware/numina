/// channel 子命令 — 启动消息渠道监听
///
/// 用法：
///   numina channel lark              # 启动飞书 channel（React 模式）
///   numina channel lark --buffer 60  # 启动飞书 channel（Buffer 模式，每 60 秒批量处理）
///   numina channel lark --cli-path /usr/local/bin/lark-cli  # 指定 lark-cli 路径

use anyhow::Result;
use clap::{Args, Subcommand};
use std::sync::Arc;
use tracing::info;

use crate::channel::{ChannelDispatcher, LarkChannel};
use crate::channel::types::ProcessingMode;
use crate::core::chat::ChatEngine;

// ─────────────────────────────────────────────
// CLI 参数定义
// ─────────────────────────────────────────────

#[derive(Args)]
pub struct ChannelArgs {
    #[command(subcommand)]
    pub command: ChannelCommand,
}

#[derive(Subcommand)]
pub enum ChannelCommand {
    /// 启动飞书（Lark/Feishu）消息渠道
    Lark(LarkArgs),

    /// 列出所有已注册的 channel 状态
    Status,
}

#[derive(Args)]
pub struct LarkArgs {
    /// 使用 Buffer 模式，指定批量处理间隔（秒）
    /// 不指定则使用 React 模式（来一条处理一条）
    #[arg(long, value_name = "SECONDS")]
    pub buffer: Option<u64>,

    /// 指定 lark-cli 可执行文件路径（默认使用 PATH 中的 lark-cli）
    #[arg(long, value_name = "PATH", default_value = "lark-cli")]
    pub cli_path: String,

    /// 指定使用的 AI 模型（覆盖默认配置）
    #[arg(long, short = 'm', value_name = "MODEL")]
    pub model: Option<String>,

    /// 额外传递给 lark-cli 的参数（如 --app-id xxx）
    #[arg(last = true)]
    pub extra_args: Vec<String>,
}

// ─────────────────────────────────────────────
// 执行入口
// ─────────────────────────────────────────────

pub async fn execute(args: &ChannelArgs) -> Result<()> {
    match &args.command {
        ChannelCommand::Lark(lark_args) => run_lark(lark_args).await,
        ChannelCommand::Status => run_status().await,
    }
}

/// 启动飞书 channel
async fn run_lark(args: &LarkArgs) -> Result<()> {
    // 确定处理模式
    let mode = match args.buffer {
        Some(secs) => {
            println!("🔄 飞书 Channel 启动（Buffer 模式，每 {} 秒批量处理）", secs);
            ProcessingMode::Buffer { interval_secs: secs }
        }
        None => {
            println!("⚡ 飞书 Channel 启动（React 模式，实时处理）");
            ProcessingMode::React
        }
    };

    println!("📡 正在连接飞书 WebSocket 事件流...");
    println!("   过滤规则：仅处理私聊消息 和 群聊中 @机器人 的消息");
    println!("   按 Ctrl+C 停止\n");

    // 初始化 ChatEngine
    let engine = Arc::new(ChatEngine::new()?);
    info!("ChatEngine initialized, model: {}", engine.default_model());

    // 构建 LarkChannel
    let lark_channel = LarkChannel::new()
        .with_cli_path(&args.cli_path)
        .with_extra_args(args.extra_args.clone());

    // 构建 ChannelDispatcher 并注册 channel
    let mut dispatcher = ChannelDispatcher::new();
    dispatcher.register(Box::new(lark_channel), mode);

    // 启动（阻塞直到 Ctrl+C）
    dispatcher.run(engine).await?;

    println!("\n✅ 飞书 Channel 已停止");
    Ok(())
}

/// 显示 channel 状态
async fn run_status() -> Result<()> {
    println!("Channel 状态：");
    println!("  支持的渠道：");
    println!("    - lark    飞书/Feishu（通过 lark-cli WebSocket 订阅）");
    println!("    - wechat  微信（待实现）");
    println!();
    println!("  处理模式：");
    println!("    - react   来一条消息立即触发 ReAct agent loop（默认）");
    println!("    - buffer  无脑接收存储，定期批量交给模型处理");
    println!();
    println!("  使用示例：");
    println!("    numina channel lark              # 飞书 React 模式");
    println!("    numina channel lark --buffer 60  # 飞书 Buffer 模式（60秒批量）");
    Ok(())
}
