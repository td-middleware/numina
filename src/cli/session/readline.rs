use std::io::Write;

use rustyline::completion::Pair;

use super::completer::ChatCompleter;

// ─────────────────────────────────────────────
// crossterm 实时交互式 readline
// ─────────────────────────────────────────────

const MAX_COMPLETION_DISPLAY: usize = 10;

pub enum ReadLine {
    Line(String),
    Interrupted,
    Eof,
}

/// 计算字符串中可见字符列数（跳过 ANSI 转义码，正确处理 Unicode 宽字符）
fn visible_columns(s: &str) -> usize {
    let mut cols = 0usize;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for inner in chars.by_ref() {
                    if inner.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                chars.next();
            }
        } else {
            let w = unicode_char_width(c);
            cols += w;
        }
    }
    cols
}

/// 返回单个 Unicode 字符的终端显示宽度（0、1 或 2）
fn unicode_char_width(c: char) -> usize {
    if c < ' ' || c == '\x7f' {
        return 0;
    }
    let cp = c as u32;
    if matches!(cp,
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33BF
        | 0x33FF..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7AF
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE1F
        | 0xFE30..=0xFE4F
        | 0xFF01..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1B000..=0x1B0FF
        | 0x1F004..=0x1F0CF
        | 0x1F200..=0x1F2FF
        | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x2FFFD
        | 0x30000..=0x3FFFD
    ) {
        2
    } else {
        1
    }
}

/// 计算 chars[..n] 的实际终端显示列数（中文等宽字符占2列）
fn chars_display_cols(chars: &[char], n: usize) -> usize {
    chars[..n].iter().map(|&c| unicode_char_width(c)).sum()
}

/// 将候选项应用到输入行（chars[offset..cursor] 替换为 replacement）
fn apply_candidate(chars: &mut Vec<char>, cursor: &mut usize, candidate: &Pair, offset: usize) {
    let rep: Vec<char> = candidate.replacement.chars().collect();
    chars.drain(offset..*cursor);
    for (i, &c) in rep.iter().enumerate() {
        chars.insert(offset + i, c);
    }
    *cursor = offset + rep.len();
}

/// 根据当前输入更新候选列表
fn update_completion(
    chars: &[char],
    cursor: usize,
    candidates: &mut Vec<Pair>,
    selected: &mut Option<usize>,
    offset: &mut usize,
) {
    use super::completer::SLASH_COMMANDS;

    let word: String = chars[..cursor].iter().collect();

    if let Some(at_pos) = word.rfind('@') {
        let path_part = &word[at_pos + 1..];
        let cands = ChatCompleter::complete_path(path_part);
        if !cands.is_empty() {
            *candidates = cands;
            *selected = None;
            *offset = word[..at_pos + 1].chars().count();
            return;
        }
    }

    if word.starts_with('/') {
        let cmd_matches: Vec<Pair> = SLASH_COMMANDS
            .iter()
            .filter(|(cmd, _)| cmd.starts_with(word.as_str()))
            .map(|(cmd, desc)| Pair {
                display: format!("{:<14} {}", cmd, desc),
                replacement: cmd.to_string(),
            })
            .collect();
        if !cmd_matches.is_empty() {
            *candidates = cmd_matches;
            *selected = None;
            *offset = 0;
            return;
        }
        let file_cands = ChatCompleter::complete_path(&word);
        if !file_cands.is_empty() {
            *candidates = file_cands;
            *selected = None;
            *offset = 0;
            return;
        }
    }

    candidates.clear();
    *selected = None;
    *offset = 0;
}

/// 渲染单个候选项（带 ANSI 颜色和选中高亮）
fn render_one_candidate(c: &Pair, selected: bool) -> String {
    let bg = if selected { "\x1b[48;5;24m" } else { "\x1b[48;5;238m" };
    let indicator = if selected { "\x1b[97m▶\x1b[0m" } else { " " };
    if c.display.ends_with('/') {
        format!("{} {}  \x1b[38;5;117m{}\x1b[0m", bg, indicator, c.display)
    } else {
        let trimmed = c.display.trim_end();
        if let Some(sep) = trimmed.find("   ") {
            let cmd_part = trimmed[..sep].trim();
            let desc_part = trimmed[sep..].trim();
            format!(
                "{} {}  \x1b[97m\x1b[1m{:<14}\x1b[0m{}\x1b[38;5;244m {}\x1b[0m",
                bg, indicator, cmd_part, bg, desc_part
            )
        } else {
            format!("{} {}  \x1b[38;5;255m{}\x1b[0m", bg, indicator, c.display)
        }
    }
}

