use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ─────────────────────────────────────────────
// Skill 数据结构
// ─────────────────────────────────────────────

/// 一个 Skill 的完整定义（从 SKILL.md 或 claude.md 解析）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Slash 命令名（如 "code-review" → `/code-review`）
    pub name: String,
    /// 简短描述（显示在 /help 和补全列表中）
    pub description: String,
    /// 何时使用（注入 system prompt 的提示）
    pub when_to_use: Option<String>,
    /// 参数提示（如 "<file> [options]"）
    pub argument_hint: Option<String>,
    /// Skill 的完整 Markdown 内容（调用时作为 prompt 注入）
    pub content: String,
    /// 来源目录（用于 ${SKILL_DIR} 替换）
    pub base_dir: Option<PathBuf>,
    /// 加载来源
    pub loaded_from: SkillSource,
    /// 示例（从 claude.md 的 `- ` 列表解析）
    pub examples: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SkillSource {
    /// ~/.numina/skills/<name>/SKILL.md
    Global,
    /// .numina/skills/<name>/SKILL.md（项目级）
    Project,
    /// claude.md 中的 ## 二级标题
    ClaudeMd,
    /// 内置 skill
    Bundled,
}

impl Skill {
    /// 将 skill 内容展开为最终 prompt（替换 $ARGUMENT 等占位符）
    pub fn expand_prompt(&self, args: &str) -> String {
        let mut content = self.content.clone();

        // 替换 ${SKILL_DIR} 为 base_dir
        if let Some(ref dir) = self.base_dir {
            let dir_str = dir.to_string_lossy();
            content = content.replace("${SKILL_DIR}", &dir_str);
            content = content.replace("${CLAUDE_SKILL_DIR}", &dir_str);
        }

        // 替换 $ARGUMENT（整体参数）
        content = content.replace("$ARGUMENT", args);
        content = content.replace("${ARGUMENT}", args);

        // 替换 $1, $2, ... 位置参数
        let parts: Vec<&str> = args.splitn(10, ' ').collect();
        for (i, part) in parts.iter().enumerate() {
            content = content.replace(&format!("${}", i + 1), part);
            content = content.replace(&format!("${{{}}}", i + 1), part);
        }

        // 如果有 base_dir，在 prompt 前加上目录提示
        if let Some(ref dir) = self.base_dir {
            format!("Base directory for this skill: {}\n\n{}", dir.display(), content)
        } else {
            content
        }
    }

    /// 估算 token 数（用于 system prompt 截断判断）
    pub fn estimate_tokens(&self) -> usize {
        let text = format!("{} {} {}",
            self.name,
            self.description,
            self.when_to_use.as_deref().unwrap_or("")
        );
        text.len() / 4 + 1
    }
}

// ─────────────────────────────────────────────
// YAML Frontmatter 解析
// ─────────────────────────────────────────────

/// SKILL.md 的 YAML frontmatter 字段
#[derive(Debug, Default)]
struct SkillFrontmatter {
    description: Option<String>,
    when_to_use: Option<String>,
    argument_hint: Option<String>,
    name: Option<String>,
}

/// 解析 SKILL.md 文件：分离 frontmatter 和 markdown 内容
fn parse_skill_md(raw: &str) -> (SkillFrontmatter, String) {
    let mut fm = SkillFrontmatter::default();

    // 检测 YAML frontmatter（--- 开头）
    if raw.starts_with("---") {
        let rest = &raw[3..];
        if let Some(end) = rest.find("\n---") {
            let yaml_str = &rest[..end];
            let content = rest[end + 4..].trim_start_matches('\n').to_string();
            parse_yaml_frontmatter(yaml_str, &mut fm);
            return (fm, content);
        }
    }

    (fm, raw.to_string())
}

/// 极简 YAML 解析（只处理 key: value 格式，不依赖外部 crate）
fn parse_yaml_frontmatter(yaml: &str, fm: &mut SkillFrontmatter) {
    for line in yaml.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(colon_pos) = line.find(':') {
            let key = line[..colon_pos].trim().to_lowercase();
            let value = line[colon_pos + 1..].trim().trim_matches('"').trim_matches('\'').to_string();
            if value.is_empty() {
                continue;
            }
            match key.as_str() {
                "description" => fm.description = Some(value),
                "when_to_use" | "when-to-use" => fm.when_to_use = Some(value),
                "argument-hint" | "argument_hint" => fm.argument_hint = Some(value),
                "name" => fm.name = Some(value),
                _ => {}
            }
        }
    }
}

