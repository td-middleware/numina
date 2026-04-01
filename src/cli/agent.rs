use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::core::agent::ActAgent;

// ─────────────────────────────────────────────
// CLI 参数定义
// ─────────────────────────────────────────────

#[derive(Parser)]
pub struct AgentArgs {
    #[command(subcommand)]
    command: Option<AgentCommands>,
}

#[derive(Subcommand)]
pub enum AgentCommands {
    /// 🚀 运行 ReAct Agent 完成一个任务（核心 Act 能力）
    Run {
        /// 任务描述（自然语言）
        task: String,

        /// 覆盖默认模型
        #[arg(short = 'o', long)]
        model: Option<String>,

        /// 工作目录（shell 工具的默认 cwd）
        #[arg(short = 'C', long)]
        cwd: Option<String>,

        /// 最大执行步数（默认 20）
        #[arg(long, default_value_t = 20)]
        max_steps: usize,

        /// 输出 JSON 格式的运行结果
        #[arg(long)]
        json: bool,

        /// 静默模式（不打印步骤详情）
        #[arg(short = 'q', long)]
        quiet: bool,
    },

    /// 列出可用的内置工具
    Tools,

    /// 列出所有 agent（占位，后续扩展）
    List,
}

// ─────────────────────────────────────────────
// 入口
// ─────────────────────────────────────────────

pub async fn execute(args: &AgentArgs) -> Result<()> {
    match &args.command {
        Some(AgentCommands::Run {
            task,
            model,
            cwd,
            max_steps,
            json,
            quiet,
        }) => {
            cmd_run(task, model.as_deref(), cwd.as_deref(), *max_steps, *json, *quiet).await
        }
        Some(AgentCommands::Tools) => cmd_tools(),
        Some(AgentCommands::List) => {
            println!("🤖 Agents are dynamically created per task.");
            println!("   Use `numina agent run <task>` to start an agent.");
            Ok(())
        }
        None => {
            println!("🤖 Numina Agent — autonomous task execution");
            println!();
            println!("USAGE:");
            println!("  numina agent run <task>          Run an agent to complete a task");
            println!("  numina agent tools               List available tools");
            println!();
            println!("EXAMPLES:");
            println!("  numina agent run \"列出当前目录的所有 Rust 文件\"");
            println!("  numina agent run \"读取 Cargo.toml 并总结依赖\" --cwd .");
            println!("  numina agent run \"帮我写一个 hello world 程序\" --model gpt-4o");
            Ok(())
        }
    }
}

// ─────────────────────────────────────────────
// run 子命令
// ─────────────────────────────────────────────

async fn cmd_run(
    task: &str,
    model: Option<&str>,
    cwd: Option<&str>,
    max_steps: usize,
    output_json: bool,
    quiet: bool,
) -> Result<()> {
    let agent = match ActAgent::new() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("⚠️  Failed to initialize agent: {}", e);
            eprintln!("   Run `numina config init` to set up your workspace.");
            return Err(e);
        }
    };

    let agent = agent
        .with_max_steps(max_steps)
        .with_verbose(!quiet);

    let result = agent.run(task, model, cwd).await?;

    if output_json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if quiet {
        // 静默模式只打印最终答案
        println!("{}", result.final_answer);
    } else {
        // verbose 模式已经在 run() 里打印了步骤，这里只打印摘要
        println!();
        println!("─────────────────────────────────────────");
        println!("📊 Summary: {} step(s)  success: {}", result.total_steps, result.success);
    }

    Ok(())
}

// ─────────────────────────────────────────────
// tools 子命令
// ─────────────────────────────────────────────

fn cmd_tools() -> Result<()> {
    use crate::core::tools::builtin::default_registry;

    let registry = default_registry();
    let tools = registry.list_tools();

    println!("🔧 Available Tools ({} total):\n", tools.len());
    for name in &tools {
        if let Some(executor) = registry.get(name) {
            println!("  📌 {}", name);
            println!("     {}", executor.description());
            println!();
        }
    }
    Ok(())
}
