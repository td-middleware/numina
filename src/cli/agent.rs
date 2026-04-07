/// CLI Agent 命令
///
/// 提供两种使用方式：
/// 1. 单次任务模式：`numina agent run <task>`
/// 2. 交互式 REPL 模式：`numina agent` 或 `numina agent chat`
///    - 支持多轮对话，每轮都是独立的 agent 任务
///    - 支持 /help /tools /quit 等斜杠命令
///    - 支持历史记录（上下键）

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::io::Write;

use crate::core::agent::ActAgent;

// ─────────────────────────────────────────────
// ANSI 颜色
// ─────────────────────────────────────────────

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const MAGENTA: &str = "\x1b[35m";
const GRAY: &str = "\x1b[38;5;244m";
const DIM: &str = "\x1b[2m";

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
    /// 🚀 运行 ReAct Agent 完成一个任务（单次模式）
    Run {
        /// 任务描述（自然语言）
        task: String,

        /// 覆盖默认模型
        #[arg(short = 'o', long)]
        model: Option<String>,

        /// 工作目录（shell 工具的默认 cwd）
        #[arg(short = 'C', long)]
        cwd: Option<String>,

        /// 最大执行步数（默认 30）
        #[arg(long, default_value_t = 30)]
        max_steps: usize,

        /// 输出 JSON 格式的运行结果
        #[arg(long)]
        json: bool,

        /// 静默模式（不打印步骤详情，只输出最终答案）
        #[arg(short = 'q', long)]
        quiet: bool,

        /// 危险操作前询问用户确认
        #[arg(long)]
        confirm: bool,
    },

    /// 💬 交互式 Agent REPL（多轮任务模式）
    Chat {
        /// 覆盖默认模型
        #[arg(short = 'o', long)]
        model: Option<String>,

        /// 工作目录
        #[arg(short = 'C', long)]
        cwd: Option<String>,

        /// 最大每轮步数（默认 30）
        #[arg(long, default_value_t = 30)]
        max_steps: usize,

        /// 危险操作前询问用户确认
        #[arg(long)]
        confirm: bool,
    },

    /// 🔧 列出可用的内置工具
    Tools,

    /// 📋 列出所有 agent（占位，后续扩展）
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
            confirm,
        }) => {
            cmd_run(task, model.as_deref(), cwd.as_deref(), *max_steps, *json, *quiet, *confirm).await
        }
        Some(AgentCommands::Chat {
            model,
            cwd,
            max_steps,
            confirm,
        }) => {
            cmd_chat(model.as_deref(), cwd.as_deref(), *max_steps, *confirm).await
        }
        Some(AgentCommands::Tools) => cmd_tools(),
        Some(AgentCommands::List) => {
            println!("🤖 Agents are dynamically created per task.");
            println!("   Use `numina agent run <task>` to start an agent.");
            println!("   Use `numina agent chat` for interactive mode.");
            Ok(())
        }
        None => {
            // 默认进入交互式模式
            cmd_chat(None, None, 30, false).await
        }
    }
}

// ─────────────────────────────────────────────
// run 子命令（单次任务）
// ─────────────────────────────────────────────

async fn cmd_run(
    task: &str,
    model: Option<&str>,
    cwd: Option<&str>,
    max_steps: usize,
    output_json: bool,
    quiet: bool,
    confirm: bool,
) -> Result<()> {
    let agent = build_agent(max_steps, !quiet, confirm)?;
    let result = agent.run(task, model, cwd).await?;

    if output_json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if quiet {
        println!("{}", result.final_answer);
    } else {
        // verbose 模式已经在 run() 里打印了步骤
        println!();
        println!(
            "{}{}─────────────────────────────────────────{}",
            DIM, GRAY, RESET
        );
        println!(
            "{}📊 {} step(s)  {} tool call(s)  {:.1}s  success: {}{}",
            GRAY,
            result.total_steps,
            result.total_tool_calls,
            result.duration_ms as f64 / 1000.0,
            if result.success {
                format!("{}yes{}", GREEN, RESET)
            } else {
                format!("{}no{}", YELLOW, RESET)
            },
            GRAY
        );
        println!("{}{}{}", DIM, GRAY, RESET);
    }

    Ok(())
}

// ─────────────────────────────────────────────
// chat 子命令（交互式 REPL）
// ─────────────────────────────────────────────