/// 从 Markdown 内容中提取第一段作为描述（fallback）
fn extract_description_from_markdown(content: &str) -> String {
    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') && !trimmed.starts_with("---") {
            let desc: String = trimmed.chars().take(120).collect();
            return if trimmed.len() > 120 { format!("{}…", desc) } else { desc };
        }
    }
    String::new()
}

// ─────────────────────────────────────────────
// Skills 目录加载
// ─────────────────────────────────────────────

/// 从 `<base>/skills/<name>/SKILL.md` 格式加载所有 skills
fn load_skills_from_dir(base: &Path, source: SkillSource) -> Vec<Skill> {
    let skills_dir = base.join("skills");
    if !skills_dir.is_dir() {
        return vec![];
    }

    let entries = match std::fs::read_dir(&skills_dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    let mut skills = Vec::new();

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let skill_file = path.join("SKILL.md");
        if !skill_file.exists() {
            // 也尝试 skill.md（小写）
            let lower = path.join("skill.md");
            if !lower.exists() {
                continue;
            }
        }

        let skill_file = if path.join("SKILL.md").exists() {
            path.join("SKILL.md")
        } else {
            path.join("skill.md")
        };

        let raw = match std::fs::read_to_string(&skill_file) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let skill_name = path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        if skill_name.is_empty() || skill_name.starts_with('.') {
            continue;
        }

        let (fm, content) = parse_skill_md(&raw);

        let description = fm.description
            .unwrap_or_else(|| extract_description_from_markdown(&content));

        let display_name = fm.name.unwrap_or_else(|| skill_name.clone());

        skills.push(Skill {
            name: display_name,
            description,
            when_to_use: fm.when_to_use,
            argument_hint: fm.argument_hint,
            content,
            base_dir: Some(path.clone()),
            loaded_from: source.clone(),
            examples: vec![],
        });
    }

    // 按名称排序
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// 从 claude.md 文件解析 skills（## 二级标题格式）
fn load_skills_from_claude_md(path: &Path) -> Vec<Skill> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    parse_claude_md(&content)
}

/// 解析 claude.md 格式：## 标题 → skill name，正文 → description，`- ` → examples
fn parse_claude_md(content: &str) -> Vec<Skill> {
    let mut skills = Vec::new();
    let mut current_name: Option<String> = None;
    let mut desc_lines: Vec<String> = Vec::new();
    let mut examples: Vec<String> = Vec::new();
    let mut content_lines: Vec<String> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("## ") {
            // 收尾上一段 skill
            if let Some(name) = current_name.take() {
                let description = desc_lines.join(" ").trim().to_string();
                let full_content = content_lines.join("\n").trim().to_string();
                skills.push(Skill {
                    name: name.to_lowercase().replace(' ', "-"),
                    description,
                    when_to_use: None,
                    argument_hint: None,
                    content: full_content,
                    base_dir: None,
                    loaded_from: SkillSource::ClaudeMd,
                    examples: examples.clone(),
                });
                desc_lines.clear();
                examples.clear();
                content_lines.clear();
            }
            current_name = Some(trimmed.trim_start_matches("## ").trim().to_string());
        } else if trimmed.starts_with("- ") {
            // 只有在 skill 上下文中才处理列表项
            if current_name.is_some() {
                let ex = trimmed.trim_start_matches("- ").trim().to_string();
                examples.push(ex.clone());
                content_lines.push(line.to_string());
            }
        } else if !trimmed.is_empty() {
            // 只有在 skill 上下文中才处理描述和内容
            if current_name.is_some() {
                if desc_lines.is_empty() {
                    desc_lines.push(trimmed.to_string());
                }
                content_lines.push(line.to_string());
            }
        }
    }

    if let Some(name) = current_name {
        let description = desc_lines.join(" ").trim().to_string();
        let full_content = content_lines.join("\n").trim().to_string();
        skills.push(Skill {
            name: name.to_lowercase().replace(' ', "-"),
            description,
            when_to_use: None,
            argument_hint: None,
            content: full_content,
            base_dir: None,
            loaded_from: SkillSource::ClaudeMd,
            examples,
        });
    }

    skills
}

