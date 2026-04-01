use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::utils::fs as fs_utils;

/// 一个简单的 Skill 抽象，用来承载从 `claude.md` 等文件中解析出来的能力描述。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub examples: Vec<String>,
}

/// Skill 管理器：负责从 workspace/项目中发现并管理 skills。
pub struct SkillManager {
    skills: Vec<Skill>,
}

impl SkillManager {
    pub fn new(skills: Vec<Skill>) -> Self {
        Self { skills }
    }

    pub fn empty() -> Self {
        Self { skills: Vec::new() }
    }

    pub fn skills(&self) -> &[Skill] {
        &self.skills
    }

    /// 从指定路径的 `claude.md` 解析 skills。
    pub fn from_claude_md(path: &Path) -> Result<Self> {
        let content = fs_utils::read_file_to_string(path)?;
        let skills = parse_claude_md(&content);
        Ok(SkillManager::new(skills))
    }

    /// 在 workspace 或当前目录中自动发现 `claude.md`。
    ///
    /// 优先顺序：
    /// 1. `<workspace_path>/claude.md`
    /// 2. `./claude.md`
    pub fn discover(workspace_path: &str) -> Result<Self> {
        let workspace_path = expand_tilde(workspace_path);
        let workspace_claude = workspace_path.join("claude.md");

        if fs_utils::file_exists(&workspace_claude) {
            return Self::from_claude_md(&workspace_claude);
        }

        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let project_claude = cwd.join("claude.md");
        if fs_utils::file_exists(&project_claude) {
            return Self::from_claude_md(&project_claude);
        }

        Ok(SkillManager::empty())
    }
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    }
    PathBuf::from(path)
}

/// 极简的 `claude.md` 解析器：
/// - 把以 `## ` 开头的二级标题视作一个 Skill 名称
/// - 标题下连续的非空行合并为 description
/// - 以 `- ` 开头的行视作 examples
fn parse_claude_md(content: &str) -> Vec<Skill> {
    let mut skills = Vec::new();

    let mut current_name: Option<String> = None;
    let mut desc_lines: Vec<String> = Vec::new();
    let mut examples: Vec<String> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("## ") {
            // 收尾上一段 skill
            if let Some(name) = current_name.take() {
                let description = desc_lines.join("\n").trim().to_string();
                skills.push(Skill {
                    name,
                    description,
                    examples: examples.clone(),
                });
                desc_lines.clear();
                examples.clear();
            }

            current_name = Some(trimmed.trim_start_matches("## ").trim().to_string());
        } else if trimmed.starts_with("- ") {
            examples.push(trimmed.trim_start_matches("- ").trim().to_string());
        } else if !trimmed.is_empty() {
            desc_lines.push(trimmed.to_string());
        }
    }

    if let Some(name) = current_name {
        let description = desc_lines.join("\n").trim().to_string();
        skills.push(Skill {
            name,
            description,
            examples,
        });
    }

    skills
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_CLAUDE_MD: &str = r#"
# Numina Skills

## Code Review
对代码进行全面审查，包括逻辑正确性、安全漏洞、性能问题和代码风格。
- 检查潜在的 SQL 注入、XSS、CSRF 等安全问题
- 分析时间复杂度和空间复杂度

## Refactor
将现有代码重构为更清晰、可维护的结构，遵循 SOLID 原则。

## Write Tests
为给定代码生成单元测试和集成测试。
- 覆盖正常路径、边界条件和错误路径
"#;

    #[test]
    fn test_parse_claude_md_skill_count() {
        let skills = parse_claude_md(SAMPLE_CLAUDE_MD);
        assert_eq!(skills.len(), 3, "应该解析出 3 个 skill");
    }

    #[test]
    fn test_parse_claude_md_skill_names() {
        let skills = parse_claude_md(SAMPLE_CLAUDE_MD);
        assert_eq!(skills[0].name, "Code Review");
        assert_eq!(skills[1].name, "Refactor");
        assert_eq!(skills[2].name, "Write Tests");
    }

    #[test]
    fn test_parse_claude_md_description() {
        let skills = parse_claude_md(SAMPLE_CLAUDE_MD);
        assert!(
            skills[0].description.contains("逻辑正确性"),
            "Code Review 的 description 应包含 '逻辑正确性'"
        );
    }

    #[test]
    fn test_parse_claude_md_examples() {
        let skills = parse_claude_md(SAMPLE_CLAUDE_MD);
        assert_eq!(skills[0].examples.len(), 2, "Code Review 应有 2 个 example");
        assert!(skills[0].examples[0].contains("SQL 注入"));
    }

    #[test]
    fn test_parse_empty_content() {
        let skills = parse_claude_md("");
        assert!(skills.is_empty(), "空内容应返回空 skills");
    }

    #[test]
    fn test_skill_manager_empty() {
        let mgr = SkillManager::empty();
        assert!(mgr.skills().is_empty());
    }

    #[test]
    fn test_skill_manager_from_content() {
        let skills = parse_claude_md(SAMPLE_CLAUDE_MD);
        let mgr = SkillManager::new(skills);
        assert_eq!(mgr.skills().len(), 3);
    }
}