/// 判断输入是否为"大输入"（多行或超长），返回缩略显示字符串
/// 如果是大输入，返回 Some(缩略文本)；否则返回 None（正常显示）
fn large_input_summary(chars: &[char]) -> Option<String> {
    // 超过 120 字符或包含换行符视为大输入
    const LARGE_THRESHOLD: usize = 120;
    let has_newline = chars.iter().any(|&c| c == '\n');
    if chars.len() <= LARGE_THRESHOLD && !has_newline {
        return None;
    }
    let line_count = chars.iter().filter(|&&c| c == '\n').count() + 1;
    let char_count = chars.len();
    if line_count > 1 {
        Some(format!(
            "\x1b[48;5;238m\x1b[38;5;117m [Pasted text +{} lines, {} chars] \x1b[0m",
            line_count, char_count
        ))
    } else {
        Some(format!(
            "\x1b[48;5;238m\x1b[38;5;117m [Long input: {} chars] \x1b[0m",
            char_count
        ))
    }
}

/// 重绘输入行 + 候选列表（在输入行下方）
/// 长文本/多行文本自动显示缩略信息，避免终端折行混乱
fn redraw_input_line(
    out: &mut std::io::Stdout,
    prompt: &str,
    chars: &[char],
    cursor: usize,
    candidates: &[Pair],
    selected: Option<usize>,
) -> std::io::Result<()> {
    use crossterm::cursor::MoveToColumn;
    use crossterm::cursor::MoveUp;
    use crossterm::style::Print;
    use crossterm::terminal::{Clear, ClearType, size as term_size};
    use crossterm::execute;

    let display_count = candidates.len().min(MAX_COMPLETION_DISPLAY);
    let extra = usize::from(candidates.len() > display_count);
    let cand_lines = if candidates.is_empty() { 0 } else { display_count + 2 + extra };

    execute!(out, MoveToColumn(0), Clear(ClearType::FromCursorDown))?;

    execute!(out, Print(prompt))?;

    // 大输入模式：显示缩略信息而非完整内容
    if let Some(summary) = large_input_summary(chars) {
        execute!(out, Print(&summary))?;
        // 光标固定在缩略信息末尾（不计算实际位置）
        let col = (visible_columns(prompt) + 1) as u16;
        execute!(out, MoveToColumn(col))?;
    } else {
        let input_str: String = chars.iter().collect();
        execute!(out, Print(&input_str))?;

        if !candidates.is_empty() {
            let term_w = term_size().map(|(w, _)| w as usize).unwrap_or(80);
            let sep = "─".repeat(term_w.min(54));
            execute!(out, Print(format!("\r\n\x1b[38;5;244m{}\x1b[0m", sep)))?;
            for i in 0..display_count {
                execute!(out, Print(format!(
                    "\r\n{}",
                    render_one_candidate(&candidates[i], Some(i) == selected)
                )))?;
            }
            if extra > 0 {
                execute!(out, Print(format!(
                    "\r\n\x1b[48;5;238m\x1b[38;5;244m    … {} more results\x1b[0m",
                    candidates.len() - display_count
                )))?;
            }
            execute!(out, Print(format!("\r\n\x1b[38;5;244m{}\x1b[0m", sep)))?;
            execute!(out, MoveUp(cand_lines as u16))?;
        }

        let col = (visible_columns(prompt) + chars_display_cols(chars, cursor)) as u16;
        execute!(out, MoveToColumn(col))?;
    }

    out.flush()?;
    Ok(())
}

