/// 飞书意图澄清器
///
/// 基于 core::intent 的通用框架，实现飞书渠道特有的意图选择交互：
/// 通过飞书消息发送选项，等待用户回复数字选择。
///
/// 通用的 IntentAnalyzer / IntentOption / IntentAnalysis 已移至 src/core/intent/，
/// 此文件只保留飞书特有的 LarkIntentClarifier。

use std::sync::Arc;
use tokio::sync::{oneshot, Mutex};
use tracing::{info, warn};
use async_trait::async_trait;

// 复用核心意图模块
pub use crate::core::intent::{
    IntentAnalysis, IntentAnalyzer, IntentClarifier, IntentOption, TuiIntentClarifier,
};

// ─────────────────────────────────────────────
// 飞书意图选择器
// ─────────────────────────────────────────────

/// 待确认的意图选择请求
struct PendingIntentChoice {
    options: Vec<IntentOption>,
    reply_tx: oneshot::Sender<Option<IntentOption>>,
}

/// 飞书意图澄清器
///
/// 实现 IntentClarifier trait，通过飞书消息向用户发送选项，
/// 等待用户回复数字选择。
///
/// 与 TuiIntentClarifier 的区别：
/// - TUI：同步阻塞终端，等待键盘输入
/// - Lark：异步发送飞书消息，等待用户在飞书中回复数字
pub struct LarkIntentClarifier {
    /// 关联的飞书消息 ID（用于回复）
    message_id: String,
    /// 待确认的意图选择（同一时间只有一个）
    pending: Arc<Mutex<Option<PendingIntentChoice>>>,
}

impl LarkIntentClarifier {
    pub fn new(message_id: String) -> Self {
        Self {
            message_id,
            pending: Arc::new(Mutex::new(None)),
        }
    }

    /// 处理用户的回复消息
    ///
    /// 当用户回复数字时，匹配对应的意图选项。
    /// 返回 true 表示消息被消费（是意图选择回复）
    pub async fn handle_reply(&self, content: &str, reply_message_id: &str) -> bool {
        let trimmed = content.trim();

        let choice_num: Option<usize> = trimmed.parse().ok();
        if choice_num.is_none() {
            return false;
        }
        let choice_num = choice_num.unwrap();

        let mut pending = self.pending.lock().await;
        if let Some(req) = pending.take() {
            let max_idx = req.options.len();
            if choice_num >= 1 && choice_num <= max_idx {
                let chosen = req.options[choice_num - 1].clone();
                info!(
                    "LarkIntentClarifier: user chose option {}: {}",
                    choice_num, chosen.title
                );
                send_lark_reply(
                    reply_message_id,
                    &format!("✅ 已选择：**{}**\n{}", chosen.title, chosen.description),
                )
                .await;
                let _ = req.reply_tx.send(Some(chosen));
                return true;
            } else if choice_num == 0 {
                send_lark_reply(reply_message_id, "❌ 已取消").await;
                let _ = req.reply_tx.send(None);
                return true;
            } else {
                // 超出范围，重新放回
                *pending = Some(req);
                send_lark_reply(
                    reply_message_id,
                    &format!("⚠️ 请输入 1-{} 之间的数字，或输入 0 取消", max_idx),
                )
                .await;
                return true;
            }
        }

        false
    }

    /// 是否有待确认的意图选择
    pub async fn has_pending(&self) -> bool {
        self.pending.lock().await.is_some()
    }
}

#[async_trait]
impl IntentClarifier for LarkIntentClarifier {
    async fn clarify(
        &self,
        original_message: &str,
        options: Vec<IntentOption>,
    ) -> Option<IntentOption> {
        // 构建选项消息
        let msg_text = build_intent_message(original_message, &options);

        // 发送飞书消息
        send_lark_reply(&self.message_id, &msg_text).await;

        // 注册待确认请求
        let (reply_tx, reply_rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            *pending = Some(PendingIntentChoice {
                options: options.clone(),
                reply_tx,
            });
        }

        info!(
            "LarkIntentClarifier: waiting for user to choose from {} options",
            options.len()
        );

        // 等待用户回复（最多 3 分钟）
        match tokio::time::timeout(tokio::time::Duration::from_secs(180), reply_rx).await {
            Ok(Ok(choice)) => choice,
            Ok(Err(_)) => None,
            Err(_) => {
                warn!("LarkIntentClarifier: user choice timed out");
                let mut pending = self.pending.lock().await;
                *pending = None;
                send_lark_reply(&self.message_id, "⏰ 选择超时（3分钟），已取消本次请求").await;
                None
            }
        }
    }
}

// ─────────────────────────────────────────────
// 辅助函数
// ─────────────────────────────────────────────

/// 构建意图选项消息文本
fn build_intent_message(original_msg: &str, options: &[IntentOption]) -> String {
    let mut lines = vec![
        "🤔 **我理解你的意思可能是以下之一，请选择：**".to_string(),
        String::new(),
        format!("原始消息：「{}」", original_msg),
        String::new(),
        "─────────────────".to_string(),
    ];

    for opt in options {
        lines.push(format!("{}️⃣  **{}**", opt.index, opt.title));
        if !opt.description.is_empty() {
            lines.push(format!("   _{}_", opt.description));
        }
    }

    lines.push("─────────────────".to_string());
    lines.push(format!(
        "请回复数字 1-{}，或回复 0 取消",
        options.len()
    ));

    lines.join("\n")
}

/// 通过 lark-cli 发送飞书消息回复
async fn send_lark_reply(message_id: &str, content: &str) {
    if message_id.is_empty() {
        return;
    }
    let reply_json = serde_json::json!({
        "msg_type": "text",
        "content": serde_json::json!({"text": content}).to_string()
    });
    let reply_str = reply_json.to_string();

    let output = tokio::process::Command::new("lark-cli")
        .args([
            "api",
            "POST",
            &format!("/open-apis/im/v1/messages/{}/reply", message_id),
            "--data",
            &reply_str,
            "--as",
            "bot",
            "--format",
            "data",
        ])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            warn!(
                "LarkIntentClarifier: send failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Err(e) => {
            warn!("LarkIntentClarifier: lark-cli not available: {}", e);
        }
    }
}

// ─────────────────────────────────────────────
// 单元测试
// ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::intent::analyzer::{extract_json, parse_intent_response};

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

    #[test]
    fn test_build_intent_message() {
        let options = vec![
            IntentOption {
                index: 1,
                title: "代码审查".to_string(),
                description: "检查潜在问题".to_string(),
                refined_prompt: "审查代码".to_string(),
            },
            IntentOption {
                index: 2,
                title: "代码格式化".to_string(),
                description: "统一风格".to_string(),
                refined_prompt: "格式化代码".to_string(),
            },
        ];
        let msg = build_intent_message("处理代码", &options);
        assert!(msg.contains("代码审查"));
        assert!(msg.contains("代码格式化"));
        assert!(msg.contains("1-2"));
    }
}