// ─────────────────────────────────────────────
// SkillManager
// ─────────────────────────────────────────────

/// Skill 管理器：负责发现、加载和查找 skills
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

    pub fn count(&self) -> usize {
        self.skills.len()
    }

    /// 按名称查找 skill（精确匹配）
    pub fn find(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.name == name)
    }

    /// 检查输入是否是一个 skill 调用（以 / 开头且匹配已知 skill 名）
    /// 返回 (skill, args)
    pub fn match_slash_command<'a>(&'a self, input: &str) -> Option<(&'a Skill, String)> {
        if !input.starts_with('/') {
            return None;
        }
        let without_slash = &input[1..];
        let (cmd, args) = match without_slash.find(' ') {
            Some(pos) => (&without_slash[..pos], without_slash[pos + 1..].trim().to_string()),
            None => (without_slash, String::new()),
        };
        self.find(cmd).map(|s| (s, args))
    }

    /// 生成注入 system prompt 的 skills 描述块（简单模式，仅列出名称和描述）
    pub fn system_prompt_block(&self) -> String {
        if self.skills.is_empty() {
            return String::new();
        }

        let mut lines = vec![
            "## Available Skills".to_string(),
            String::new(),
            "You have access to the following slash commands (skills):".to_string(),
            String::new(),
        ];

        for skill in &self.skills {
            let arg_hint = skill.argument_hint.as_deref().unwrap_or("");
            if arg_hint.is_empty() {
                lines.push(format!("- `/{}`  — {}", skill.name, skill.description));
            } else {
                lines.push(format!("- `/{} {}`  — {}", skill.name, arg_hint, skill.description));
            }
            if let Some(ref wtu) = skill.when_to_use {
                lines.push(format!("  When to use: {}", wtu));
            }
        }

        lines.push(String::new());
        lines.push("When the user invokes a skill with `/skill-name [args]`, execute the skill's instructions with the provided arguments.".to_string());

        lines.join("\n")
    }

    /// 【方案一】生成轻量摘要块（只含 name + description + when_to_use 关键词）
    /// 用于 system prompt 固定注入，token 消耗极低（~50 tokens/skill）
    pub fn summary_prompt_block(&self) -> String {
        if self.skills.is_empty() {
            return String::new();
        }

        let auto_skills: Vec<&Skill> = self.skills.iter()
            .filter(|s| s.when_to_use.is_some())
            .collect();
        let manual_skills: Vec<&Skill> = self.skills.iter()
            .filter(|s| s.when_to_use.is_none())
            .collect();

        let mut lines: Vec<String> = Vec::new();

        if !auto_skills.is_empty() {
            lines.push("## Available Skills (Intent-Triggered)".to_string());
            lines.push(String::new());
            lines.push("When user intent matches a skill's trigger condition, the system will automatically inject the full skill instructions. You do NOT need to ask the user to type a slash command.".to_string());
            lines.push(String::new());
            for skill in &auto_skills {
                let wtu = skill.when_to_use.as_deref().unwrap_or("");
                lines.push(format!("- **{}**: {} | Trigger: {}", skill.name, skill.description, wtu));
            }
            lines.push(String::new());
        }

        if !manual_skills.is_empty() {
            lines.push("## Manual Skills (Slash Commands)".to_string());
            lines.push(String::new());
            for skill in &manual_skills {
                let arg_hint = skill.argument_hint.as_deref().unwrap_or("");
                if arg_hint.is_empty() {
                    lines.push(format!("- `/{}`  — {}", skill.name, skill.description));
                } else {
                    lines.push(format!("- `/{} {}`  — {}", skill.name, arg_hint, skill.description));
                }
            }
            lines.push(String::new());
        }

        lines.join("\n")
    }

    /// 【方案一】根据用户输入匹配意图，返回命中的 skill 列表（按相关度排序）
    ///
    /// 匹配逻辑：
    /// 1. 把 `when_to_use` 中的中文顿号/逗号/空格分割为关键词列表
    /// 2. 对每个关键词，同时生成"后缀子词"（如"控制器告警" → 也生成"告警"）
    /// 3. 用户输入中包含任意关键词或子词则命中
    /// 4. 命中关键词越多，排序越靠前
    pub fn match_intent<'a>(&'a self, user_input: &str) -> Vec<(&'a Skill, usize)> {
        let input_lower = user_input.to_lowercase();

        let mut matches: Vec<(&Skill, usize)> = self.skills.iter()
            .filter(|s| s.when_to_use.is_some())
            .filter_map(|skill| {
                let wtu = skill.when_to_use.as_deref().unwrap_or("");

                // 分割 when_to_use 为原始关键词（支持中文顿号、逗号、空格、斜杠、竖线）
                let raw_keywords: Vec<&str> = wtu
                    .split(|c| c == '、' || c == ',' || c == ' ' || c == '/' || c == '|')
                    .map(|s| s.trim())
                    .filter(|s| {
                        let char_count = s.chars().count();
                        char_count >= 2 // 按字符数过滤（中文每字1个字符）
                    })
                    .collect();

                // 扩展关键词：对长关键词（>3个字符）提取后缀子词
                // 例如："控制器告警" → ["控制器告警", "告警"]
                //       "当用户提到控制器告警" → 跳过（太长的前缀词）
                let mut expanded: Vec<String> = Vec::new();
                for kw in &raw_keywords {
                    let chars: Vec<char> = kw.chars().collect();
                    let char_len = chars.len();
                    expanded.push(kw.to_string());
                    // 对 3-6 字符的关键词，提取后 2 个字符作为子词
                    if char_len >= 3 && char_len <= 8 {
                        // 后缀：最后2个字符
                        let suffix2: String = chars[char_len.saturating_sub(2)..].iter().collect();
                        if suffix2.chars().count() >= 2 {
                            expanded.push(suffix2);
                        }
                        // 后缀：最后3个字符（如果够长）
                        if char_len >= 4 {
                            let suffix3: String = chars[char_len.saturating_sub(3)..].iter().collect();
                            expanded.push(suffix3);
                        }
                    }
                }

                // 去重
                expanded.sort();
                expanded.dedup();

                let hit_count = expanded.iter()
                    .filter(|kw| input_lower.contains(&kw.to_lowercase()))
                    .count();

                if hit_count > 0 {
                    Some((skill, hit_count))
                } else {
                    None
                }
            })
            .collect();

        // 按命中数降序排序
        matches.sort_by(|a, b| b.1.cmp(&a.1));
        matches
    }

    /// 【方案一】为命中的 skill 生成完整内容块（按需展开，注入到本次请求）
    pub fn expand_matched_skills(&self, user_input: &str) -> String {
        let matched = self.match_intent(user_input);
        if matched.is_empty() {
            return String::new();
        }

        let mut lines: Vec<String> = Vec::new();
        lines.push("## Activated Skills (Full Instructions)".to_string());
        lines.push(String::new());
        lines.push("The following skills have been automatically activated based on your request:".to_string());
        lines.push(String::new());

        for (skill, hit_count) in &matched {
            lines.push(format!("### Skill: `{}` — {} (matched {} keyword{})",
                skill.name, skill.description, hit_count,
                if *hit_count > 1 { "s" } else { "" }
            ));
            if let Some(ref wtu) = skill.when_to_use {
                lines.push(format!("**Triggered by**: {}", wtu));
            }
            if let Some(ref hint) = skill.argument_hint {
                lines.push(format!("**Arguments**: {}", hint));
            }
            lines.push(String::new());
            lines.push("**Full Instructions**:".to_string());
            lines.push(skill.content.clone());
            lines.push(String::new());
            lines.push("---".to_string());
            lines.push(String::new());
        }

        lines.join("\n")
    }

    /// 生成自动意图触发的 skills 描述块（注入 system prompt）
    ///
    /// - 有 `when_to_use` 的 skill → **自动触发型**：注入完整内容，AI 根据用户意图自动执行
    /// - 无 `when_to_use` 的 skill → **手动型**：只列出名称和描述（需用户 `/skill-name` 调用）
    pub fn auto_trigger_prompt_block(&self) -> String {
        if self.skills.is_empty() {
            return String::new();
        }

        let auto_skills: Vec<&Skill> = self.skills.iter()
            .filter(|s| s.when_to_use.is_some())
            .collect();
        let manual_skills: Vec<&Skill> = self.skills.iter()
            .filter(|s| s.when_to_use.is_none())
            .collect();

        let mut lines: Vec<String> = Vec::new();

        // ── 自动触发型 skills ──
        if !auto_skills.is_empty() {
            lines.push("## Skills (Auto-Triggered by Intent)".to_string());
            lines.push(String::new());
            lines.push(
                "The following skills are activated **automatically** when you detect the user's intent matches. \
                You do NOT need to wait for the user to type a slash command — just execute the skill directly."
                .to_string()
            );
            lines.push(String::new());

            for skill in &auto_skills {
                let when_to_use = skill.when_to_use.as_deref().unwrap_or("");
                lines.push(format!("### Skill: `{}` — {}", skill.name, skill.description));
                lines.push(format!("**Activate when**: {}", when_to_use));
                if let Some(ref hint) = skill.argument_hint {
                    lines.push(format!("**Arguments**: {}", hint));
                }
                lines.push(String::new());
                lines.push("**Instructions**:".to_string());
                lines.push(skill.content.clone());
                lines.push(String::new());
                lines.push("---".to_string());
                lines.push(String::new());
            }
        }

        // ── 手动型 skills（仅列出，不注入完整内容）──
        if !manual_skills.is_empty() {
            lines.push("## Skills (Manual Slash Commands)".to_string());
            lines.push(String::new());
            lines.push("The following skills must be explicitly invoked with `/skill-name [args]`:".to_string());
            lines.push(String::new());

            for skill in &manual_skills {
                let arg_hint = skill.argument_hint.as_deref().unwrap_or("");
                if arg_hint.is_empty() {
                    lines.push(format!("- `/{}`  — {}", skill.name, skill.description));
                } else {
                    lines.push(format!("- `/{} {}`  — {}", skill.name, arg_hint, skill.description));
                }
            }
            lines.push(String::new());
        }

        lines.join("\n")
    }

    /// 从多个目录发现并加载所有 skills
    ///
    /// 优先级（高→低）：
    /// 1. 项目级 `.numina/skills/`（当前目录）
    /// 2. 全局 `~/.numina/skills/`
    /// 3. workspace `claude.md`
    /// 4. 当前目录 `claude.md`
    pub fn discover(workspace_path: &str) -> Result<Self> {
        let mut all_skills: Vec<Skill> = Vec::new();
        let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();

        let add_skills = |all: &mut Vec<Skill>, seen: &mut std::collections::HashSet<String>, new_skills: Vec<Skill>| {
            for skill in new_skills {
                if !seen.contains(&skill.name) {
                    seen.insert(skill.name.clone());
                    all.push(skill);
                }
            }
        };

        // 1. 项目级 .numina/skills/
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let project_skills_base = cwd.join(".numina");
        if project_skills_base.is_dir() {
            let skills = load_skills_from_dir(&project_skills_base, SkillSource::Project);
            add_skills(&mut all_skills, &mut seen_names, skills);
        }

        // 2. 全局 ~/.numina/skills/
        let global_base = expand_tilde(workspace_path);
        // workspace_path 通常是 ~/.numina/workspace，取其父目录 ~/.numina
        let numina_home = global_base.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| global_base.clone());
        let global_skills = load_skills_from_dir(&numina_home, SkillSource::Global);
        add_skills(&mut all_skills, &mut seen_names, global_skills);

        // 3. workspace claude.md
        let workspace_claude = global_base.join("claude.md");
        if workspace_claude.exists() {
            let skills = load_skills_from_claude_md(&workspace_claude);
            add_skills(&mut all_skills, &mut seen_names, skills);
        }

        // 4. 当前目录 claude.md
        let project_claude = cwd.join("claude.md");
        if project_claude.exists() {
            let skills = load_skills_from_claude_md(&project_claude);
            add_skills(&mut all_skills, &mut seen_names, skills);
        }

        Ok(SkillManager::new(all_skills))
    }

    /// 从指定 claude.md 文件加载（向后兼容）
    pub fn from_claude_md(path: &Path) -> Result<Self> {
        let skills = load_skills_from_claude_md(path);
        Ok(SkillManager::new(skills))
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

// ─────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────

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

    const SAMPLE_SKILL_MD: &str = r#"---
description: 对代码进行全面的安全审查
when_to_use: 当需要检查代码安全漏洞时
argument-hint: <file_or_dir>
---

请对以下代码进行安全审查：

$ARGUMENT

重点检查：
1. SQL 注入
2. XSS 漏洞
3. 认证/授权问题
"#;

    #[test]
    fn test_parse_claude_md_skill_count() {
        let skills = parse_claude_md(SAMPLE_CLAUDE_MD);
        assert_eq!(skills.len(), 3);
    }

    #[test]
    fn test_parse_claude_md_skill_names() {
        let skills = parse_claude_md(SAMPLE_CLAUDE_MD);
        assert_eq!(skills[0].name, "code-review");
        assert_eq!(skills[1].name, "refactor");
        assert_eq!(skills[2].name, "write-tests");
    }

    #[test]
    fn test_parse_claude_md_description() {
        let skills = parse_claude_md(SAMPLE_CLAUDE_MD);
        assert!(skills[0].description.contains("逻辑正确性"));
    }

    #[test]
    fn test_parse_claude_md_examples() {
        let skills = parse_claude_md(SAMPLE_CLAUDE_MD);
        assert_eq!(skills[0].examples.len(), 2);
        assert!(skills[0].examples[0].contains("SQL 注入"));
    }

    #[test]
    fn test_parse_skill_md_frontmatter() {
        let (fm, content) = parse_skill_md(SAMPLE_SKILL_MD);
        assert_eq!(fm.description.as_deref(), Some("对代码进行全面的安全审查"));
        assert_eq!(fm.when_to_use.as_deref(), Some("当需要检查代码安全漏洞时"));
        assert_eq!(fm.argument_hint.as_deref(), Some("<file_or_dir>"));
        assert!(content.contains("$ARGUMENT"));
    }

    #[test]
    fn test_skill_expand_prompt_argument() {
        let skill = Skill {
            name: "security-review".to_string(),
            description: "安全审查".to_string(),
            when_to_use: None,
            argument_hint: Some("<file>".to_string()),
            content: "请审查：$ARGUMENT".to_string(),
            base_dir: None,
            loaded_from: SkillSource::Global,
            examples: vec![],
        };
        let expanded = skill.expand_prompt("src/main.rs");
        assert_eq!(expanded, "请审查：src/main.rs");
    }

    #[test]
    fn test_skill_expand_prompt_positional() {
        let skill = Skill {
            name: "test".to_string(),
            description: "test".to_string(),
            when_to_use: None,
            argument_hint: None,
            content: "file=$1 lang=$2".to_string(),
            base_dir: None,
            loaded_from: SkillSource::Global,
            examples: vec![],
        };
        let expanded = skill.expand_prompt("main.rs rust");
        assert_eq!(expanded, "file=main.rs lang=rust");
    }

    #[test]
    fn test_skill_manager_match_slash_command() {
        let skills = parse_claude_md(SAMPLE_CLAUDE_MD);
        let mgr = SkillManager::new(skills);

        let result = mgr.match_slash_command("/code-review src/main.rs");
        assert!(result.is_some());
        let (skill, args) = result.unwrap();
        assert_eq!(skill.name, "code-review");
        assert_eq!(args, "src/main.rs");
    }

    #[test]
    fn test_skill_manager_no_match() {
        let mgr = SkillManager::empty();
        assert!(mgr.match_slash_command("/unknown").is_none());
        assert!(mgr.match_slash_command("not a command").is_none());
    }

    #[test]
    fn test_system_prompt_block() {
        let skills = parse_claude_md(SAMPLE_CLAUDE_MD);
        let mgr = SkillManager::new(skills);
        let block = mgr.system_prompt_block();
        assert!(block.contains("## Available Skills"));
        assert!(block.contains("/code-review"));
        assert!(block.contains("/refactor"));
    }

    #[test]
    fn test_empty_manager() {
        let mgr = SkillManager::empty();
        assert!(mgr.skills().is_empty());
        assert_eq!(mgr.system_prompt_block(), "");
    }

    // ── match_intent 测试 ──

    fn make_alert_skill() -> Skill {
        Skill {
            name: "alert-search".to_string(),
            description: "搜索控制器告警数据".to_string(),
            when_to_use: Some("当用户提到控制器告警、告警搜索、告警数据、告警分析、告警看板、SearchBI、告警报告、查告警、找告警、alert search、alert data 时自动触发".to_string()),
            argument_hint: Some("<告警查询描述>".to_string()),
            content: "## 告警搜索指令\n调用 search_alert 工具".to_string(),
            base_dir: None,
            loaded_from: SkillSource::Global,
            examples: vec![],
        }
    }

    #[test]
    fn test_match_intent_direct_keyword() {
        // 直接包含关键词"告警数据"
        let mgr = SkillManager::new(vec![make_alert_skill()]);
        let matches = mgr.match_intent("帮我查下 XCU-A 的告警数据");
        assert!(!matches.is_empty(), "应该命中 alert-search");
        assert_eq!(matches[0].0.name, "alert-search");
    }

    #[test]
    fn test_match_intent_suffix_keyword() {
        // "温度告警" 不在关键词列表，但"告警"是"控制器告警"的后缀子词，应该命中
        let mgr = SkillManager::new(vec![make_alert_skill()]);
        let matches = mgr.match_intent("给我找下最近一周 ThorU 的温度告警");
        assert!(!matches.is_empty(), "应该通过后缀子词'告警'命中 alert-search");
        assert_eq!(matches[0].0.name, "alert-search");
    }

    #[test]
    fn test_match_intent_english_keyword() {
        // 英文关键词 "alert search"
        let mgr = SkillManager::new(vec![make_alert_skill()]);
        let matches = mgr.match_intent("alert search for kernel panic");
        assert!(!matches.is_empty(), "应该命中 alert-search（英文关键词）");
    }

    #[test]
    fn test_match_intent_searchbi() {
        // SearchBI 关键词（大小写不敏感）
        let mgr = SkillManager::new(vec![make_alert_skill()]);
        let matches = mgr.match_intent("searchbi 告警看板");
        assert!(!matches.is_empty(), "应该命中 alert-search（SearchBI）");
    }

    #[test]
    fn test_match_intent_no_match() {
        // 无关输入不应命中
        let mgr = SkillManager::new(vec![make_alert_skill()]);
        let matches = mgr.match_intent("帮我 review 这段代码");
        assert!(matches.is_empty(), "代码 review 不应命中 alert-search");

        let matches2 = mgr.match_intent("ls -la");
        assert!(matches2.is_empty(), "ls 命令不应命中任何 skill");
    }

    #[test]
    fn test_match_intent_multi_skill_ranking() {
        // 多个 skill 时，命中关键词多的排在前面
        let alert_skill = make_alert_skill();
        let review_skill = Skill {
            name: "code-review".to_string(),
            description: "代码审查".to_string(),
            when_to_use: Some("当用户说 review 代码、审查代码、代码质量时".to_string()),
            argument_hint: None,
            content: "审查代码".to_string(),
            base_dir: None,
            loaded_from: SkillSource::Global,
            examples: vec![],
        };
        let mgr = SkillManager::new(vec![alert_skill, review_skill]);

        // 包含多个告警关键词，alert-search 应排第一
        let matches = mgr.match_intent("SearchBI 告警看板 告警数据分析");
        assert!(!matches.is_empty());
        assert_eq!(matches[0].0.name, "alert-search");
    }

    #[test]
    fn test_summary_prompt_block_contains_trigger() {
        let mgr = SkillManager::new(vec![make_alert_skill()]);
        let block = mgr.summary_prompt_block();
        assert!(block.contains("alert-search"));
        assert!(block.contains("Trigger:"));
        assert!(block.contains("Intent-Triggered"));
        // 不应包含完整内容
        assert!(!block.contains("search_alert"));
    }

    #[test]
    fn test_expand_matched_skills_contains_full_content() {
        let mgr = SkillManager::new(vec![make_alert_skill()]);
        let expanded = mgr.expand_matched_skills("查告警数据");
        assert!(!expanded.is_empty());
        assert!(expanded.contains("Activated Skills"));
        assert!(expanded.contains("search_alert")); // 完整内容
        assert!(expanded.contains("alert-search"));
    }
}