async fn cmd_chat(
    model: Option<&str>,
    cwd: Option<&str>,
    max_steps: usize,
    confirm: bool,
) -> Result<()> {
    print_agent_welcome(model, cwd, max_steps);

    // 使用 rustyline 提供历史记录和行编辑
    let config = rustyline::Config::builder()
        .history_ignore_space(true)
        .completion_type(rustyline::CompletionType::List)
        .build();

    let mut rl = rustyline::DefaultEditor::with_config(config)?;

    // 加载历史记录
    let history_path = dirs::home_dir()
        .map(|h| h.join(".numina").join("agent_history"))
        .unwrap_or_else(|| std::path::PathBuf::from(".numina_agent_history"));

    if let Some(parent) = history_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = rl.load_history(&history_path);

    let mut session_count = 0u32;

    loop {
        // prompt 必须是纯文本（不含 ANSI 转义码），否则 rustyline 会把
        // ANSI 字节计入宽度，导致光标停在错误位置。
        // 颜色通过在 readline 之前打印来实现视觉效果。
        let prompt = "❯ ";

        match rl.readline(prompt) {
            Ok(line) => {
                let input = line.trim().to_string();
                if input.is_empty() {
                    continue;
                }

                // 添加到历史记录
                let _ = rl.add_history_entry(&input);

                // 处理斜杠命令
                if input.starts_with('/') {
                    match handle_slash_command(&input, model, cwd) {
                        SlashResult::Continue => continue,
                        SlashResult::Quit => break,
                        SlashResult::Unknown => {
                            println!("{}Unknown command: {}  Type /help for help.{}", YELLOW, input, RESET);
                            continue;
                        }
                    }
                }

                // 执行 agent 任务
                session_count += 1;
                println!(
                    "{}{}[Task #{}]{}",
                    DIM, GRAY, session_count, RESET
                );

                let agent = match build_agent(max_steps, true, confirm) {
                    Ok(a) => a,
                    Err(e) => {
                        eprintln!("{}⚠  Failed to initialize agent: {}{}", YELLOW, e, RESET);
                        continue;
                    }
                };

                match agent.run(&input, model, cwd).await {
                    Ok(result) => {
                        println!(
                            "\n{}{}─ {} step(s)  {} tool call(s)  {:.1}s{}",
                            DIM, GRAY,
                            result.total_steps,
                            result.total_tool_calls,
                            result.duration_ms as f64 / 1000.0,
                            RESET
                        );
                    }
                    Err(e) => {
                        eprintln!("\n{}❌ Agent error: {}{}", "\x1b[31m", e, RESET);
                    }
                }
            }

            Err(rustyline::error::ReadlineError::Interrupted) => {
                // Ctrl+C — 取消当前输入，继续
                println!("{}(Ctrl+C — type /quit to exit){}", GRAY, RESET);
                continue;
            }

            Err(rustyline::error::ReadlineError::Eof) => {
                // Ctrl+D — 退出
                println!("\n{}Goodbye!{}", GRAY, RESET);
                break;
            }

            Err(e) => {
                eprintln!("{}Readline error: {}{}", "\x1b[31m", e, RESET);
                break;
            }
        }
    }

    // 保存历史记录
    let _ = rl.save_history(&history_path);

    Ok(())
}

// ─────────────────────────────────────────────
// 斜杠命令处理
// ─────────────────────────────────────────────

enum SlashResult {
    Continue,
    Quit,
    Unknown,
}

fn handle_slash_command(input: &str, model: Option<&str>, cwd: Option<&str>) -> SlashResult {
    let parts: Vec<&str> = input.splitn(2, ' ').collect();
    match parts[0] {
        "/help" | "/h" => {
            print_help();
            SlashResult::Continue
        }
        "/quit" | "/exit" | "/q" => {
            println!("{}Goodbye!{}", GRAY, RESET);
            SlashResult::Quit
        }
        "/tools" => {
            let _ = cmd_tools();
            SlashResult::Continue
        }
        "/clear" => {
            print!("\x1b[2J\x1b[H");
            std::io::stdout().flush().ok();
            print_agent_welcome(model, cwd, 30);
            SlashResult::Continue
        }
        "/status" => {
            println!("{}Model: {}  CWD: {}{}",
                GRAY,
                model.unwrap_or("(default)"),
                cwd.unwrap_or("(current)"),
                RESET
            );
            SlashResult::Continue
        }
        _ => SlashResult::Unknown,
    }
}

// ─────────────────────────────────────────────
// tools 子命令
// ─────────────────────────────────────────────

fn cmd_tools() -> Result<()> {
    use crate::core::tools::builtin::default_registry;

    let registry = default_registry();
    let tools = registry.list_tools();

    println!("\n{}{}🔧 Available Tools ({} total):{}", BOLD, CYAN, tools.len(), RESET);
    println!();

    for name in &tools {
        if let Some(executor) = registry.get(name) {
            println!("  {}{}{}{}  {}{}{}", BOLD, GREEN, name, RESET, GRAY, executor.description(), RESET);
        }
    }
    println!();
    Ok(())
}

