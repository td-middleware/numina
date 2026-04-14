use std::io::Write;

use rustyline::completion::Pair;

use super::completer::ChatCompleter;

// ─────────────────────────────────────────────
// crossterm 实时交互式 readline
// ─────────────────────────────────────────────

const MAX_COMPLETION_DISPLAY: usize = 10;

/// 粘贴块占位字符的起始 code point（Unicode 私用区 E100..=EFFF）
const PASTE_MARKER_BASE: u32 = 0xE100;
/// 粘贴块占位字符的最大 code point
const PASTE_MARKER_MAX: u32 = 0xEFFF;

/// 判断字符是否为粘贴块占位符（Unicode 私用区 E100..=EFFF）
#[inline]
fn is_paste_marker(c: char) -> bool {
    let cp = c as u32;
    cp >= PASTE_MARKER_BASE && cp <= PASTE_MARKER_MAX
}

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

/// 构建用于打印的显示字符串：
/// - paste marker 字符替换为带 ANSI 颜色的折叠标签（如 `[Pasted text #1 +8 lines]`）
/// - 其余字符直接输出
fn build_display_string(
    chars: &[char],
    pasted_chunks: &std::collections::HashMap<char, String>,
) -> String {
    let mut result = String::new();
    let mut paste_num = 0usize;
    for &c in chars {
        if is_paste_marker(c) {
            paste_num += 1;
            let plain = if let Some(chunk) = pasted_chunks.get(&c) {
                let line_count = chunk.lines().count().max(1);
                format!("[Pasted text #{} +{} lines]", paste_num, line_count)
            } else {
                format!("[Pasted text #{}]", paste_num)
            };
            result.push_str(&format!("\x1b[48;5;238m\x1b[38;5;117m {} \x1b[0m", plain));
        } else {
            result.push(c);
        }
    }
    result
}

/// 计算 chars[..n] 的总可视列数
/// - paste marker 按折叠标签的实际显示宽度计算（" [Pasted text #N +M lines] "）
/// - 普通字符按 unicode_char_width 计算
fn chars_vis_cols_paste(
    chars: &[char],
    n: usize,
    pasted_chunks: &std::collections::HashMap<char, String>,
) -> usize {
    let mut cols = 0usize;
    let mut paste_num = 0usize;
    for &c in &chars[..n] {
        if is_paste_marker(c) {
            paste_num += 1;
            let tag_vis = if let Some(chunk) = pasted_chunks.get(&c) {
                let line_count = chunk.lines().count().max(1);
                format!("[Pasted text #{} +{} lines]", paste_num, line_count).len() + 2
            } else {
                format!("[Pasted text #{}]", paste_num).len() + 2
            };
            cols += tag_vis;
        } else {
            cols += unicode_char_width(c);
        }
    }
    cols
}

