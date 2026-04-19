//! Memory 关键词检索

use crate::memory::store::MemoryStore;
use crate::memory::types::MemoryEntry;

/// 关键词检索，返回相关度最高的 N 条
/// 打分规则：
///   - content 中包含查询词：+1 分/词
///   - tags 中包含查询词：+2 分/词（权重更高）
pub fn search_memories(query: &str, limit: usize) -> Vec<MemoryEntry> {
    let all = MemoryStore::load_all();
    if query.is_empty() {
        return all.into_iter().take(limit).collect();
    }

    let query_words: Vec<String> = query
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() > 1)
        .collect();

    if query_words.is_empty() {
        return all.into_iter().take(limit).collect();
    }

    let mut scored: Vec<(usize, MemoryEntry)> = all
        .into_iter()
        .map(|e| {
            let content_lower = e.content.to_lowercase();
            let content_score = query_words
                .iter()
                .filter(|w| content_lower.contains(w.as_str()))
                .count();
            let tag_score = e
                .tags
                .iter()
                .filter(|t| query_words.iter().any(|w| t.contains(w.as_str())))
                .count()
                * 2;
            (content_score + tag_score, e)
        })
        .filter(|(score, _)| *score > 0)
        .collect();

    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().take(limit).map(|(_, e)| e).collect()
}

/// 构建注入 system prompt 的记忆块
/// 返回空字符串表示没有记忆
pub fn build_memory_prompt(query: &str) -> String {
    let relevant = if query.is_empty() {
        // 无查询时取最近 10 条（全部加载，按添加顺序取最新）
        let mut all = MemoryStore::load_all();
        // 取最后 10 条（最新的）
        if all.len() > 10 {
            all = all.into_iter().rev().take(10).rev().collect();
        }
        all
    } else {
        search_memories(query, 8)
    };

    if relevant.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        String::from("## Memories"),
        String::from("The following are things you should remember about the user and their projects:"),
    ];
    for entry in &relevant {
        let scope_tag = match entry.scope {
            crate::memory::types::MemoryScope::Global => "global",
            crate::memory::types::MemoryScope::Project => "project",
        };
        lines.push(format!("- [{}] {}", scope_tag, entry.content));
    }
    lines.join("\n")
}
