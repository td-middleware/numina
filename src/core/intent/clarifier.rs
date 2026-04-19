/// 意图澄清器 trait 及内置实现
///
/// IntentClarifier 定义了向用户展示意图选项并等待选择的接口。
/// 不同上下文有不同的实现：
///   - TuiIntentClarifier：crossterm 交互式选择（TUI/CLI 用）
///   - LarkIntentClarifier：飞书消息选择（在 src/channel/lark/ 中实现）

use async_trait::async_trait;
use crate::core::intent::analyzer::IntentOption;

// ─────────────────────────────────────────────
// IntentClarifier trait
// ─────────────────────────────────────────────

/// 意图澄清器 trait
///
/// 向用户展示候选意图选项，等待用户选择，返回选中的选项。
/// 返回 None 表示用户取消。
#[async_trait]
pub trait IntentClarifier: Send + Sync {
    async fn clarify(
        &self,
        original_message: &str,
        options: Vec<IntentOption>,
    ) -> Option<IntentOption>;
}

// ─────────────────────────────────────────────
// TuiIntentClarifier — crossterm 交互式选择
// ─────────────────────────────────────────────

/// TUI 意图澄清器
///
/// 使用 crossterm 在终端中渲染交互式选择框，
/// 支持 ↑↓ 导航、Enter 确认、Esc 取消。
///
/// 外观示例：
/// ```text
///   ╭─ 🤔 意图澄清 ──────────────────────────────────
///   │
///   │  你的消息：「帮我处理一下代码」
///   │  我理解你的意思可能是以下之一：
///   │
///   ├────────────────────────────────────────────────
///   │
///   │  ▶ 1. 代码审查
///   │       检查代码潜在问题
///   │     2. 代码格式化
///   │       统一代码风格
///   │     3. 添加注释
///   │       为代码添加注释
///   │
///   │  ↑↓ 导航 · Enter 确认 · Esc 取消
///   ╰────────────────────────────────────────────────
/// ```
pub struct TuiIntentClarifier;

impl TuiIntentClarifier {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TuiIntentClarifier {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl IntentClarifier for TuiIntentClarifier {
    async fn clarify(
        &self,
        original_message: &str,
        options: Vec<IntentOption>,
    ) -> Option<IntentOption> {
        if options.is_empty() {
            return None;
        }

        // crossterm 操作需要在同步上下文中执行
        let original_message = original_message.to_string();
        let options_clone = options.clone();

        let result = tokio::task::spawn_blocking(move || {
            tui_select_intent(&original_message, &options_clone)
        })
        .await
        .unwrap_or(None);

        result.map(|idx| options[idx].clone())
    }
}

/// crossterm 交互式意图选择（同步，在 spawn_blocking 中运行）
///
/// 返回选中的选项索引（0-based），None 表示取消
fn tui_select_intent(original_message: &str, options: &[IntentOption]) -> Option<usize> {
    use crossterm::cursor::{MoveToColumn, MoveUp};
    use crossterm::event::{read as ev_read, Event, KeyCode, KeyEvent, KeyModifiers};
    use crossterm::execute;
    use crossterm::style::Print;
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType};
    use std::io::{stdout, Write};

    let num_options = options.len();
    let mut selected: usize = 0;
    let mut out = stdout();

    let build_lines = |sel: usize| -> Vec<String> {
        let mut lines = Vec::new();

        // 顶部边框
        lines.push(format!(
            "  \x1b[35m╭─ 🤔 意图澄清 ──────────────────────────────────\x1b[0m"
        ));
        lines.push(format!("  \x1b[35m│\x1b[0m"));

        // 原始消息
        let msg_preview: String = original_message.chars().take(50).collect();
        let msg_ellipsis = if original_message.len() > 50 { "…" } else { "" };
        lines.push(format!(
            "  \x1b[35m│\x1b[0m  你的消息：\x1b[2m「{}{}」\x1b[0m",
            msg_preview, msg_ellipsis
        ));
        lines.push(format!(
            "  \x1b[35m│\x1b[0m  我理解你的意思可能是以下之一，请选择："
        ));
        lines.push(format!("  \x1b[35m│\x1b[0m"));
        lines.push(format!(
            "  \x1b[35m├────────────────────────────────────────────────\x1b[0m"
        ));
        lines.push(format!("  \x1b[35m│\x1b[0m"));

        // 选项列表
        for (i, opt) in options.iter().enumerate() {
            if i == sel {
                // 高亮选中项
                lines.push(format!(
                    "  \x1b[35m│\x1b[0m  \x1b[48;5;53m\x1b[97m ▶ {}. {:<40}\x1b[0m",
                    opt.index, opt.title
                ));
                if !opt.description.is_empty() {
                    lines.push(format!(
                        "  \x1b[35m│\x1b[0m  \x1b[48;5;53m\x1b[2m     {:<42}\x1b[0m",
                        opt.description
                    ));
                }
            } else {
                lines.push(format!(
                    "  \x1b[35m│\x1b[0m     \x1b[1m{}. {}\x1b[0m",
                    opt.index, opt.title
                ));
                if !opt.description.is_empty() {
                    lines.push(format!(
                        "  \x1b[35m│\x1b[0m       \x1b[2m{}\x1b[0m",
                        opt.description
                    ));
                }
            }
        }

        lines.push(format!("  \x1b[35m│\x1b[0m"));
        lines.push(format!(
            "  \x1b[35m│\x1b[0m  \x1b[2m↑↓ 导航 · Enter 确认 · Esc 取消\x1b[0m"
        ));
        lines.push(format!(
            "  \x1b[35m╰────────────────────────────────────────────────\x1b[0m"
        ));
        lines
    };

