/// 模型配置文件管理
/// 独立的 JSON 配置文件：~/.numina/models.json
/// 支持直接编辑 JSON 文件或通过命令行操作，两种方式均可生效

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 模型配置文件的完整结构（~/.numina/models.json）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelsConfig {
    /// 当前激活的默认模型名称
    #[serde(default)]
    pub active: String,
    /// 所有已注册的模型列表
    #[serde(default)]
    pub models: Vec<ModelEntry>,
}

/// 单个模型的配置条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    /// 模型名称/ID（如 gpt-4o, claude-3-5-sonnet-20241022）
    pub name: String,
    /// 提供商：openai | anthropic | local
    pub provider: String,
    /// API endpoint（可选，OpenAI 兼容接口用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// API key（明文存储，生产环境建议用环境变量）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// 模型描述（可选）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 覆盖全局 temperature
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// 覆盖全局 max_tokens
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
}

impl ModelsConfig {
    /// 配置文件路径：~/.numina/models.json
    pub fn config_path() -> Result<PathBuf> {
        let path = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?
            .join(".numina")
            .join("models.json");
        Ok(path)
    }

    /// 加载配置文件，不存在则返回默认空配置
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let config: Self = serde_json::from_str(&content)
                .map_err(|e| anyhow::anyhow!("Failed to parse {}: {}", path.display(), e))?;
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    /// 保存配置文件
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        // 确保目录存在
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// 初始化默认配置文件（如果不存在）
    pub fn init_if_missing() -> Result<bool> {
        let path = Self::config_path()?;
        if path.exists() {
            return Ok(false);
        }
        let default_config = Self {
            active: String::new(),
            models: vec![],
        };
        default_config.save()?;
        Ok(true)
    }

    /// 获取当前激活模型名称
    pub fn active_model(&self) -> &str {
        if !self.active.is_empty() {
            return &self.active;
        }
        // 回退：找第一个模型
        self.models.first().map(|m| m.name.as_str()).unwrap_or("gpt-4o")
    }

    /// 设置激活模型
    pub fn set_active(&mut self, name: &str) -> bool {
        if self.models.iter().any(|m| m.name == name) {
            self.active = name.to_string();
            true
        } else {
            false
        }
    }
}
