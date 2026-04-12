// ─────────────────────────────────────────────
// 权限确认对话框
// ─────────────────────────────────────────────

/// MCP 工具调用权限确认对话框（Claude Code 风格）
/// 显示 MCP server 名称、工具名称、参数 JSON
/// 返回 1 = Allow once, 2 = Allow for session, 3 = Deny
pub fn read_permission_choice_mcp(server_name: &str, tool_name: &str, args_json: &str) -> u8 {
    use crossterm::cursor::{MoveToColumn, MoveUp};
    use crossterm::event::{read as ev_read, Event, KeyCode, KeyEvent, KeyModifiers};
    use crossterm::style::Print;
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType};
    use crossterm::execute;
    use std::io::{stdout, Write};

    const OPTIONS: &[&str] = &[
        "Allow once",
        "Allow for this session",
        "Deny",
    ];
    const NUM_OPTIONS: usize = 3;

    let mut selected: usize = 0;
    let mut out = stdout();

    // 格式化 JSON 参数（最多显示 20 行）
    let args_lines: Vec<String> = {
        const MAX_ARG_LINES: usize = 20;
        const MAX_ARG_WIDTH: usize = 200;
        let raw_lines: Vec<&str> = args_json.lines().collect();
        let show = raw_lines.len().min(MAX_ARG_LINES);
        let mut result: Vec<String> = raw_lines[..show]
            .iter()
            .map(|l| {
                let trimmed = l.trim_end();
                let display: String = trimmed.chars().take(MAX_ARG_WIDTH).collect();
                let ellipsis = if trimmed.len() > MAX_ARG_WIDTH { "…" } else { "" };
                format!("{}{}", display, ellipsis)
            })
            .collect();
        if raw_lines.len() > MAX_ARG_LINES {
            result.push(format!("… {} more lines", raw_lines.len() - MAX_ARG_LINES));
        }
        result
    };

    let build_lines = |sel: usize| -> Vec<String> {
        let mut lines = Vec::new();
        // 顶部边框
        lines.push(format!("  \x1b[34m╭─ 🔧 MCP Tool Use ──────────────────────────────────\x1b[0m"));
        lines.push(format!("  \x1b[34m│\x1b[0m"));
        // MCP server 名称
        lines.push(format!("  \x1b[34m│\x1b[0m  MCP:  \x1b[1m\x1b[36m{}\x1b[0m", server_name));
        // 工具名称
        lines.push(format!("  \x1b[34m│\x1b[0m  Tool: \x1b[1m{}\x1b[0m", tool_name));
        // 参数 JSON
        if !args_json.is_empty() && args_json != "{}" {
            lines.push(format!("  \x1b[34m│\x1b[0m  Args:"));
            for arg_line in &args_lines {
                lines.push(format!("  \x1b[34m│\x1b[0m    \x1b[48;5;235m\x1b[37m  {:<80}\x1b[0m", arg_line));
            }
        }
        lines.push(format!("  \x1b[34m│\x1b[0m"));
        lines.push(format!("  \x1b[34m├────────────────────────────────────────────────────\x1b[0m"));
        lines.push(format!("  \x1b[34m│\x1b[0m"));
        // 选项
        for (i, label) in OPTIONS.iter().enumerate() {
            if i == sel {
                lines.push(format!(
                    "  \x1b[34m│\x1b[0m  \x1b[48;5;24m\x1b[97m ▶ {:<40}\x1b[0m",
                    label
                ));
            } else {
                lines.push(format!(
                    "  \x1b[34m│\x1b[0m    \x1b[2m{}\x1b[0m",
                    label
                ));
            }
        }
        lines.push(format!("  \x1b[34m│\x1b[0m"));
        lines.push(format!("  \x1b[34m│\x1b[0m  \x1b[2m↑↓ navigate · Enter confirm · Esc deny\x1b[0m"));
        lines.push(format!("  \x1b[34m╰────────────────────────────────────────────────────\x1b[0m"));
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
            Ok(Event::Key(KeyEvent { code: KeyCode::Up, modifiers: KeyModifiers::NONE, .. })) => {
                selected = if selected == 0 { NUM_OPTIONS - 1 } else { selected - 1 };
                let _ = execute!(out, MoveUp(total_lines), MoveToColumn(0));
                let new_lines = build_lines(selected);
                for line in &new_lines {
                    let _ = execute!(out,
                        Clear(ClearType::CurrentLine),
                        Print(format!("{}\r\n", line))
                    );
                }
                let _ = out.flush();
            }
            Ok(Event::Key(KeyEvent { code: KeyCode::Down, modifiers: KeyModifiers::NONE, .. })) => {
                selected = (selected + 1) % NUM_OPTIONS;
                let _ = execute!(out, MoveUp(total_lines), MoveToColumn(0));
                let new_lines = build_lines(selected);
                for line in &new_lines {
                    let _ = execute!(out,
                        Clear(ClearType::CurrentLine),
                        Print(format!("{}\r\n", line))
                    );
                }
                let _ = out.flush();
            }
            Ok(Event::Key(KeyEvent { code: KeyCode::Enter, modifiers: KeyModifiers::NONE, .. })) => {
                break (selected + 1) as u8;
            }
            Ok(Event::Key(KeyEvent { code: KeyCode::Char('1'), .. })) => break 1,
            Ok(Event::Key(KeyEvent { code: KeyCode::Char('2'), .. })) => break 2,
            Ok(Event::Key(KeyEvent { code: KeyCode::Char('3'), .. })) => break 3,
            Ok(Event::Key(KeyEvent { code: KeyCode::Char('y'), .. }))
            | Ok(Event::Key(KeyEvent { code: KeyCode::Char('Y'), .. })) => break 1,
            Ok(Event::Key(KeyEvent { code: KeyCode::Char('n'), .. }))
            | Ok(Event::Key(KeyEvent { code: KeyCode::Char('N'), .. })) => break 3,
            Ok(Event::Key(KeyEvent { code: KeyCode::Esc, .. }))
            | Ok(Event::Key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::CONTROL,
                ..
            })) => break 0, // 0 = 强制中止（Esc/Ctrl+C），区别于 3=Deny
            _ => {}
        }
    };

    let _ = disable_raw_mode();

    let result_label = match choice {
        1 => "\x1b[32mAllowed once\x1b[0m",
        2 => "\x1b[32mAllowed for session\x1b[0m",
        0 => "\x1b[31mAborted\x1b[0m",
        _ => "\x1b[33mDenied\x1b[0m",
    };
    println!("  {}", result_label);

    choice
}