/// 在终端底部渲染竖向交互式候选列表（供 completer 的 Tab 键处理器使用）
pub fn show_interactive_list(candidates: &[Pair]) -> std::io::Result<Option<String>> {
    use crossterm::cursor::{MoveTo, position as cursor_pos};
    use crossterm::event::{read as ev_read, Event, KeyCode, KeyEvent, KeyModifiers};
    use crossterm::style::Print;
    use crossterm::terminal::{Clear, ClearType, size as term_size};
    use crossterm::execute;
    use std::io::{stdout, Write};

    let (term_w, term_h) = term_size()?;
    let display_count = candidates.len().min(MAX_COMPLETION_DISPLAY);
    let extra_line = usize::from(candidates.len() > display_count);
    let list_height = (display_count + 2 + extra_line) as u16;
    let list_start = term_h.saturating_sub(list_height + 1);
    let mut selected = 0usize;
    let mut out = stdout();
    let sep = "─".repeat((term_w as usize).min(54));

    let (orig_col, orig_row) = cursor_pos()?;

    let draw = |out: &mut std::io::Stdout, sel: usize| -> std::io::Result<()> {
        execute!(out, MoveTo(0, list_start), Clear(ClearType::CurrentLine))?;
        execute!(out, Print(format!("\x1b[38;5;244m{}\x1b[0m", sep)))?;
        for i in 0..display_count {
            execute!(out, MoveTo(0, list_start + 1 + i as u16), Clear(ClearType::CurrentLine))?;
            execute!(out, Print(render_one_candidate(&candidates[i], i == sel)))?;
        }
        if extra_line > 0 {
            execute!(out, MoveTo(0, list_start + 1 + display_count as u16), Clear(ClearType::CurrentLine))?;
            execute!(out, Print(format!(
                "\x1b[48;5;238m\x1b[38;5;244m    … {} more results\x1b[0m",
                candidates.len() - display_count
            )))?;
        }
        execute!(out, MoveTo(0, list_start + list_height - 1), Clear(ClearType::CurrentLine))?;
        execute!(out, Print(format!("\x1b[38;5;244m{}\x1b[0m", sep)))?;
        execute!(out, MoveTo(orig_col, orig_row))?;
        out.flush()
    };

    draw(&mut out, selected)?;

    let result = loop {
        match ev_read()? {
            Event::Key(KeyEvent { code: KeyCode::Up, modifiers: KeyModifiers::NONE, .. }) => {
                let old = selected;
                selected = if selected == 0 { display_count - 1 } else { selected - 1 };
                execute!(out,
                    MoveTo(0, list_start + 1 + old as u16),
                    Clear(ClearType::CurrentLine),
                    Print(render_one_candidate(&candidates[old], false))
                )?;
                execute!(out,
                    MoveTo(0, list_start + 1 + selected as u16),
                    Clear(ClearType::CurrentLine),
                    Print(render_one_candidate(&candidates[selected], true))
                )?;
                execute!(out, MoveTo(orig_col, orig_row))?;
                out.flush()?;
            }
            Event::Key(KeyEvent { code: KeyCode::Down, modifiers: KeyModifiers::NONE, .. }) => {
                let old = selected;
                selected = (selected + 1) % display_count;
                execute!(out,
                    MoveTo(0, list_start + 1 + old as u16),
                    Clear(ClearType::CurrentLine),
                    Print(render_one_candidate(&candidates[old], false))
                )?;
                execute!(out,
                    MoveTo(0, list_start + 1 + selected as u16),
                    Clear(ClearType::CurrentLine),
                    Print(render_one_candidate(&candidates[selected], true))
                )?;
                execute!(out, MoveTo(orig_col, orig_row))?;
                out.flush()?;
            }
            Event::Key(KeyEvent { code: KeyCode::Enter, modifiers: KeyModifiers::NONE, .. })
            | Event::Key(KeyEvent { code: KeyCode::Tab, modifiers: KeyModifiers::NONE, .. }) => {
                break Some(candidates[selected].replacement.clone());
            }
            Event::Key(KeyEvent { code: KeyCode::Esc, .. })
            | Event::Key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::CONTROL,
                ..
            }) => {
                break None;
            }
            _ => {}
        }
    };

    for i in 0..list_height {
        execute!(out, MoveTo(0, list_start + i), Clear(ClearType::CurrentLine))?;
    }
    execute!(out, MoveTo(orig_col, orig_row))?;
    out.flush()?;

    Ok(result)
}

