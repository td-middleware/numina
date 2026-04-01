pub mod parser;
pub mod executor;
pub mod optimizer;

pub use parser::PlanParser;
pub use executor::PlanExecutor;
pub use optimizer::PlanOptimizer;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<PlanStep>,
    pub status: PlanStatus,
    pub metadata: PlanMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub step_type: StepType,
    pub depends_on: Vec<Uuid>,
    pub config: StepConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepType {
    AgentTask,
    ToolCall,
    Parallel,
    Sequence,
    Condition,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepConfig {
    pub retry_count: u32,
    pub timeout_seconds: u64,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PlanStatus {
    Draft,
    Ready,
    Running,
    Paused,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanMetadata {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub created_by: String,
    pub tags: Vec<String>,
}

impl Plan {
    pub fn new(name: String) -> Self {
        let now = chrono::Utc::now();
        Self {
            id: Uuid::new_v4(),
            name,
            description: None,
            steps: Vec::new(),
            status: PlanStatus::Draft,
            metadata: PlanMetadata {
                created_at: now,
                updated_at: now,
                created_by: "user".to_string(),
                tags: Vec::new(),
            },
        }
    }

    pub fn add_step(&mut self, step: PlanStep) -> &mut Self {
        self.steps.push(step);
        self
    }
}
