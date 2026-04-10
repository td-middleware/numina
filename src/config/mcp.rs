/// MCP server 配置文件管理
/// 独立的 JSON 配置文件：~/.numina/mcp.json
/// 支持直接编辑 JSON 文件或通过命令行操作，两种方式均可生效

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// MCP 配置文件的完整结构（~/.numina/mcp.json）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpConfig {
    /// 是否在启动时自动连接所有已启用的 server
    #[serde(default)]
    pub auto_connect: bool,
    /// 所有已注册的 MCP server 列表
    #[serde(default)]
    pub servers: Vec<McpServerEntry>,
}

/// 单个 MCP server 的配置条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerEntry {
    /// server 名称（唯一标识）
    pub name: String,
    /// server 类型：stdio | http | websocket
    #[serde(default = "default_stdio")]
    pub server_type: String,
    /// 命令（stdio 类型）或 URL（http/websocket 类型）
    pub command_or_url: String,
    /// stdio 类型的额外参数（空格分隔字符串）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    /// 是否启用（默认 true）
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 描述（可选）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 环境变量列表（key=value 格式）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,
}

fn default_stdio() -> String {
    "stdio".to_string()
}

fn default_true() -> bool {
    true
}

impl McpConfig {
    /// 配置文件路径：~/.numina/mcp.json
    pub fn config_path() -> Result<PathBuf> {
        let path = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?
            .join(".numina")
            .join("mcp.json");
        Ok(path)
    }

    /// 加载配置文件，不存在则返回默认空配置
    /// 支持多种格式（按优先级）：
    /// 1. VSCode 格式: { "mcp_servers": { "name": { "type": "http", "url": "...", "headers": {...} } } }
    /// 2. VSCode 格式: { "mcpServers": { "name": { "command": "...", "args": [...] } } }
    /// 3. Numina 格式: { "servers": [...] }
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(&path)?;
        let json: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse {}: {}", path.display(), e))?;

        // 优先检查 mcp_servers（下划线格式，用户配置的格式）
        if let Some(mcp_servers) = json.get("mcp_servers").and_then(|v| v.as_object()) {
            let mut config = Self::default();
            let (added, _updated, _skipped) =
                config.merge_from_vscode_format(mcp_servers, true)?;
            tracing::debug!("Loaded {} MCP servers from mcp_servers format", added);
            return Ok(config);
        }

        // 检查 mcpServers（驼峰格式，Claude Desktop / VSCode 标准格式）
        if let Some(mcp_servers) = json.get("mcpServers").and_then(|v| v.as_object()) {
            let mut config = Self::default();
            let (added, _updated, _skipped) =
                config.merge_from_vscode_format(mcp_servers, true)?;
            tracing::debug!("Loaded {} MCP servers from mcpServers format", added);
            return Ok(config);
        }

        // 检查 Numina 原生格式（servers 数组非空）
        if let Some(servers) = json.get("servers").and_then(|v| v.as_array()) {
            if !servers.is_empty() {
                if let Ok(config) = serde_json::from_value::<Self>(json.clone()) {
                    return Ok(config);
                }
            }
        }

        // 最后尝试直接反序列化（兼容旧格式）
        if let Ok(config) = serde_json::from_value::<Self>(json) {
            return Ok(config);
        }

        tracing::warn!("Unknown MCP config format in {}, using empty config", path.display());
        Ok(Self::default())
    }

    /// 保存配置文件
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
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
        let default_config = Self::default();
        default_config.save()?;
        Ok(true)
    }

    /// 获取所有已启用的 server
    pub fn enabled_servers(&self) -> Vec<&McpServerEntry> {
        self.servers.iter().filter(|s| s.enabled).collect()
    }

    /// 解析 VSCode-style MCP 配置格式并合并到当前配置
    /// 格式：{ "mcpServers": { "name": { "command": "...", "args": [...], "env": {...} } } }
    pub fn merge_from_vscode_format(
        &mut self,
        mcp_servers: &serde_json::Map<String, serde_json::Value>,
        overwrite: bool,
    ) -> Result<(usize, usize, usize)> {
        let mut added = 0usize;
        let mut updated = 0usize;
        let mut skipped = 0usize;

        for (name, val) in mcp_servers {
            let entry = parse_vscode_server_entry(name, val)?;
            let existing_idx = self.servers.iter().position(|s| s.name == entry.name);
            match existing_idx {
                Some(idx) if overwrite => {
                    self.servers[idx] = entry;
                    updated += 1;
                }
                Some(_) => {
                    skipped += 1;
                }
                None => {
                    self.servers.push(entry);
                    added += 1;
                }
            }
        }

        Ok((added, updated, skipped))
    }
}

/// 解析单个 VSCode-style server 条目
/// 支持两种格式：
/// - 格式1: { "command": "...", "args": [...], "env": {...} }
/// - 格式2: { "type": "http", "url": "...", "headers": {...} }
pub fn parse_vscode_server_entry(
    name: &str,
    val: &serde_json::Value,
) -> Result<McpServerEntry> {
    let obj = val.as_object().ok_or_else(|| {
        anyhow::anyhow!("MCP server '{}' config must be an object", name)
    })?;

    // 优先检查 format 2: type + url + headers
    if let (Some(type_val), Some(url_val)) = (obj.get("type"), obj.get("url")) {
        let server_type = type_val.as_str().unwrap_or("http").to_string();
        let url = url_val.as_str().ok_or_else(|| {
            anyhow::anyhow!("MCP server '{}' url must be a string", name)
        })?;

        // headers → env (key=value 格式)
        let env: Vec<String> = obj
            .get("headers")
            .and_then(|v| v.as_object())
            .map(|headers| {
                headers
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, v.as_str().unwrap_or("")))
                    .collect()
            })
            .unwrap_or_default();

        let description = obj
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        return Ok(McpServerEntry {
            name: name.to_string(),
            server_type,
            command_or_url: url.to_string(),
            args: None,
            enabled: true,
            description,
            env,
        });
    }

    // 回退到 format 1: command + args + env
    let (server_type, command_or_url) =
        if let Some(cmd) = obj.get("command").and_then(|v| v.as_str()) {
            ("stdio".to_string(), cmd.to_string())
        } else if let Some(url) = obj.get("url").and_then(|v| v.as_str()) {
            let stype = if url.starts_with("ws://") || url.starts_with("wss://") {
                "websocket"
            } else {
                "http"
            };
            (stype.to_string(), url.to_string())
        } else {
            return Err(anyhow::anyhow!(
                "MCP server '{}' must have either 'command' or 'url' field",
                name
            ));
        };

    // args 数组 → 空格拼接字符串
    let args = obj.get("args").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|a| a.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    });

    // env 对象 → key=value 列表
    let env: Vec<String> = obj
        .get("env")
        .and_then(|v| v.as_object())
        .map(|env_map| {
            env_map
                .iter()
                .map(|(k, v)| format!("{}={}", k, v.as_str().unwrap_or("")))
                .collect()
        })
        .unwrap_or_default();

    let description = obj
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(McpServerEntry {
        name: name.to_string(),
        server_type,
        command_or_url,
        args,
        enabled: true,
        description,
        env,
    })
}
