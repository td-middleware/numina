//! Memory 数据类型定义

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use chrono::Utc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MemorySource {
    User, // 用户手动添加
    Auto, // AI 自动提取
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryScope {
    Global,  // 全局（跨项目，存 ~/.numina/memory/global.json）
    Project, // 项目级（存 {cwd}/.numina/memory.json）
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub content: String,
    pub tags: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    pub source: MemorySource,
    pub scope: MemoryScope,
}

impl MemoryEntry {
    pub fn new(content: impl Into<String>, source: MemorySource, scope: MemoryScope) -> Self {
        let now = Utc::now().to_rfc3339();
        let content = content.into();
        let tags = extract_tags(&content);
        Self {
            id: Uuid::new_v4().to_string()[..8].to_string(), // 短 ID，方便用户输入
            content,
            tags,
            created_at: now.clone(),
            updated_at: now,
            source,
            scope,
        }
    }
}

/// 从内容中提取简单标签（取长度 > 3 的词，最多 5 个）
pub fn extract_tags(content: &str) -> Vec<String> {
    content
        .split_whitespace()
        .filter(|w| w.len() > 3)
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .filter(|w| !w.is_empty())
        .take(5)
        .collect()
}
