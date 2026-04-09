// ─────────────────────────────────────────────
// @ 文件注入：解析消息中的 @path，替换为文件内容
// ─────────────────────────────────────────────

/// 解析消息中所有 @path 引用，将文件/文件夹内容注入到消息末尾
/// 返回 (处理后的消息, 注入的文件数量)
pub fn expand_at_references(input: &str) -> (String, usize) {
    use std::path::Path;

    let mut paths: Vec<String> = Vec::new();
    for token in input.split_whitespace() {
        if let Some(path_str) = token.strip_prefix('@') {
            if !path_str.is_empty() {
                paths.push(path_str.to_string());
            }
        }
    }

    if paths.is_empty() {
        return (input.to_string(), 0);
    }

    let mut injected = String::new();
    let mut count = 0usize;

    for path_str in &paths {
        let path = Path::new(path_str);
        if path.is_file() {
            match std::fs::read_to_string(path) {
                Ok(content) => {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    let lang = ext_to_lang(ext);
                    injected.push_str(&format!(
                        "\n\n--- File: {} ---\n```{}\n{}\n```",
                        path_str, lang, content.trim_end()
                    ));
                    count += 1;
                }
                Err(e) => {
                    injected.push_str(&format!("\n\n--- File: {} (read error: {}) ---", path_str, e));
                }
            }
        } else if path.is_dir() {
            let listing = list_dir_tree(path, 2, 50);
            injected.push_str(&format!(
                "\n\n--- Directory: {} ---\n```\n{}\n```",
                path_str, listing
            ));
            count += 1;
        } else {
            injected.push_str(&format!("\n\n--- @{}: not found ---", path_str));
        }
    }

    if injected.is_empty() {
        (input.to_string(), 0)
    } else {
        (format!("{}{}", input, injected), count)
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
