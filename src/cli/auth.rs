/// auth 子命令 — 飞书登录授权
///
/// 用法：
///   numina auth login                          # 飞书用户授权（交互式）
///   numina auth login --scope "calendar:..."   # 按 scope 授权
///   numina auth login --domain calendar        # 按业务域授权
///   numina auth status                         # 查看当前授权状态
///   numina auth logout                         # 退出登录

use anyhow::Result;
use clap::{Args, Subcommand};
use std::process::Stdio;
use tokio::process::Command;

// ─────────────────────────────────────────────
// CLI 参数定义
// ─────────────────────────────────────────────

#[derive(Args)]
pub struct AuthArgs {
    #[command(subcommand)]
    pub command: Option<AuthCommands>,
}

#[derive(Subcommand)]
pub enum AuthCommands {
    /// 飞书用户登录授权（通过 lark-cli auth login）
    Login(LoginArgs),

    /// 查看当前飞书授权状态
    Status(StatusArgs),

    /// 退出飞书登录
    Logout(LogoutArgs),
}

#[derive(Args)]
pub struct LoginArgs {
    /// 按业务域授权（如 calendar、im、drive 等）
    #[arg(long, value_name = "DOMAIN")]
    pub domain: Option<String>,

    /// 按具体 scope 授权（推荐，符合最小权限原则）
    /// 多个 scope 用空格分隔，如 "calendar:calendar:readonly im:message:send_as_bot"
    #[arg(long, value_name = "SCOPE")]
    pub scope: Option<String>,

    /// 指定 lark-cli 可执行文件路径（默认使用 PATH 中的 lark-cli）
    #[arg(long, value_name = "PATH", default_value = "lark-cli")]
    pub cli_path: String,

    /// 额外传递给 lark-cli 的参数
    #[arg(last = true)]
    pub extra_args: Vec<String>,
}

#[derive(Args)]
pub struct StatusArgs {
    /// 指定 lark-cli 可执行文件路径（默认使用 PATH 中的 lark-cli）
    #[arg(long, value_name = "PATH", default_value = "lark-cli")]
    pub cli_path: String,
}

#[derive(Args)]
pub struct LogoutArgs {
    /// 指定 lark-cli 可执行文件路径（默认使用 PATH 中的 lark-cli）
    #[arg(long, value_name = "PATH", default_value = "lark-cli")]
    pub cli_path: String,
}

// ─────────────────────────────────────────────
// 执行入口
// ─────────────────────────────────────────────

pub async fn execute(args: &AuthArgs) -> Result<()> {
    match &args.command {
        Some(AuthCommands::Login(login_args)) => run_login(login_args).await,
        Some(AuthCommands::Status(status_args)) => run_status(status_args).await,
        Some(AuthCommands::Logout(logout_args)) => run_logout(logout_args).await,
        None => {
            println!("🔐 Numina 飞书授权管理");
            println!();
            println!("子命令：");
            println!("  login    飞书用户登录授权");
            println!("  status   查看当前授权状态");
            println!("  logout   退出飞书登录");
            println!();
            println!("示例：");
            println!("  numina auth login                              # 交互式授权");
            println!("  numina auth login --scope \"im:message\"         # 按 scope 授权");
            println!("  numina auth login --domain calendar            # 按业务域授权");
            println!("  numina auth status                             # 查看授权状态");
            Ok(())
        }
    }
}

