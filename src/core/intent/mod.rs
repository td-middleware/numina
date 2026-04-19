/// 意图澄清核心模块
///
/// 提供全局可复用的意图分析和澄清能力，可在 TUI、Channel 等不同上下文中使用。
///
/// # 架构
///
/// ```text
/// IntentAnalyzer          — 调用模型分析意图（纯逻辑，无渠道依赖）
/// IntentClarifier (trait) — 向用户展示选项并等待选择
///   ├── TuiIntentClarifier   — crossterm 交互式选择（TUI/CLI 用）
///   └── LarkIntentClarifier  — 飞书消息选择（channel 用）
/// ```
///
/// # 使用示例
///
/// ```rust
/// // TUI 场景
/// let analyzer = IntentAnalyzer::new(engine.clone());
/// let clarifier = TuiIntentClarifier::new();
/// let prompt = clarify_intent(&analyzer, &clarifier, user_input).await?;
/// engine.chat_react(&prompt, ...).await?;
///
/// // Feishu channel 场景
/// let clarifier = LarkIntentClarifier::new(message_id.clone());
/// let prompt = clarify_intent(&analyzer, &clarifier, user_input).await?;
/// ```

pub mod analyzer;
pub mod clarifier;

pub use analyzer::{IntentAnalysis, IntentAnalyzer, IntentOption};
pub use clarifier::{IntentClarifier, TuiIntentClarifier};

use std::sync::Arc;

/// 便捷函数：分析意图，如果不明确则通过 clarifier 让用户选择
///
/// 返回最终要执行的 prompt：
/// - 意图明确 → 返回精准化后的 prompt
/// - 意图不明确 → 展示选项，用户选择后返回对应的 refined_prompt
/// - 用户取消 → 返回 None
pub async fn clarify_intent<C: IntentClarifier>(
    analyzer: &IntentAnalyzer,
    clarifier: &C,
    user_message: &str,
) -> anyhow::Result<Option<String>> {
    match analyzer.analyze(user_message).await? {
        IntentAnalysis::Clear { refined_prompt } => Ok(Some(refined_prompt)),
        IntentAnalysis::Ambiguous { options } => {
            match clarifier.clarify(user_message, options).await {
                Some(chosen) => Ok(Some(chosen.refined_prompt)),
                None => Ok(None), // 用户取消
            }
        }
    }
}

/// 便捷函数：分析意图，如果不明确则通过 clarifier 让用户选择（Arc 版本）
pub async fn clarify_intent_arc<C: IntentClarifier>(
    analyzer: &IntentAnalyzer,
    clarifier: Arc<C>,
    user_message: &str,
) -> anyhow::Result<Option<String>> {
    clarify_intent(analyzer, clarifier.as_ref(), user_message).await
}

// ─────────────────────────────────────────────
// 集成测试（使用 PassthroughClarifier 模拟用户选择）
// ─────────────────────────────────────────────

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::core::intent::clarifier::PassthroughClarifier;
    use crate::core::intent::analyzer::{IntentAnalysis, parse_intent_response};

    /// 测试案例 1：TUI 场景 — 意图明确，直接执行
    ///
    /// 模拟用户在 TUI 中输入「查看当前目录」，
    /// 模型判断意图明确，clarify_intent 直接返回精准化 prompt，
    /// 不需要弹出选择框。
    ///
    /// 对应真实场景：
    ///   用户输入 → IntentAnalyzer::analyze() → Clear
    ///   → clarify_intent 返回 Some("列出当前目录下的所有文件和子目录")
    ///   → 直接调用 engine.chat_react(refined_prompt, ...)
    #[test]
    fn test_case_1_tui_clear_intent() {
        // 模拟模型返回「意图明确」的 JSON
        let model_response = r#"{"clear": true, "refined_prompt": "列出当前目录下的所有文件和子目录"}"#;
        let result = parse_intent_response(model_response, "查看当前目录").unwrap();

        match result {
            IntentAnalysis::Clear { refined_prompt } => {
                // 意图明确：直接用精准化 prompt 执行
                assert_eq!(refined_prompt, "列出当前目录下的所有文件和子目录");
                println!("✅ 案例1 TUI 明确意图：直接执行 → \"{}\"", refined_prompt);
            }
            IntentAnalysis::Ambiguous { .. } => panic!("应该是明确意图"),
        }
    }

    /// 测试案例 2：飞书 Channel 场景 — 意图不明确，用户选择后执行
    ///
    /// 模拟用户在飞书发送「帮我处理一下代码」，
    /// 模型判断意图不明确，生成 3 个候选选项，
    /// PassthroughClarifier 自动选择第 1 个（模拟用户回复「1」），
    /// clarify_intent 返回第 1 个选项的 refined_prompt。
    ///
    /// 对应真实场景（飞书）：
    ///   用户发消息 → IntentAnalyzer::analyze() → Ambiguous { options: [审查, 格式化, 注释] }
    ///   → LarkIntentClarifier::clarify() 发送飞书消息展示选项
    ///   → 用户回复「2」→ 返回 Some("请格式化当前项目的代码")
    ///   → 调用 engine.chat_react("请格式化当前项目的代码", ...)
    ///   → 飞书回复执行结果
    #[tokio::test]
    async fn test_case_2_lark_ambiguous_intent_user_selects() {
        // 模拟模型返回「意图不明确」的 JSON，包含 3 个候选选项
        let model_response = r#"{
            "clear": false,
            "options": [
                {
                    "title": "代码审查",
                    "description": "检查代码中的潜在问题和安全漏洞",
                    "refined_prompt": "请审查当前项目的代码，找出潜在问题和安全漏洞"
                },
                {
                    "title": "代码格式化",
                    "description": "统一代码风格，运行 cargo fmt",
                    "refined_prompt": "请格式化当前项目的代码，运行 cargo fmt 并报告变更"
                },
                {
                    "title": "添加注释",
                    "description": "为关键函数和模块添加文档注释",
                    "refined_prompt": "请为当前项目的关键函数和模块添加详细的文档注释"
                }
            ]
        }"#;

        let analysis = parse_intent_response(model_response, "帮我处理一下代码").unwrap();

        match analysis {
            IntentAnalysis::Ambiguous { options } => {
                assert_eq!(options.len(), 3);
                println!("🤔 意图不明确，生成 {} 个候选选项：", options.len());
                for opt in &options {
                    println!("   {}. {} — {}", opt.index, opt.title, opt.description);
                }

                // 使用 PassthroughClarifier 模拟用户选择第 1 个（飞书场景下用户回复「1」）
                let clarifier = PassthroughClarifier;
                let chosen = clarifier.clarify("帮我处理一下代码", options).await;

                assert!(chosen.is_some());
                let chosen = chosen.unwrap();
                assert_eq!(chosen.title, "代码审查");
                assert_eq!(chosen.refined_prompt, "请审查当前项目的代码，找出潜在问题和安全漏洞");

                println!("✅ 案例2 飞书用户选择「{}」→ 执行：\"{}\"",
                    chosen.title, chosen.refined_prompt);
            }
            IntentAnalysis::Clear { .. } => panic!("应该是不明确意图"),
        }
    }
}