/// 交互式读取一行输入（crossterm raw mode）
pub fn interactive_readline(
    prompt: &str,
    history: &mut Vec<String>,
) -> std::io::Result<ReadLine> {
    use crossterm::cursor::MoveToColumn;
    use crossterm::event::{read as ev_read, Event, KeyCode, KeyEvent, KeyModifiers};
    use crossterm::style::Print;
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    use crossterm::execute;
    use std::io::stdout;

    enable_raw_mode()?;
    let mut out = stdout();

    let mut chars: Vec<char> = Vec::new();
    let mut cursor: usize = 0;
    let mut history_idx: Option<usize> = None;
    let mut history_saved = String::new();
    let mut candidates: Vec<Pair> = Vec::new();
    let mut selected: Option<usize> = None;
    let mut offset: usize = 0;
    // 是否正在翻历史（翻历史时不触发补全列表）
    let mut in_history_browse = false;

    let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &candidates, selected);

    let result = loop {
        let event = match ev_read() {
            Ok(e) => e,
            Err(e) => { let _ = disable_raw_mode(); return Err(e); }
        };

        match event {
            // ── Ctrl+C 中断 ──
            Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers: KeyModifiers::CONTROL, .. }) => {
                let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &[], None);
                let _ = execute!(out, Print("\r\n"));
                break ReadLine::Interrupted;
            }
            // ── Ctrl+D EOF（仅在输入为空时） ──
            Event::Key(KeyEvent { code: KeyCode::Char('d'), modifiers: KeyModifiers::CONTROL, .. }) => {
                if chars.is_empty() {
                    let _ = execute!(out, Print("\r\n"));
                    break ReadLine::Eof;
                }
            }
            // ── Enter 提交 / 确认候选 ──
            Event::Key(KeyEvent { code: KeyCode::Enter, modifiers: KeyModifiers::NONE, .. }) => {
                if let Some(idx) = selected {
                    // 有选中候选项：应用候选
                    let is_file_completion = offset > 0; // offset > 0 表示 @ 触发的文件/目录补全
                    apply_candidate(&mut chars, &mut cursor, &candidates[idx], offset);
                    selected = None;
                    candidates.clear();
                    offset = 0;
                    in_history_browse = false;

                    if is_file_completion {
                        // @ 文件/目录补全：只完成补全，不提交，让用户继续输入
                        let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &[], None);
                    } else {
                        // /命令 补全：应用后直接提交
                        let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &[], None);
                        let _ = execute!(out, Print("\r\n"));
                        let line: String = chars.iter().collect();
                        break ReadLine::Line(line);
                    }
                } else if in_history_browse {
                    // 从历史翻来的内容按 Enter：
                    // 如果是 / 开头的命令，先显示补全列表供选择
                    let line: String = chars.iter().collect();
                    if line.starts_with('/') {
                        in_history_browse = false;
                        history_idx = None;
                        update_completion(&chars, cursor, &mut candidates, &mut selected, &mut offset);
                        if !candidates.is_empty() {
                            // 显示补全列表，等待用户选择
                            let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &candidates, selected);
                        } else {
                            // 没有补全候选，直接提交
                            let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &[], None);
                            let _ = execute!(out, Print("\r\n"));
                            break ReadLine::Line(line);
                        }
                    } else {
                        // 非命令历史，直接提交
                        in_history_browse = false;
                        let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &[], None);
                        let _ = execute!(out, Print("\r\n"));
                        break ReadLine::Line(line);
                    }
                } else {
                    let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &[], None);
                    let _ = execute!(out, Print("\r\n"));
                    let line: String = chars.iter().collect();
                    break ReadLine::Line(line);
                }
            }
            // ── Tab 选择/确认候选 ──
            Event::Key(KeyEvent { code: KeyCode::Tab, modifiers: KeyModifiers::NONE, .. }) => {
                if !candidates.is_empty() {
                    if let Some(idx) = selected {
                        apply_candidate(&mut chars, &mut cursor, &candidates[idx], offset);
                        selected = None;
                        candidates.clear();
                        offset = 0;
                        update_completion(&chars, cursor, &mut candidates, &mut selected, &mut offset);
                    } else {
                        selected = Some(0);
                    }
                    let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &candidates, selected);
                }
            }
            // ── Esc 关闭候选 ──
            Event::Key(KeyEvent { code: KeyCode::Esc, .. }) => {
                if !candidates.is_empty() {
                    candidates.clear();
                    selected = None;
                    offset = 0;
                    let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &[], None);
                }
            }
            // ── ↑ 候选上移 / 历史向上 ──
            // 规则：正在翻历史（in_history_browse）或无候选时，上键翻历史（不显示补全）
            //       未翻历史且有候选时，上键在候选列表里移动
            Event::Key(KeyEvent { code: KeyCode::Up, modifiers: KeyModifiers::NONE, .. }) => {
                if !in_history_browse && !candidates.is_empty() {
                    // 有补全候选且未在翻历史：上键在候选列表里移动
                    let n = candidates.len().min(MAX_COMPLETION_DISPLAY);
                    selected = Some(match selected {
                        None | Some(0) => n.saturating_sub(1),
                        Some(i) => i - 1,
                    });
                    let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &candidates, selected);
                } else if !history.is_empty() {
                    // 翻历史模式：关闭补全列表，不触发新的补全
                    candidates.clear();
                    selected = None;
                    offset = 0;
                    if history_idx.is_none() { history_saved = chars.iter().collect(); }
                    let idx = match history_idx {
                        None => history.len() - 1,
                        Some(0) => 0,
                        Some(i) => i - 1,
                    };
                    history_idx = Some(idx);
                    in_history_browse = true;
                    chars = history[idx].chars().collect();
                    cursor = chars.len();
                    // 翻历史时不显示补全列表
                    let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &[], None);
                }
            }
            // ── ↓ 候选下移 / 历史向下 ──
            Event::Key(KeyEvent { code: KeyCode::Down, modifiers: KeyModifiers::NONE, .. }) => {
                if !in_history_browse && !candidates.is_empty() {
                    // 有补全候选且未在翻历史：下键在候选列表里移动
                    let n = candidates.len().min(MAX_COMPLETION_DISPLAY);
                    selected = Some(match selected {
                        None => 0,
                        Some(i) => (i + 1) % n,
                    });
                    let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &candidates, selected);
                } else if let Some(idx) = history_idx {
                    // 翻历史模式：继续向下翻
                    candidates.clear();
                    selected = None;
                    offset = 0;
                    if idx + 1 < history.len() {
                        history_idx = Some(idx + 1);
                        chars = history[idx + 1].chars().collect();
                    } else {
                        history_idx = None;
                        in_history_browse = false;
                        chars = history_saved.chars().collect();
                    }
                    cursor = chars.len();
                    // 翻历史时不显示补全列表
                    let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &[], None);
                }
            }
            // ── ← 光标左移 ──
            Event::Key(KeyEvent { code: KeyCode::Left, modifiers: KeyModifiers::NONE, .. }) => {
                if cursor > 0 {
                    cursor -= 1;
                    let col = (visible_columns(prompt) + chars_display_cols(&chars, cursor)) as u16;
                    let _ = execute!(out, MoveToColumn(col));
                    let _ = out.flush();
                }
            }
            // ── → 光标右移 ──
            Event::Key(KeyEvent { code: KeyCode::Right, modifiers: KeyModifiers::NONE, .. }) => {
                if cursor < chars.len() {
                    cursor += 1;
                    let col = (visible_columns(prompt) + chars_display_cols(&chars, cursor)) as u16;
                    let _ = execute!(out, MoveToColumn(col));
                    let _ = out.flush();
                }
            }
            // ── Ctrl+A 行首 ──
            Event::Key(KeyEvent { code: KeyCode::Char('a'), modifiers: KeyModifiers::CONTROL, .. }) => {
                cursor = 0;
                let col = visible_columns(prompt) as u16;
                let _ = execute!(out, MoveToColumn(col));
                let _ = out.flush();
            }
            // ── Ctrl+E 行尾 ──
            Event::Key(KeyEvent { code: KeyCode::Char('e'), modifiers: KeyModifiers::CONTROL, .. }) => {
                cursor = chars.len();
                let col = (visible_columns(prompt) + chars_display_cols(&chars, cursor)) as u16;
                let _ = execute!(out, MoveToColumn(col));
                let _ = out.flush();
            }
            // ── Ctrl+U 清除行 ──
            Event::Key(KeyEvent { code: KeyCode::Char('u'), modifiers: KeyModifiers::CONTROL, .. }) => {
                chars.clear();
                cursor = 0;
                history_idx = None;
                update_completion(&chars, cursor, &mut candidates, &mut selected, &mut offset);
                let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &candidates, selected);
            }
            // ── Backspace ──
            Event::Key(KeyEvent { code: KeyCode::Backspace, .. }) => {
                if cursor > 0 {
                    cursor -= 1;
                    chars.remove(cursor);
                    history_idx = None;
                    update_completion(&chars, cursor, &mut candidates, &mut selected, &mut offset);
                    let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &candidates, selected);
                }
            }
            // ── Delete（前向删除） ──
            Event::Key(KeyEvent { code: KeyCode::Delete, .. }) => {
                if cursor < chars.len() {
                    chars.remove(cursor);
                    update_completion(&chars, cursor, &mut candidates, &mut selected, &mut offset);
                    let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &candidates, selected);
                }
            }
            // ── 普通字符 ──
            Event::Key(KeyEvent {
                code: KeyCode::Char(c),
                modifiers,
                ..
            }) if modifiers == KeyModifiers::NONE || modifiers == KeyModifiers::SHIFT => {
                chars.insert(cursor, c);
                cursor += 1;
                history_idx = None;
                in_history_browse = false; // 用户开始输入，退出历史翻看模式
                update_completion(&chars, cursor, &mut candidates, &mut selected, &mut offset);
                let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &candidates, selected);
            }
            _ => {}
        }
    };

    let _ = disable_raw_mode();
    Ok(result)
}
