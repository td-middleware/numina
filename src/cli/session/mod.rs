// ─────────────────────────────────────────────
// session 模块：将原 cli/chat.rs 拆分为多个子模块
// ─────────────────────────────────────────────

pub mod completer;
pub mod commands;
pub mod file_ref;
pub mod permission;
pub mod readline;
pub mod renderer;
pub mod runner;
