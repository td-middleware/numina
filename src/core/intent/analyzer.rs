/// 意图分析器
///
/// 调用模型分析用户消息，判断意图是否明确，
/// 如果不明确则生成候选意图列表。
///
/// 此模块不依赖任何渠道（Lark/TUI），可在任何上下文中使用。

use std::sync::Arc;
use crate::core::chat::ChatEngine;

// ─────────────────────────────────────────────
// 意图候选项
// ─────────────────────────────────────────────

/// 一个候选意图
#[derive(Debug, Clone)]
pub struct IntentOption {
    /// 序号（1-based，用于用户回复）
    pub index: usize,
    /// 意图标题（简短，如「代码审查」）
    pub title: String,
    /// 意图描述（详细说明会做什么）
    pub description: String,
    /// 精准化后的 prompt（直接发给 agent loop 执行）
    pub refined_prompt: String,
}

/// 意图分析结果
#[derive(Debug)]
pub enum IntentAnalysis {
    /// 意图明确，直接执行（附带精准化的 prompt）
    Clear { refined_prompt: String },
    /// 意图不明确，需要用户选择
    Ambiguous { options: Vec<IntentOption> },
}

// ─────────────────────────────────────────────
// 意图分析器
// ─────────────────────────────────────────────

/// 意图分析器
///
/// 调用模型分析用户消息，判断意图是否明确，
/// 如果不明确则生成候选意图列表。
pub struct IntentAnalyzer {
    engine: Arc<ChatEngine>,
}

impl IntentAnalyzer {
    pub fn new(engine: Arc<ChatEngine>) -> Self {
        Self { engine }
    }

    /// 分析用户消息的意图
    ///
    /// 返回 IntentAnalysis::Clear 或 IntentAnalysis::Ambiguous
    pub async fn analyze(&self, user_message: &str) -> anyhow::Result<IntentAnalysis> {
        let analysis_prompt = format!(
            r#"分析以下用户消息的意图，判断是否明确：

用户消息：「{msg}」

请按以下 JSON 格式回复（不要有其他内容）：

如果意图明确，回复：
{{"clear": true, "refined_prompt": "精准化后的完整指令"}}

如果意图不明确（有多种可能的理解），回复：
{{"clear": false, "options": [
  {{"title": "意图标题1", "description": "会做什么的简短说明", "refined_prompt": "精准化指令1"}},
  {{"title": "意图标题2", "description": "会做什么的简短说明", "refined_prompt": "精准化指令2"}},
  {{"title": "意图标题3", "description": "会做什么的简短说明", "refined_prompt": "精准化指令3"}}
]}}

规则：
- 选项数量 2-4 个，不要太多
- 每个选项的 refined_prompt 要足够具体，可以直接执行
- 如果消息非常明确（如「查看当前目录」「帮我写个 hello world」），直接返回 clear=true
- 只返回 JSON，不要有任何解释"#,
            msg = user_message
        );

        let (response, _sid, _tokens, _ctx) = self
            .engine
            .chat_once(&analysis_prompt, None, None)
            .await?;

        parse_intent_response(&response, user_message)
    }
}

// ─────────────────────────────────────────────
// JSON 解析
// ─────────────────────────────────────────────

/// 解析模型返回的意图分析 JSON
pub(crate) fn parse_intent_response(
    response: &str,
    original_msg: &str,
) -> anyhow::Result<IntentAnalysis> {
    let json_str = extract_json(response).unwrap_or(response);

    let val: serde_json::Value = serde_json::from_str(json_str).unwrap_or_else(|_| {
        serde_json::json!({"clear": true, "refined_prompt": original_msg})
    });

    let is_clear = val.get("clear").and_then(|v| v.as_bool()).unwrap_or(true);

    if is_clear {
        let refined = val
            .get("refined_prompt")
            .and_then(|v| v.as_str())
            .unwrap_or(original_msg)
            .to_string();
        Ok(IntentAnalysis::Clear {
            refined_prompt: refined,
        })
    } else {
        let options_val = val
            .get("options")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        if options_val.is_empty() {
            return Ok(IntentAnalysis::Clear {
                refined_prompt: original_msg.to_string(),
            });
        }

        let options: Vec<IntentOption> = options_val
            .iter()
            .enumerate()
            .map(|(i, opt)| IntentOption {
                index: i + 1,
                title: opt
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("选项")
                    .to_string(),
                description: opt
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                refined_prompt: opt
                    .get("refined_prompt")
                    .and_then(|v| v.as_str())
                    .unwrap_or(original_msg)
                    .to_string(),
            })
            .collect();

        Ok(IntentAnalysis::Ambiguous { options })
    }
}

/// 从文本中提取 JSON 块（处理模型在 JSON 前后加了说明文字的情况）
pub(crate) fn extract_json(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if start <= end {
        Some(&text[start..=end])
    } else {
        None
    }
}

// ─────────────────────────────────────────────
// 单元测试
// ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_clear_intent() {
        let response = r#"{"clear": true, "refined_prompt": "查看当前目录下的所有文件"}"#;
        let result = parse_intent_response(response, "看看文件").unwrap();
        match result {
            IntentAnalysis::Clear { refined_prompt } => {
                assert_eq!(refined_prompt, "查看当前目录下的所有文件");
            }
            _ => panic!("expected Clear"),
        }
    }

    #[test]
    fn test_parse_ambiguous_intent() {
        let response = r#"{
            "clear": false,
            "options": [
                {"title": "代码审查", "description": "检查代码潜在问题", "refined_prompt": "请审查当前项目的代码，找出潜在问题"},
                {"title": "代码格式化", "description": "统一代码风格", "refined_prompt": "请格式化当前项目的代码"},
                {"title": "添加注释", "description": "为代码添加注释", "refined_prompt": "请为当前项目的代码添加详细注释"}
            ]
        }"#;
        let result = parse_intent_response(response, "处理一下代码").unwrap();
        match result {
            IntentAnalysis::Ambiguous { options } => {
                assert_eq!(options.len(), 3);
                assert_eq!(options[0].title, "代码审查");
                assert_eq!(options[0].index, 1);
                assert_eq!(options[1].title, "代码格式化");
                assert_eq!(options[2].title, "添加注释");
            }
            _ => panic!("expected Ambiguous"),
        }
    }

    #[test]
    fn test_parse_invalid_json_fallback() {
        let response = "我无法理解这个请求";
        let result = parse_intent_response(response, "原始消息").unwrap();
        match result {
            IntentAnalysis::Clear { refined_prompt } => {
                assert_eq!(refined_prompt, "原始消息");
            }
            _ => panic!("expected Clear fallback"),
        }
    }

    #[test]
    fn test_extract_json() {
        let text = r#"好的，这是分析结果：{"clear": true} 希望对你有帮助"#;
        assert_eq!(extract_json(text), Some(r#"{"clear": true}"#));
    }
}