// ─────────────────────────────────────────────
// 辅助函数
// ─────────────────────────────────────────────

fn build_agent(max_steps: usize, verbose: bool, confirm: bool) -> Result<ActAgent> {
    ActAgent::new()
        .map(|a| a
            .with_max_steps(max_steps)
            .with_verbose(verbose)
            .with_confirm_dangerous(confirm)
        )
        .map_err(|e| {
            eprintln!("{}⚠  Failed to initialize agent: {}{}", YELLOW, e, RESET);
            eprintln!("   Run `numina config init` to set up your workspace.");
            e
        })
}

fn print_agent_welcome(model: Option<&str>, cwd: Option<&str>, max_steps: usize) {
    println!();
    println!("{}{}  Numina Agent  {}", BOLD, MAGENTA, RESET);
    println!("{}Autonomous AI coding agent · ReAct loop · Tool use{}", GRAY, RESET);
    println!();
    println!("  {}Model:{} {}",
        GRAY, RESET,
        model.unwrap_or("(configured default)")
    );
    println!("  {}CWD:{}   {}",
        GRAY, RESET,
        cwd.unwrap_or("(current directory)")
    );
    println!("  {}Steps:{} {} max per task", GRAY, RESET, max_steps);
    println!();

    // ReAct 能力说明
    println!("{}How it works (ReAct loop):{}", GRAY, RESET);
    println!("  {}1. 🤔 Reason{}  — Agent thinks about the task step by step", CYAN, RESET);
    println!("  {}2. 🔧 Act{}     — Agent calls a tool (shell, read_file, search…)", CYAN, RESET);
    println!("  {}3. 👁  Observe{} — Agent reads the tool result", CYAN, RESET);
    println!("  {}4. 🔁 Repeat{}  — Until task is complete (calls task_complete)", CYAN, RESET);
    println!();

    println!("{}Type a task in natural language, or use a slash command:{}", GRAY, RESET);
    println!("  {}/help{}    Show available commands", CYAN, RESET);
    println!("  {}/tools{}   List all available tools", CYAN, RESET);
    println!("  {}/quit{}    Exit", CYAN, RESET);
    println!();

    println!("{}Try these to test ReAct tool use:{}", GRAY, RESET);
    println!("  {}{}ls -la{}{}                          → runs shell, shows files", BOLD, "\x1b[93m", RESET, DIM);
    println!("  {}{}list all .rs files here{}{}         → uses find_files + list_dir", BOLD, "\x1b[93m", RESET, DIM);
    println!("  {}{}read Cargo.toml and summarize{}{}   → uses read_file", BOLD, "\x1b[93m", RESET, DIM);
    println!("  {}{}find all TODO in src/{}{}           → uses search_code", BOLD, "\x1b[93m", RESET, DIM);
    println!("  {}{}write hello.py with hello world{}{} → uses write_file + shell", BOLD, "\x1b[93m", RESET, DIM);
    println!("  {}{}what is my current directory?{}{}   → uses shell (pwd)", BOLD, "\x1b[93m", RESET, DIM);
    println!();
    println!("  {}💡 Tip: use `numina agent run \"<task>\"` for single-shot mode{}", DIM, RESET);
    println!();
}

fn print_help() {
    println!();
    println!("{}{}Numina Agent — Help{}", BOLD, CYAN, RESET);
    println!();
    println!("{}Slash Commands:{}", GRAY, RESET);
    println!("  {}/help{}     Show this help", CYAN, RESET);
    println!("  {}/tools{}    List available tools", CYAN, RESET);
    println!("  {}/status{}   Show current model and working directory", CYAN, RESET);
    println!("  {}/clear{}    Clear the screen", CYAN, RESET);
    println!("  {}/quit{}     Exit the agent", CYAN, RESET);
    println!();
    println!("{}Usage Tips:{}", GRAY, RESET);
    println!("  • Describe your task in natural language");
    println!("  • The agent will use tools to complete the task autonomously");
    println!("  • Use `numina agent run <task>` for single-shot mode");
    println!("  • Use `--confirm` flag to approve dangerous shell commands");
    println!("  • Use `--json` flag to get structured output");
    println!();
    println!("{}Available Tools:{}", GRAY, RESET);
    println!("  read_file, write_file, edit_file, list_dir");
    println!("  shell, search_code, find_files, http_get, task_complete");
    println!();
}
