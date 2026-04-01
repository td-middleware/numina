use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use super::CollabMessage;
use uuid::Uuid;
use anyhow::Result;

pub struct MessageBus {
    channels: Arc<RwLock<HashMap<Uuid, broadcast::Sender<CollabMessage>>>>,
    capacity: usize,
}

impl MessageBus {
    pub fn new(capacity: usize) -> Self {
        Self {
            channels: Arc::new(RwLock::new(HashMap::new())),
            capacity,
        }
    }

    pub async fn create_channel(&self, session_id: Uuid) -> Result<broadcast::Sender<CollabMessage>> {
        let (tx, _rx) = broadcast::channel(self.capacity);
        let mut channels = self.channels.write().await;
        channels.insert(session_id, tx.clone());
        Ok(tx)
    }

    pub async fn subscribe(&self, session_id: &Uuid) -> Result<broadcast::Receiver<CollabMessage>> {
        let channels = self.channels.read().await;
        channels
            .get(session_id)
            .map(|tx| tx.subscribe())
            .ok_or_else(|| anyhow::anyhow!("Channel not found for session: {}", session_id))
    }

    pub async fn publish(&self, session_id: &Uuid, message: CollabMessage) -> Result<()> {
        let channels = self.channels.read().await;
        if let Some(tx) = channels.get(session_id) {
            let _ = tx.send(message);
            Ok(())
        } else {
            anyhow::bail!("Channel not found for session: {}", session_id)
        }
    }

    pub async fn remove_channel(&self, session_id: &Uuid) {
        let mut channels = self.channels.write().await;
        channels.remove(session_id);
    }
}
