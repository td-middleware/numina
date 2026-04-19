//! Memory 文件存储层
//!
//! 全局记忆：  ~/.numina/memory/global.json
//! 项目记忆：  {cwd}/.numina/memory.json

use anyhow::Result;
use std::path::PathBuf;
use crate::memory::types::{MemoryEntry, MemoryScope};

// ─────────────────────────────────────────────
// 路径解析
// ─────────────────────────────────────────────

pub fn global_memory_path() -> Option<PathBuf> {
    let dir = dirs::home_dir()?.join(".numina").join("memory");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("global.json"))
}

pub fn project_memory_path() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let dir = cwd.join(".numina");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("memory.json"))
}

// ─────────────────────────────────────────────
// 读写
// ─────────────────────────────────────────────

pub fn load_from(path: &PathBuf) -> Vec<MemoryEntry> {
    if !path.exists() {
        return Vec::new();
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_to(path: &PathBuf, entries: &[MemoryEntry]) -> Result<()> {
    let content = serde_json::to_string_pretty(entries)?;
    std::fs::write(path, content)?;
    Ok(())
}

// ─────────────────────────────────────────────
// MemoryStore 公共接口
// ─────────────────────────────────────────────

pub struct MemoryStore;

impl MemoryStore {
    /// 加载所有记忆（全局 + 项目）
    pub fn load_all() -> Vec<MemoryEntry> {
        let mut all = Vec::new();
        if let Some(p) = global_memory_path() {
            all.extend(load_from(&p));
        }
        if let Some(p) = project_memory_path() {
            all.extend(load_from(&p));
        }
        all
    }

    /// 加载全局记忆
    pub fn load_global() -> Vec<MemoryEntry> {
        global_memory_path()
            .map(|p| load_from(&p))
            .unwrap_or_default()
    }

    /// 加载项目记忆
    pub fn load_project() -> Vec<MemoryEntry> {
        project_memory_path()
            .map(|p| load_from(&p))
            .unwrap_or_default()
    }

    /// 添加一条记忆，返回新记忆的 ID
    pub fn add(entry: MemoryEntry) -> Result<String> {
        let id = entry.id.clone();
        let path = match entry.scope {
            MemoryScope::Global => global_memory_path()
                .ok_or_else(|| anyhow::anyhow!("Cannot determine global memory path"))?,
            MemoryScope::Project => project_memory_path()
                .ok_or_else(|| anyhow::anyhow!("Cannot determine project memory path"))?,
        };
        let mut entries = load_from(&path);
        entries.push(entry);
        save_to(&path, &entries)?;
        Ok(id)
    }

    /// 删除一条记忆（按 ID 前缀匹配），返回是否删除成功
    pub fn remove(id_prefix: &str) -> Result<bool> {
        let mut removed = false;

        if let Some(p) = global_memory_path() {
            let mut entries = load_from(&p);
            let before = entries.len();
            entries.retain(|e| !e.id.starts_with(id_prefix));
            if entries.len() < before {
                save_to(&p, &entries)?;
                removed = true;
            }
        }

        if let Some(p) = project_memory_path() {
            let mut entries = load_from(&p);
            let before = entries.len();
            entries.retain(|e| !e.id.starts_with(id_prefix));
            if entries.len() < before {
                save_to(&p, &entries)?;
                removed = true;
            }
        }

        Ok(removed)
    }

    /// 清空所有记忆（全局 + 项目）
    pub fn clear_all() -> Result<()> {
        if let Some(p) = global_memory_path() {
            save_to(&p, &[])?;
        }
        if let Some(p) = project_memory_path() {
            save_to(&p, &[])?;
        }
        Ok(())
    }
}