/// 执行飞书登录授权
async fn run_login(args: &LoginArgs) -> Result<()> {
    // 检查 lark-cli 是否可用
    check_lark_cli(&args.cli_path).await?;

    // 构建命令参数
    let mut cmd_args: Vec<String> = vec!["auth".to_string(), "login".to_string()];

    if let Some(domain) = &args.domain {
        cmd_args.push("--domain".to_string());
        cmd_args.push(domain.clone());
        println!("🔐 飞书授权登录（业务域：{}）", domain);
    } else if let Some(scope) = &args.scope {
        cmd_args.push("--scope".to_string());
        cmd_args.push(scope.clone());
        println!("🔐 飞书授权登录（scope：{}）", scope);
    } else {
        println!("🔐 飞书授权登录");
    }

    // 追加额外参数
    cmd_args.extend(args.extra_args.clone());

    println!("   正在启动授权流程，授权链接将直接显示在下方...");
    println!("   请在浏览器中打开链接完成授权，完成后自动返回\n");

    // 关键：使用 inherit 让 lark-cli 直接操控终端（tty），
    // 它会自己向终端打印授权链接，无需我们捕获转发
    let status = Command::new(&args.cli_path)
        .args(&cmd_args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .map_err(|e| anyhow::anyhow!(
            "无法启动 lark-cli（{}）：{}\n请确认已安装：npm install -g @larksuite/cli",
            args.cli_path, e
        ))?;

    if status.success() || status.code().unwrap_or(-1) == 1 {
        println!("\n✅ 飞书授权成功！");
        println!("   现在可以使用 numina channel lark 启动飞书消息监听");
    } else {
        anyhow::bail!(
            "lark-cli auth login 退出码：{}，请检查配置或重试",
            status.code().unwrap_or(-1)
        );
    }

    Ok(())
}

/// 查看飞书授权状态
async fn run_status(args: &StatusArgs) -> Result<()> {
    check_lark_cli(&args.cli_path).await?;

    println!("🔍 查询飞书授权状态...\n");

    // 尝试用 lark-cli 查询当前用户信息来验证授权
    let output = Command::new(&args.cli_path)
        .args(["contact", "user", "me", "--as", "user"])
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("执行 lark-cli 失败：{}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        // 尝试解析 JSON 输出中的用户信息
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
            let name = json.pointer("/data/name")
                .or_else(|| json.pointer("/name"))
                .and_then(|v| v.as_str())
                .unwrap_or("未知");
            let email = json.pointer("/data/email")
                .or_else(|| json.pointer("/email"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            println!("✅ 飞书授权状态：已登录");
            println!("   用户名：{}", name);
            if !email.is_empty() {
                println!("   邮箱：{}", email);
            }
        } else {
            println!("✅ 飞书授权状态：已登录");
            if !stdout.trim().is_empty() {
                println!("   {}", stdout.trim());
            }
        }
    } else {
        // 检查是否是未授权错误
        let combined = format!("{}{}", stdout, stderr);
        if combined.contains("unauthorized") || combined.contains("token") || combined.contains("login") || combined.contains("auth") {
            println!("❌ 飞书授权状态：未登录或授权已过期");
            println!();
            println!("请运行以下命令完成授权：");
            println!("  numina auth login");
        } else {
            println!("⚠️  无法确认授权状态");
            if !combined.trim().is_empty() {
                println!("   详情：{}", combined.trim());
            }
            println!();
            println!("如需重新授权，请运行：");
            println!("  numina auth login");
        }
    }

    Ok(())
}

/// 退出飞书登录
async fn run_logout(args: &LogoutArgs) -> Result<()> {
    check_lark_cli(&args.cli_path).await?;

    println!("🚪 正在退出飞书登录...");

    let output = Command::new(&args.cli_path)
        .args(["auth", "logout"])
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("执行 lark-cli 失败：{}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        println!("✅ 已退出飞书登录");
        if !stdout.trim().is_empty() {
            println!("   {}", stdout.trim());
        }
    } else {
        let combined = format!("{}{}", stdout, stderr);
        if !combined.trim().is_empty() {
            println!("⚠️  {}", combined.trim());
        } else {
            println!("⚠️  退出登录时遇到问题，请手动清理 lark-cli 凭证");
        }
    }

    Ok(())
}

/// 检查 lark-cli 是否已安装并可用
async fn check_lark_cli(cli_path: &str) -> Result<()> {
    let output = Command::new(cli_path)
        .arg("--version")
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let version = String::from_utf8_lossy(&out.stdout);
            let version = version.trim();
            if !version.is_empty() {
                println!("   lark-cli 版本：{}", version);
            }
            Ok(())
        }
        Ok(_) => Ok(()), // 有输出但非零退出码，仍然可用
        Err(e) => {
            anyhow::bail!(
                "未找到 lark-cli（{}）：{}\n\n请先安装 lark-cli：\n  npm install -g @larksuite/cli\n\n安装后可运行：\n  numina auth login",
                cli_path, e
            )
        }
    }
}
