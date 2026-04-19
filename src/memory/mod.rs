//! Memory 模块 — 跨会话的持久化记忆能力
//!
//! 目录结构：
//!   src/memory/
//!     types.rs   — 数据结构（MemoryEntry, MemoryScope, MemorySource）
//!     store.rs   — 文件读写（MemoryStore）
//!     search.rs  — 关键词检索
//!     mod.rs     — 公共导出

pub mod types;
pub mod store;
pub mod search;

// 便捷重导出
pub use types::{MemoryEntry, MemoryScope, MemorySource};
pub use store::MemoryStore;
pub use search::{search_memories, build_memory_prompt};