    let _ = enable_raw_mode();

    let initial_lines = build_lines(selected);
    let total_lines = initial_lines.len() as u16;
    for line in &initial_lines {
        let _ = execute!(out, Print(format!("{}\r\n", line)));
    }
    let _ = out.flush();

    let choice = loop {
        match ev_read() {
            Ok(Event::Key(KeyEvent {
                code: KeyCode::Up,
                modifiers: KeyModifiers::NONE,
                ..
            })) => {
                selected = if selected == 0 {
                    num_options - 1
                } else {
                    selected - 1
                };
                let _ = execute!(out, MoveUp(total_lines), MoveToColumn(0));
                let new_lines = build_lines(selected);
                for line in &new_lines {
                    let _ = execute!(
                        out,
                        Clear(ClearType::CurrentLine),
                        Print(format!("{}\r\n", line))
                    );
                }
                let _ = out.flush();
            }
            Ok(Event::Key(KeyEvent {
                code: KeyCode::Down,
                modifiers: KeyModifiers::NONE,
                ..
            })) => {
                selected = (selected + 1) % num_options;
                let _ = execute!(out, MoveUp(total_lines), MoveToColumn(0));
                let new_lines = build_lines(selected);
                for line in &new_lines {
                    let _ = execute!(
                        out,
                        Clear(ClearType::CurrentLine),
                        Print(format!("{}\r\n", line))
                    );
                }
                let _ = out.flush();
            }
            Ok(Event::Key(KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                ..
            })) => {
                break Some(selected);
            }
            // 数字快捷键 1-9
            Ok(Event::Key(KeyEvent {
                code: KeyCode::Char(c),
                ..
            })) if c.is_ascii_digit() && c != '0' => {
                let n = (c as usize) - ('1' as usize);
                if n < num_options {
                    break Some(n);
                }
            }
            Ok(Event::Key(KeyEvent {
                code: KeyCode::Esc, ..
            }))
            | Ok(Event::Key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::CONTROL,
                ..
            })) => {
                break None;
            }
            _ => {}
        }
    };

    let _ = disable_raw_mode();

    // 显示选择结果
    match choice {
        Some(idx) => {
            println!(
                "  \x1b[35m✓\x1b[0m 已选择：\x1b[1m{}\x1b[0m",
                options[idx].title
            );
        }
        None => {
            println!("  \x1b[2m✗ 已取消\x1b[0m");
        }
    }
    let _ = out.flush();

    choice
}

// ─────────────────────────────────────────────
// PassthroughClarifier — 直接返回第一个选项（测试用）
// ─────────────────────────────────────────────

/// 直通澄清器（测试/非交互场景用）
///
/// 直接返回第一个选项，不做任何交互。
pub struct PassthroughClarifier;

#[async_trait]
impl IntentClarifier for PassthroughClarifier {
    async fn clarify(
        &self,
        _original_message: &str,
        options: Vec<IntentOption>,
    ) -> Option<IntentOption> {
        options.into_iter().next()
    }
}

// ─────────────────────────────────────────────
// 单元测试
// ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_passthrough_clarifier() {
        let clarifier = PassthroughClarifier;
        let options = vec![
            IntentOption {
                index: 1,
                title: "选项A".to_string(),
                description: "描述A".to_string(),
                refined_prompt: "执行A".to_string(),
            },
            IntentOption {
                index: 2,
                title: "选项B".to_string(),
                description: "描述B".to_string(),
                refined_prompt: "执行B".to_string(),
            },
        ];
        let result = clarifier.clarify("测试消息", options).await;
        assert!(result.is_some());
        assert_eq!(result.unwrap().title, "选项A");
    }

    #[tokio::test]
    async fn test_passthrough_clarifier_empty() {
        let clarifier = PassthroughClarifier;
        let result = clarifier.clarify("测试消息", vec![]).await;
        assert!(result.is_none());
    }
}
