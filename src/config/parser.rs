use anyhow::Result;
use super::NuminaConfig;

pub struct ConfigParser;

impl ConfigParser {
    pub fn parse(content: &str) -> Result<NuminaConfig> {
        toml::from_str(content)
            .map_err(|e| anyhow::anyhow!("Failed to parse config: {}", e))
    }

    pub fn serialize(config: &NuminaConfig) -> Result<String> {
        toml::to_string_pretty(config)
            .map_err(|e| anyhow::anyhow!("Failed to serialize config: {}", e))
    }
}
