// ─────────────────────────────────────────────
// @ 文件注入：解析消息中的 @path，替换为文件内容
// ─────────────────────────────────────────────

/// 解析消息中所有 @path 引用，将文件/文件夹内容注入到消息末尾
/// 返回 (处理后的消息, 注入的文件数量)
///
/// 支持 `~` 路径展开（`~/foo` → `$HOME/foo`），并在消息文本中同步替换，
/// 避免 AI 自己猜测 `~` 对应的实际路径（AI 训练数据里可能用 `/home/user`）。
pub fn expand_at_references(input: &str) -> (String, usize) {
    use std::path::Path;

    // 获取家目录，用于展开 ~
    let home_dir = std::env::var("HOME").unwrap_or_default();

    // 展开 ~ 为家目录
    let expand_tilde = |s: &str| -> String {
        if home_dir.is_empty() {
            return s.to_string();
        }
        if s == "~" {
            home_dir.clone()
        } else if s.starts_with("~/") {
            format!("{}{}", home_dir, &s[1..])
        } else {
            s.to_string()
        }
    };

    // 判断字符是否为路径合法字符（ASCII 字母数字 + 路径分隔符）
    // 不含中文、空格、括号等，确保 @path后紧跟中文时路径能正确截断
    fn is_path_char(c: char) -> bool {
        c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_' | '~')
    }

    // 扫描 input，用字符级解析提取所有 @path 引用
    // 路径在遇到第一个非路径字符（如中文、空格、标点）时截断
    let mut paths: Vec<String> = Vec::new();
    let input_chars: Vec<char> = input.chars().collect();
    let mut ci = 0;
    while ci < input_chars.len() {
        if input_chars[ci] == '@' {
            let path_start = ci + 1;
            let mut path_end = path_start;
            while path_end < input_chars.len() && is_path_char(input_chars[path_end]) {
                path_end += 1;
            }
            if path_end > path_start {
                let path_str: String = input_chars[path_start..path_end].iter().collect();
                paths.push(path_str);
            }
            ci = path_end;
        } else {
            ci += 1;
        }
    }

    if paths.is_empty() {
        return (input.to_string(), 0);
    }

    // 把消息文本里的 @~ 替换为展开路径（path_str 只含路径字符，替换精确安全）
    let mut processed_input = input.to_string();
    for path_str in &paths {
        let expanded = expand_tilde(path_str);
        if expanded != *path_str {
            processed_input = processed_input.replace(
                &format!("@{}", path_str),
                &format!("@{}", expanded),
            );
        }
    }

    let mut injected = String::new();
    let mut count = 0usize;

    for path_str in &paths {
        let expanded = expand_tilde(path_str);
        let path = Path::new(&expanded);
        if path.is_file() {
            match std::fs::read_to_string(path) {
                Ok(content) => {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    let lang = ext_to_lang(ext);
                    injected.push_str(&format!(
                        "\n\n--- File: {} ---\n```{}\n{}\n```",
                        expanded, lang, content.trim_end()
                    ));
                    count += 1;
                }
                Err(e) => {
                    injected.push_str(&format!("\n\n--- File: {} (read error: {}) ---", expanded, e));
                }
            }
        } else if path.is_dir() {
            let listing = list_dir_tree(path, 2, 50);
            injected.push_str(&format!(
                "\n\n--- Directory: {} ---\n```\n{}\n```",
                expanded, listing
            ));
            count += 1;
        } else {
            injected.push_str(&format!("\n\n--- @{}: not found ---", expanded));
        }
    }

    if injected.is_empty() {
        (processed_input, 0)
    } else {
        (format!("{}{}", processed_input, injected), count)
    }
}

/// 文件扩展名 → 代码块语言标识
pub fn ext_to_lang(ext: &str) -> &str {
    match ext {
        "rs" => "rust",
        "go" => "go",
        "py" => "python",
        "js" | "mjs" => "javascript",
        "ts" => "typescript",
        "json" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "md" => "markdown",
        "sh" | "bash" => "bash",
        "c" => "c",
        "cpp" | "cc" | "cxx" => "cpp",
        "java" => "java",
        "html" => "html",
        "css" => "css",
        "sql" => "sql",
        "xml" => "xml",
        _ => "",
    }
}

/// 递归列出目录树，限制深度和文件数
pub fn list_dir_tree(dir: &std::path::Path, max_depth: usize, max_files: usize) -> String {
    let mut lines = Vec::new();
    let mut count = 0usize;
    list_dir_recursive(dir, "", max_depth, 0, &mut lines, &mut count, max_files);
    lines.join("\n")
}

fn list_dir_recursive(
    dir: &std::path::Path,
    prefix: &str,
    max_depth: usize,
    depth: usize,
    lines: &mut Vec<String>,
    count: &mut usize,
    max_files: usize,
) {
    if depth > max_depth || *count >= max_files {
        return;
    }
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
        Err(_) => return,
    };
    entries.sort_by(|a, b| {
        let a_is_dir = a.path().is_dir();
        let b_is_dir = b.path().is_dir();
        b_is_dir.cmp(&a_is_dir).then(a.file_name().cmp(&b.file_name()))
    });
    let entries: Vec<_> = entries.into_iter().filter(|e| {
        let name = e.file_name();
        let name_str = name.to_string_lossy();
        !name_str.starts_with('.') && name_str != "target" && name_str != "node_modules"
    }).collect();

    let total = entries.len();
    for (i, entry) in entries.iter().enumerate() {
        if *count >= max_files {
            lines.push(format!("{}  ... (truncated)", prefix));
            break;
        }
        let is_last = i == total - 1;
        let connector = if is_last { "└── " } else { "├── " };
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let path = entry.path();
        if path.is_dir() {
            lines.push(format!("{}{}{}/", prefix, connector, name_str));
            let new_prefix = format!("{}{}  ", prefix, if is_last { " " } else { "│" });
            list_dir_recursive(&path, &new_prefix, max_depth, depth + 1, lines, count, max_files);
        } else {
            lines.push(format!("{}{}{}", prefix, connector, name_str));
            *count += 1;
        }
    }
}
