pub mod coordinator;
pub mod message_bus;
pub mod consensus;

pub use coordinator::CollaborationCoordinator;
pub use message_bus::MessageBus;
pub use consensus::ConsensusEngine;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollaborationSession {
    pub id: Uuid,
    pub name: String,
    pub task: String,
    pub agents: Vec<Uuid>,
    pub mode: CollaborationMode,
    pub status: SessionStatus,
    pub messages: Vec<CollabMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CollaborationMode {
    Sequential,
    Parallel,
    Consensus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionStatus {
    Created,
    Running,
    Paused,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabMessage {
    pub id: Uuid,
    pub from_agent: Uuid,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub message_type: MessageType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageType {
    Proposal,
    Vote,
    Feedback,
    Result,
    Error,
}