/// 读取权限确认选择（crossterm raw mode，返回 1/2/3）
/// 1 = Yes once, 2 = Yes for session, 3 = No
pub fn read_permission_choice() -> u8 {
    use crossterm::event::{read as ev_read, Event, KeyCode, KeyEvent, KeyModifiers};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

    let _ = enable_raw_mode();
    let choice = loop {
        match ev_read() {
            Ok(Event::Key(KeyEvent { code: KeyCode::Char('1'), .. })) => break 1,
            Ok(Event::Key(KeyEvent { code: KeyCode::Char('2'), .. })) => break 2,
            Ok(Event::Key(KeyEvent { code: KeyCode::Char('3'), .. })) => break 3,
            Ok(Event::Key(KeyEvent { code: KeyCode::Char('n'), .. }))
            | Ok(Event::Key(KeyEvent { code: KeyCode::Char('N'), .. })) => break 3,
            Ok(Event::Key(KeyEvent { code: KeyCode::Char('y'), .. }))
            | Ok(Event::Key(KeyEvent { code: KeyCode::Char('Y'), .. })) => break 1,
            Ok(Event::Key(KeyEvent { code: KeyCode::Enter, .. })) => break 1,
            Ok(Event::Key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::CONTROL,
                ..
            })) => break 3,
            Ok(Event::Key(KeyEvent { code: KeyCode::Esc, .. })) => break 3,
            _ => continue,
        }
    };
    let _ = disable_raw_mode();
    choice
}

