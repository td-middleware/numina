use anyhow::Result;
use clap::{Parser, Subcommand};

// ─────────────────────────────────────────────
// CLI 参数定义（保留在此处，供 cli/mod.rs 引用）
// ─────────────────────────────────────────────

#[derive(Parser)]
pub struct ChatArgs {
    #[command(subcommand)]
    pub command: Option<ChatCommand>,

    /// 直接发送一条消息（非交互式）
    #[arg(short = 'M', long)]
    pub message: Option<String>,

    /// 覆盖默认模型（如 gpt-4o、claude-3-5-sonnet-20241022）
    #[arg(short = 'o', long)]
    pub model: Option<String>,

    /// 继续已有会话（传入 session ID）
    #[arg(short = 's', long)]
    pub session: Option<String>,

    /// 使用流式输出（逐 token 打印）
    #[arg(long, default_value_t = true)]
    pub stream: bool,
}

#[derive(Subcommand)]
pub enum ChatCommand {
    /// 列出所有历史会话
    Sessions,
    /// 查看某个会话的详细记录
    Show {
        /// Session ID
        session_id: String,
    },
}

// ─────────────────────────────────────────────
// 入口：委托给 session::runner
// ─────────────────────────────────────────────

pub async fn execute(args: &ChatArgs) -> Result<()> {
    crate::cli::session::runner::execute(args).await
}
