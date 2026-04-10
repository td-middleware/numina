// ─────────────────────────────────────────────
// 权限确认对话框
// ─────────────────────────────────────────────

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
            })) => break 3,
            _ => {}
        }
    };

    let _ = disable_raw_mode();

    let result_label = match choice {
        1 => "\x1b[32mYes, run once\x1b[0m",
        2 => "\x1b[32mYes, allow for session\x1b[0m",
        _ => "\x1b[33mNo, skipped\x1b[0m",
    };
    println!("  Selected: {}", result_label);

    choice
}
