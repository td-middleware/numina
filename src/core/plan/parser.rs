use anyhow::Result;
use super::Plan;

pub struct PlanParser;

impl PlanParser {
    pub fn parse(content: &str) -> Result<Plan> {
        // Simplified parser - in production, parse TOML/YAML/JSON
        if content.trim().is_empty() {
            anyhow::bail!("Plan content cannot be empty");
        }

        let mut plan = Plan::new("Parsed Plan".to_string());
        plan.description = Some(content.to_string());
        
        Ok(plan)
    }

    pub fn parse_file(path: &str) -> Result<Plan> {
        let content = std::fs::read_to_string(path)?;
        Self::parse(&content)
    }
}