/// 交互式权限确认选择器（上下键选择 + 颜色高亮）
/// 渲染完整的权限确认对话框，支持 ↑↓ 导航，Enter 确认
/// 返回 1 = Yes once, 2 = Yes for session, 3 = No
pub fn read_permission_choice_interactive(tool_name: &str, cmd: &str) -> u8 {
    use crossterm::cursor::{MoveToColumn, MoveUp};
    use crossterm::event::{read as ev_read, Event, KeyCode, KeyEvent, KeyModifiers};
    use crossterm::style::Print;
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType};
    use crossterm::execute;
    use std::io::{stdout, Write};

    const OPTIONS: &[&str] = &[
        "Yes, run once",
        "Yes, allow for this session",
        "No, skip this operation",
    ];
    const NUM_OPTIONS: usize = 3;

    let mut selected: usize = 0;
    let mut out = stdout();

    let build_lines = |sel: usize| -> Vec<String> {
        let mut lines = Vec::new();
        lines.push(format!("  \x1b[33m╭─ Permission Required ─────────────────────────────\x1b[0m"));
        lines.push(format!("  \x1b[33m│\x1b[0m Tool: \x1b[1m{}\x1b[0m", tool_name));
        if !cmd.is_empty() {
            // 多行命令：每行都加 │  前缀，靠左对齐，超长行截断
            const MAX_CMD_WIDTH: usize = 60;
            const MAX_CMD_LINES: usize = 12;
            let cmd_lines: Vec<&str> = cmd.lines().collect();
            let show_count = cmd_lines.len().min(MAX_CMD_LINES);
            lines.push(format!("  \x1b[33m│\x1b[0m Command:"));
            for (i, cmd_line) in cmd_lines[..show_count].iter().enumerate() {
                let trimmed = cmd_line.trim_end();
                let display: String = trimmed.chars().take(MAX_CMD_WIDTH).collect();
                let ellipsis = if trimmed.len() > MAX_CMD_WIDTH { "…" } else { "" };
                lines.push(format!("  \x1b[33m│\x1b[0m   \x1b[2m{}{}\x1b[0m", display, ellipsis));
                let _ = i;
            }
            if cmd_lines.len() > MAX_CMD_LINES {
                lines.push(format!("  \x1b[33m│\x1b[0m   \x1b[2m… {} more lines\x1b[0m", cmd_lines.len() - MAX_CMD_LINES));
            }
        }
        lines.push(format!("  \x1b[33m│\x1b[0m"));
        lines.push(format!("  \x1b[33m│\x1b[0m Do you want to proceed?"));
        lines.push(format!("  \x1b[33m│\x1b[0m"));
        for (i, label) in OPTIONS.iter().enumerate() {
            if i == sel {
                lines.push(format!(
                    "  \x1b[33m│\x1b[0m  \x1b[48;5;24m\x1b[97m ▶ {:<38}\x1b[0m",
                    label
                ));
            } else {
                lines.push(format!(
                    "  \x1b[33m│\x1b[0m    \x1b[2m{}\x1b[0m",
                    label
                ));
            }
        }
        lines.push(format!("  \x1b[33m│\x1b[0m"));
        lines.push(format!("  \x1b[33m│\x1b[0m  \x1b[2m↑↓ navigate · Enter confirm · Esc cancel\x1b[0m"));
        lines.push(format!("  \x1b[33m╰────────────────────────────────────────────────────\x1b[0m"));
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
            Ok(Event::Key(KeyEvent { code: KeyCode::Up, modifiers: KeyModifiers::NONE, .. })) => {
                selected = if selected == 0 { NUM_OPTIONS - 1 } else { selected - 1 };
                let _ = execute!(out, MoveUp(total_lines), MoveToColumn(0));
                let new_lines = build_lines(selected);
                for line in &new_lines {
                    let _ = execute!(out,
                        Clear(ClearType::CurrentLine),
                        Print(format!("{}\r\n", line))
                    );
                }
                let _ = out.flush();
            }
            Ok(Event::Key(KeyEvent { code: KeyCode::Down, modifiers: KeyModifiers::NONE, .. })) => {
                selected = (selected + 1) % NUM_OPTIONS;
                let _ = execute!(out, MoveUp(total_lines), MoveToColumn(0));
                let new_lines = build_lines(selected);
                for line in &new_lines {
                    let _ = execute!(out,
                        Clear(ClearType::CurrentLine),
                        Print(format!("{}\r\n", line))
                    );
                }
                let _ = out.flush();
            }
            Ok(Event::Key(KeyEvent { code: KeyCode::Enter, modifiers: KeyModifiers::NONE, .. })) => {
                break (selected + 1) as u8;
            }
            Ok(Event::Key(KeyEvent { code: KeyCode::Char('1'), .. })) => break 1,
            Ok(Event::Key(KeyEvent { code: KeyCode::Char('2'), .. })) => break 2,
            Ok(Event::Key(KeyEvent { code: KeyCode::Char('3'), .. })) => break 3,
            Ok(Event::Key(KeyEvent { code: KeyCode::Char('y'), .. }))
            | Ok(Event::Key(KeyEvent { code: KeyCode::Char('Y'), .. })) => break 1,
            Ok(Event::Key(KeyEvent { code: KeyCode::Char('n'), .. }))
            | Ok(Event::Key(KeyEvent { code: KeyCode::Char('N'), .. })) => break 3,
            Ok(Event::Key(KeyEvent { code: KeyCode::Esc, .. }))
            | Ok(Event::Key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::CONTROL,
                ..
            })) => break 0, // 0 = 强制中止（Esc/Ctrl+C），区别于 3=Deny
            _ => {}
        }
    };

    let _ = disable_raw_mode();

    let result_label = match choice {
        1 => "\x1b[32mYes, run once\x1b[0m",
        2 => "\x1b[32mYes, allow for session\x1b[0m",
        0 => "\x1b[31mAborted\x1b[0m",
        _ => "\x1b[33mNo, skipped\x1b[0m",
    };
    println!("  Selected: {}", result_label);

    choice
}
