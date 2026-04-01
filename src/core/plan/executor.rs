use anyhow::Result;
use super::{Plan, PlanStatus};
use uuid::Uuid;

pub struct PlanExecutor {
    plan: Plan,
}

impl PlanExecutor {
    pub fn new(plan: Plan) -> Self {
        Self { plan }
    }

    pub async fn execute(&mut self, dry_run: bool) -> Result<Vec<PlanExecutionResult>> {
        self.plan.status = PlanStatus::Running;
        
        let mut results = Vec::new();
        
        for step in &self.plan.steps {
            let result = if dry_run {
                PlanExecutionResult {
                    step_id: step.id,
                    success: true,
                    output: format!("Dry run: would execute {}", step.name),
                    duration_ms: 0,
                }
            } else {
                self.execute_step(step).await?
            };
            
            results.push(result);
        }
        
        self.plan.status = PlanStatus::Completed;
        Ok(results)
    }

    async fn execute_step(&self, step: &super::PlanStep) -> Result<PlanExecutionResult> {
        // Simplified execution
        Ok(PlanExecutionResult {
            step_id: step.id,
            success: true,
            output: format!("Executed: {}", step.name),
            duration_ms: 100,
        })
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }
}

#[derive(Debug)]
pub struct PlanExecutionResult {
    pub step_id: Uuid,
    pub success: bool,
    pub output: String,
    pub duration_ms: u64,
}
