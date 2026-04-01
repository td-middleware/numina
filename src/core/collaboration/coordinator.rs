use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use super::{CollaborationSession, CollaborationMode, SessionStatus, CollabMessage};
use uuid::Uuid;

pub struct CollaborationCoordinator {
    sessions: Arc<RwLock<HashMap<Uuid, CollaborationSession>>>,
    config: CoordinatorConfig,
}

#[derive(Debug, Clone)]
pub struct CoordinatorConfig {
    pub timeout_seconds: u64,
    pub max_parallel_agents: usize,
    pub consensus_required: bool,
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self {
            timeout_seconds: 300,
            max_parallel_agents: 5,
            consensus_required: false,
        }
    }
}

impl CollaborationCoordinator {
    pub fn new(config: CoordinatorConfig) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    pub async fn create_session(
        &self,
        name: String,
        task: String,
        agents: Vec<Uuid>,
        mode: CollaborationMode,
    ) -> Result<Uuid> {
        let session = CollaborationSession {
            id: Uuid::new_v4(),
            name,
            task,
            agents,
            mode,
            status: SessionStatus::Created,
            messages: Vec::new(),
        };

        let session_id = session.id;
        let mut sessions = self.sessions.write().await;
        sessions.insert(session_id, session);

        Ok(session_id)
    }

    pub async fn start_session(&self, session_id: &Uuid) -> Result<()> {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.status = SessionStatus::Running;
            Ok(())
        } else {
            anyhow::bail!("Session not found: {}", session_id)
        }
    }

    pub async fn stop_session(&self, session_id: &Uuid) -> Result<()> {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.status = SessionStatus::Completed;
            Ok(())
        } else {
            anyhow::bail!("Session not found: {}", session_id)
        }
    }

    pub async fn add_message(&self, session_id: &Uuid, message: CollabMessage) -> Result<()> {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.messages.push(message);
            Ok(())
        } else {
            anyhow::bail!("Session not found: {}", session_id)
        }
    }

    pub async fn get_session(&self, session_id: &Uuid) -> Result<CollaborationSession> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", session_id))
    }

    pub async fn list_sessions(&self) -> Vec<CollaborationSession> {
        let sessions = self.sessions.read().await;
        sessions.values().cloned().collect()
    }
}
