pub mod parser;
pub mod validator;
pub mod models;
pub mod mcp;

pub use parser::ConfigParser;
pub use validator::ConfigValidator;
pub use models::{ModelsConfig, ModelEntry};
pub use mcp::{McpConfig as McpFileConfig, McpServerEntry};

use serde::{Deserialize, Serialize};
use anyhow::Result;

/// 主配置文件结构（~/.numina/config.toml）
/// 存储全局设置；模型列表和 MCP server 列表分别存储在独立 JSON 文件中
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NuminaConfig {
    pub general: GeneralConfig,
    pub model: ModelConfig,
    pub collaboration: CollaborationConfig,
    pub mcp_global: McpGlobalConfig,
    pub workspace: WorkspaceConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    pub version: String,
    pub log_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub default_model: String,
    pub temperature: f32,
    pub max_tokens: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollaborationConfig {
    pub timeout_seconds: u64,
    pub max_parallel_agents: usize,
    pub consensus_required: bool,
}

/// 全局 MCP 设置（存在 config.toml 中）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpGlobalConfig {
    pub auto_connect: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    pub path: String,
    pub max_memory_mb: usize,
}

impl Default for NuminaConfig {
    fn default() -> Self {
        Self {
            general: GeneralConfig {
                version: "0.1.0".to_string(),
                log_level: "info".to_string(),
            },
            model: ModelConfig {
                default_model: "gpt-4o".to_string(),
                temperature: 0.7,
                max_tokens: 4096,
            },
            collaboration: CollaborationConfig {
                timeout_seconds: 300,
                max_parallel_agents: 5,
                consensus_required: false,
            },
            mcp_global: McpGlobalConfig {
                auto_connect: false,
            },
            workspace: WorkspaceConfig {
                path: "~/.numina/workspace".to_string(),
                max_memory_mb: 1024,
            },
        }
    }
}

impl NuminaConfig {
    /// 配置文件路径：~/.numina/config.toml
    pub fn config_path() -> Result<std::path::PathBuf> {
        let path = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?
            .join(".numina")
            .join("config.toml");
        Ok(path)
    }

    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;
        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            // 兼容旧格式（含 mcp 字段）
            let config: Self = toml::from_str(&content).unwrap_or_else(|_| Self::default());
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self) -> Result<()> {
        let config_dir = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?
            .join(".numina");
        std::fs::create_dir_all(&config_dir)?;
        let config_path = config_dir.join("config.toml");
        let content = ConfigParser::serialize(self)?;
        std::fs::write(config_path, content)?;
        Ok(())
    }

    /// 获取当前激活的默认模型名称
    /// 优先读取 models.json 中的 active 字段
    pub fn active_model(&self) -> String {
        if let Ok(mc) = ModelsConfig::load() {
            let active = mc.active_model();
            if !active.is_empty() {
                return active.to_string();
            }
        }
        self.model.default_model.clone()
    }

    /// 初始化所有配置文件（config.toml + models.json + mcp.json）
    pub fn init_all() -> Result<()> {
        let config_dir = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?
            .join(".numina");
        std::fs::create_dir_all(&config_dir)?;

        // 初始化主配置
        let config_path = config_dir.join("config.toml");
        if !config_path.exists() {
            let default = Self::default();
            default.save()?;
        }

        // 初始化 models.json
        ModelsConfig::init_if_missing()?;

        // 初始化 mcp.json
        McpFileConfig::init_if_missing()?;

        Ok(())
    }
}
