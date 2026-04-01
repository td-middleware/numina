use super::NuminaConfig;

pub struct ConfigValidator;

impl ConfigValidator {
    pub fn validate(config: &NuminaConfig) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if config.model.temperature < 0.0 || config.model.temperature > 2.0 {
            errors.push("Temperature must be between 0.0 and 2.0".to_string());
        }

        if config.model.max_tokens == 0 {
            errors.push("Max tokens must be greater than 0".to_string());
        }

        if config.collaboration.max_parallel_agents == 0 {
            errors.push("Max parallel agents must be greater than 0".to_string());
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}