/// 展开 chars 中所有 paste marker，替换为实际粘贴内容，返回完整字符串（用于提交）
fn expand_chars_content(
    chars: &[char],
    pasted_chunks: &std::collections::HashMap<char, String>,
) -> String {
    let mut result = String::new();
    for &c in chars {
        if is_paste_marker(c) {
            if let Some(chunk) = pasted_chunks.get(&c) {
                result.push_str(chunk);
            }
        } else {
            result.push(c);
        }
    }
    result
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

    // 在 chars[..cursor] 里找最后一个 @ 字符（绝对 char 索引，忽略 paste markers）
    if let Some(at_char_idx) = chars[..cursor].iter().rposition(|&c| c == '@') {
        let path_part: String = chars[at_char_idx + 1..cursor].iter()
            .filter(|&&c| !is_paste_marker(c))
            .collect();
        let cands = ChatCompleter::complete_path(&path_part);
        if !cands.is_empty() {
            *candidates = cands;
            *selected = None;
            *offset = at_char_idx + 1; // @ 后面的位置（chars 里绝对索引）
            return;
        }
    }

    // 提取过滤 paste marker 后的前 cursor 字符（用于 / 命令匹配）
    let word: String = chars[..cursor].iter()
        .filter(|&&c| !is_paste_marker(c))
        .collect();

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

/// 重绘输入行 + 候选列表
///
/// 用户手动输入的内容完整显示（超长时自然终端 wrap），paste marker 渲染为折叠标签。
///
/// `pasted_chunks`：paste marker 字符 → 实际粘贴内容的映射。
/// `prev_cursor_row`：上次渲染时光标所在的行（相对起始行，0-indexed）。
///
/// 返回 `(total_rendered_rows, this_cursor_row)`：
/// - `total_rendered_rows`：本次渲染的总行数（含候选列表）
/// - `this_cursor_row`：本次光标所在行（相对起始行，0-indexed），供下次重绘回退
fn redraw_input_line(
    out: &mut std::io::Stdout,
    prompt: &str,
    chars: &[char],
    cursor: usize,
    candidates: &[Pair],
    selected: Option<usize>,
    pasted_chunks: &std::collections::HashMap<char, String>,
    prev_cursor_row: usize,
) -> std::io::Result<(usize, usize)> {
    use crossterm::cursor::{MoveToColumn, MoveUp};
    use crossterm::style::Print;
    use crossterm::terminal::{Clear, ClearType, size as term_size};
    use crossterm::execute;

    let display_count = candidates.len().min(MAX_COMPLETION_DISPLAY);
    let extra = usize::from(candidates.len() > display_count);
    let cand_lines = if candidates.is_empty() { 0 } else { display_count + 2 + extra };

    let (term_w, term_h) = term_size().map(|(w, h)| (w as usize, h as usize)).unwrap_or((80, 24));
    let prompt_vis = visible_columns(prompt);

    // 向上回退到起始行（光标在上次渲染的 cursor_row 处）
    // 安全限制：MoveUp 不超过终端高度，避免超出屏幕触发终端内部 bug
    let safe_prev_up = prev_cursor_row.min(term_h.saturating_sub(2));
    if safe_prev_up > 0 {
        execute!(out, MoveUp(safe_prev_up as u16))?;
    }
    execute!(out, MoveToColumn(0), Clear(ClearType::FromCursorDown))?;

    // 打印 prompt
    execute!(out, Print(prompt))?;

    // 打印输入内容：paste marker → 折叠标签，其余字符直接打印（允许终端自然 wrap）
    let display_str = build_display_string(chars, pasted_chunks);
    execute!(out, Print(&display_str))?;

    // 计算输入区域总可视列数（含 prompt）
    let display_total_vis = chars_vis_cols_paste(chars, chars.len(), pasted_chunks);
    let total_vis = prompt_vis + display_total_vis;
    // 输入区域占用的行数（终端 wrap 后）
    let input_rows = ((total_vis + term_w - 1) / term_w).max(1);
    let mut total_rows = input_rows;

    // 渲染候选列表（在输入区域之下）
    if !candidates.is_empty() {
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
        // 此时光标回到输入最后一行（input_last_row）
        total_rows += cand_lines;
    }

    // 定位光标到逻辑位置
    // 注意：终端使用"延迟 wrap"——打印恰好 term_w 个字符后光标仍在当前行末尾，
    // 下次打印才换行。所以 cursor_vis 是 term_w 整数倍时，光标在上一行末尾而非下一行行首。
    // 若不处理这个边界，cursor_row 会多算 1，导致每次重绘 MoveUp 多移一行，
    // 最终光标超出屏幕顶部，触发 Terminal.app 内部追踪区域越界崩溃。
    let cursor_vis = prompt_vis + chars_vis_cols_paste(chars, cursor, pasted_chunks);
    let (cursor_row, cursor_col) = if term_w > 0 {
        if cursor_vis > 0 && cursor_vis % term_w == 0 {
            // 延迟 wrap：恰好填满 N 行，光标停在第 N-1 行末（0-indexed）
            let row = (cursor_vis / term_w).saturating_sub(1).min(term_h.saturating_sub(1));
            (row, term_w - 1)
        } else {
            let row = (cursor_vis / term_w).min(term_h.saturating_sub(1));
            (row, cursor_vis % term_w)
        }
    } else {
        (0, cursor_vis)
    };

    // input_last_row：输入区域最后一行，同样考虑延迟 wrap
    let input_last_row = if total_vis == 0 {
        0
    } else if total_vis % term_w == 0 {
        (total_vis / term_w).saturating_sub(1)
    } else {
        (total_vis - 1) / term_w
    };
    let rows_to_move_up = input_last_row.saturating_sub(cursor_row);
    if rows_to_move_up > 0 {
        execute!(out, MoveUp(rows_to_move_up as u16))?;
    }
    execute!(out, MoveToColumn(cursor_col as u16))?;

    out.flush()?;
    Ok((total_rows, cursor_row))
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
///
/// - 用户手动输入的内容完整显示，超长时自然终端 wrap
/// - 粘贴的多行内容折叠为 `[Pasted text #N +M lines]` 标签，提交时自动展开为完整内容
pub fn interactive_readline(
    prompt: &str,
    history: &mut Vec<String>,
) -> std::io::Result<ReadLine> {
    use crossterm::event::{read as ev_read, Event, KeyCode, KeyEvent, KeyModifiers};
    use crossterm::style::Print;
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    use crossterm::execute;
    use std::collections::HashMap;
    use std::io::stdout;

    enable_raw_mode()?;
    let mut out = stdout();

    // 启用 bracketed paste mode：粘贴内容被 \x1b[200~ ... \x1b[201~ 包裹
    // crossterm >= 0.27 支持 Event::Paste，可区分粘贴和手动输入
    let _ = execute!(out, crossterm::event::EnableBracketedPaste);
    let _ = out.flush();

    let mut chars: Vec<char> = Vec::new();
    let mut cursor: usize = 0;
    let mut history_idx: Option<usize> = None;
    let mut history_saved = String::new();
    let mut candidates: Vec<Pair> = Vec::new();
    let mut selected: Option<usize> = None;
    let mut offset: usize = 0;
    // 是否正在翻历史（翻历史时不触发补全列表）
    let mut in_history_browse = false;

    // 粘贴块存储：paste marker 字符 → 实际粘贴内容
    let mut pasted_chunks: HashMap<char, String> = HashMap::new();
    // 下一个 paste marker 的 code point（私用区 E100..=EFFF）
    let mut next_paste_code: u32 = PASTE_MARKER_BASE;

    // 上次渲染时光标所在行（相对起始行，0-indexed），用于下次重绘精确回退
    let mut prev_cursor_row: usize = 0;

    // 统一重绘宏：自动传递/更新 prev_cursor_row
    macro_rules! redraw {
        ($cands:expr, $sel:expr) => {
            if let Ok((_, crow)) = redraw_input_line(
                &mut out, prompt, &chars, cursor, $cands, $sel,
                &pasted_chunks, prev_cursor_row,
            ) {
                prev_cursor_row = crow;
            }
        };
    }

    // 退出时关闭 bracketed paste mode 的辅助宏
    macro_rules! exit_readline {
        ($result:expr) => {{
            let _ = execute!(out, crossterm::event::DisableBracketedPaste);
            let _ = out.flush();
            let _ = disable_raw_mode();
            return Ok($result);
        }};
    }

    redraw!(&candidates, selected);

    let result = loop {
        let event = match ev_read() {
            Ok(e) => e,
            Err(e) => {
                let _ = execute!(out, crossterm::event::DisableBracketedPaste);
                let _ = disable_raw_mode();
                return Err(e);
            }
        };

        match event {
            // ── Bracketed paste ──
            // crossterm >= 0.27 将 bracketed paste 解析为 Event::Paste
            Event::Paste(pasted_text) => {
                let has_newline = pasted_text.contains('\n');
                if has_newline && next_paste_code <= PASTE_MARKER_MAX {
                    // 多行粘贴：用 paste marker 折叠，实际内容存入 pasted_chunks
                    let marker = char::from_u32(next_paste_code).unwrap_or('\u{E100}');
                    next_paste_code += 1;
                    pasted_chunks.insert(marker, pasted_text);
                    chars.insert(cursor, marker);
                    cursor += 1;
                } else {
                    // 单行粘贴（不含换行）：直接插入字符
                    for c in pasted_text.chars() {
                        if c != '\n' && c != '\r' {
                            chars.insert(cursor, c);
                            cursor += 1;
                        }
                    }
                }
                in_history_browse = false;
                candidates.clear();
                selected = None;
                offset = 0;
                redraw!(&[], None);
            }
            // ── Ctrl+C 中断 ──
            Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers: KeyModifiers::CONTROL, .. }) => {
                redraw!(&[], None);
                let _ = execute!(out, Print("\r\n"));
                let _ = execute!(out, crossterm::event::DisableBracketedPaste);
                let _ = out.flush();
                break ReadLine::Interrupted;
            }
            // ── Ctrl+D EOF（仅在输入为空时） ──
            Event::Key(KeyEvent { code: KeyCode::Char('d'), modifiers: KeyModifiers::CONTROL, .. }) => {
                if chars.is_empty() {
                    let _ = execute!(out, Print("\r\n"));
                    let _ = execute!(out, crossterm::event::DisableBracketedPaste);
                    let _ = out.flush();
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
                        redraw!(&[], None);
                    } else {
                        // /命令 补全：应用后直接提交
                        redraw!(&[], None);
                        let _ = execute!(out, Print("\r\n"));
                        let line = expand_chars_content(&chars, &pasted_chunks);
                        exit_readline!(ReadLine::Line(line));
                    }
                } else if in_history_browse {
                    // 翻历史时按 Enter：若 / 命令则先显示补全列表
                    let line_raw: String = chars.iter()
                        .filter(|&&c| !is_paste_marker(c))
                        .collect();
                    if line_raw.starts_with('/') {
                        in_history_browse = false;
                        history_idx = None;
                        update_completion(&chars, cursor, &mut candidates, &mut selected, &mut offset);
                        if !candidates.is_empty() {
                            // 显示补全列表，等待用户选择
                            redraw!(&candidates, selected);
                        } else {
                            // 没有补全候选，直接提交
                            redraw!(&[], None);
                            let _ = execute!(out, Print("\r\n"));
                            let line = expand_chars_content(&chars, &pasted_chunks);
                            exit_readline!(ReadLine::Line(line));
                        }
                    } else {
                        // 非命令历史，直接提交
                        in_history_browse = false;
                        redraw!(&[], None);
                        let _ = execute!(out, Print("\r\n"));
                        let line = expand_chars_content(&chars, &pasted_chunks);
                        exit_readline!(ReadLine::Line(line));
                    }
                } else {
                    redraw!(&[], None);
                    let _ = execute!(out, Print("\r\n"));
                    let line = expand_chars_content(&chars, &pasted_chunks);
                    exit_readline!(ReadLine::Line(line));
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
                    redraw!(&candidates, selected);
                }
            }
            // ── Esc 关闭候选 ──
            Event::Key(KeyEvent { code: KeyCode::Esc, .. }) => {
                if !candidates.is_empty() {
                    candidates.clear();
                    selected = None;
                    offset = 0;
                    redraw!(&[], None);
                }
            }
            // ── ↑ 候选上移 / 历史向上 ──
            // 规则：有候选且未翻历史时，上键在候选列表移动；否则翻历史
            Event::Key(KeyEvent { code: KeyCode::Up, modifiers: KeyModifiers::NONE, .. }) => {
                if !in_history_browse && !candidates.is_empty() {
                    let n = candidates.len().min(MAX_COMPLETION_DISPLAY);
                    selected = Some(match selected {
                        None | Some(0) => n.saturating_sub(1),
                        Some(i) => i - 1,
                    });
                    redraw!(&candidates, selected);
                } else if !history.is_empty() {
                    candidates.clear();
                    selected = None;
                    offset = 0;
                    if history_idx.is_none() {
                        // 保存当前输入（展开 paste markers 以保存完整内容）
                        history_saved = expand_chars_content(&chars, &pasted_chunks);
                    }
                    let idx = match history_idx {
                        None => history.len() - 1,
                        Some(0) => 0,
                        Some(i) => i - 1,
                    };
                    history_idx = Some(idx);
                    in_history_browse = true;
                    chars = history[idx].chars().collect();
                    // 历史内容不含 paste markers，清空 pasted_chunks
                    pasted_chunks.clear();
                    next_paste_code = PASTE_MARKER_BASE;
                    cursor = chars.len();
                    redraw!(&[], None);
                }
            }
            // ── ↓ 候选下移 / 历史向下 ──
            Event::Key(KeyEvent { code: KeyCode::Down, modifiers: KeyModifiers::NONE, .. }) => {
                if !in_history_browse && !candidates.is_empty() {
                    let n = candidates.len().min(MAX_COMPLETION_DISPLAY);
                    selected = Some(match selected {
                        None => 0,
                        Some(i) => (i + 1) % n,
                    });
                    redraw!(&candidates, selected);
                } else if let Some(idx) = history_idx {
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
                    pasted_chunks.clear();
                    next_paste_code = PASTE_MARKER_BASE;
                    cursor = chars.len();
                    redraw!(&[], None);
                }
            }
            // ── ← 光标左移 ──
            Event::Key(KeyEvent { code: KeyCode::Left, modifiers: KeyModifiers::NONE, .. }) => {
                if cursor > 0 {
                    cursor -= 1;
                    redraw!(&candidates, selected);
                }
            }
            // ── → 光标右移 ──
            Event::Key(KeyEvent { code: KeyCode::Right, modifiers: KeyModifiers::NONE, .. }) => {
                if cursor < chars.len() {
                    cursor += 1;
                    redraw!(&candidates, selected);
                }
            }
            // ── Ctrl+A 行首 ──
            Event::Key(KeyEvent { code: KeyCode::Char('a'), modifiers: KeyModifiers::CONTROL, .. }) => {
                cursor = 0;
                redraw!(&candidates, selected);
            }
            // ── Ctrl+E 行尾 ──
            Event::Key(KeyEvent { code: KeyCode::Char('e'), modifiers: KeyModifiers::CONTROL, .. }) => {
                cursor = chars.len();
                redraw!(&candidates, selected);
            }
            // ── Ctrl+U 清除行 ──
            Event::Key(KeyEvent { code: KeyCode::Char('u'), modifiers: KeyModifiers::CONTROL, .. }) => {
                chars.clear();
                cursor = 0;
                history_idx = None;
                pasted_chunks.clear();
                next_paste_code = PASTE_MARKER_BASE;
                update_completion(&chars, cursor, &mut candidates, &mut selected, &mut offset);
                redraw!(&candidates, selected);
            }
            // ── Backspace ──
            Event::Key(KeyEvent { code: KeyCode::Backspace, .. }) => {
                if cursor > 0 {
                    cursor -= 1;
                    chars.remove(cursor);
                    history_idx = None;
                    update_completion(&chars, cursor, &mut candidates, &mut selected, &mut offset);
                    redraw!(&candidates, selected);
                }
            }
            // ── Delete（前向删除） ──
            Event::Key(KeyEvent { code: KeyCode::Delete, .. }) => {
                if cursor < chars.len() {
                    chars.remove(cursor);
                    update_completion(&chars, cursor, &mut candidates, &mut selected, &mut offset);
                    redraw!(&candidates, selected);
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
                in_history_browse = false;
                update_completion(&chars, cursor, &mut candidates, &mut selected, &mut offset);
                redraw!(&candidates, selected);
            }
            _ => {}
        }
    };

    let _ = execute!(out, crossterm::event::DisableBracketedPaste);
    let _ = out.flush();
    let _ = disable_raw_mode();
    Ok(result)
}
