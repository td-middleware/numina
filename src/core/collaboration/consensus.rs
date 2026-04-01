use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Vote {
    pub agent_id: Uuid,
    pub choice: VoteChoice,
    pub confidence: f32,
}

#[derive(Debug, Clone)]
pub enum VoteChoice {
    Approve,
    Reject,
    Abstain,
}

pub struct ConsensusEngine {
    threshold: f32,
}

impl ConsensusEngine {
    pub fn new(threshold: f32) -> Self {
        Self { threshold }
    }

    pub fn evaluate(&self, votes: &[Vote]) -> ConsensusResult {
        if votes.is_empty() {
            return ConsensusResult::Failed("No votes".to_string());
        }

        let approve_count = votes.iter().filter(|v| matches!(v.choice, VoteChoice::Approve)).count();
        let total_voters = votes.len();
        
        let approval_ratio = approve_count as f32 / total_voters as f32;

        if approval_ratio >= self.threshold {
            ConsensusResult::Reached {
                approval_ratio,
                votes: votes.to_vec(),
            }
        } else {
            ConsensusResult::Failed(format!("Approval ratio {:.2} below threshold {:.2}", 
                                           approval_ratio, self.threshold))
        }
    }

    pub async fn collect_consensus(&self, agent_ids: &[Uuid], proposal: &str) -> Vec<Vote> {
        agent_ids
            .iter()
            .map(|id| Vote {
                agent_id: *id,
                choice: VoteChoice::Approve,
                confidence: 0.9,
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
pub enum ConsensusResult {
    Reached {
        approval_ratio: f32,
        votes: Vec<Vote>,
    },
    Failed(String),
}
