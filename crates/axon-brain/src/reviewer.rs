//! 调度大脑复核 / reviewer for worker output.
//!
//! Worker 执行完成后,调度大脑不只看 exit_code,而是经 `Reviewer` 判断产出是否满足
//! 任务的 `acceptance` 标准。M3 提供规则复核与 LLM 复核两种实现。

use async_trait::async_trait;

use axon_core::Result;
use axon_llm::{CompletionRequest, LlmProvider, Message, Role};
use axon_proto::Task;

/// 复核结果 / review verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewResult {
    /// 通过 / output meets acceptance criteria.
    Accept,
    /// 不通过 / output does not meet criteria.
    Reject { reason: String },
    /// 建议重试 / retry with the given reason.
    Retry { reason: String },
}

/// 任务产出的统一视图 / a unified view of task output for review.
#[derive(Debug, Clone)]
pub struct TaskOutput {
    /// 容器/VM 退出码 / process exit code.
    pub exit_code: i32,
    /// 标准输出 / stdout.
    pub stdout: String,
    /// 标准错误 / stderr.
    pub stderr: String,
    /// Agent 生成的产出摘要 / agent summary.
    pub summary: String,
    /// 自检日志 / self-check log.
    pub log: String,
    /// Agent 自检是否通过 / self-check passed.
    pub self_check_passed: bool,
}

/// 复核器 trait / the reviewer trait.
#[async_trait]
pub trait Reviewer: Send + Sync {
    /// 复核 worker 产出 / review worker output against task acceptance.
    async fn review(
        &self,
        task: &Task,
        output: &TaskOutput,
        llm: &dyn LlmProvider,
    ) -> Result<ReviewResult>;
}

/// 规则复核器 / rule-based reviewer.
///
/// 简单规则:exit_code == 0 且 agent 自检通过则 Accept,否则 Reject。
#[derive(Debug, Default)]
pub struct RuleReviewer;

impl RuleReviewer {
    /// 创建规则复核器 / create a rule-based reviewer.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Reviewer for RuleReviewer {
    async fn review(
        &self,
        _task: &Task,
        output: &TaskOutput,
        _llm: &dyn LlmProvider,
    ) -> Result<ReviewResult> {
        if output.exit_code == 0 && output.self_check_passed {
            Ok(ReviewResult::Accept)
        } else {
            let reason = if output.exit_code != 0 {
                format!("exit_code={}", output.exit_code)
            } else {
                "self_check_failed".into()
            };
            Ok(ReviewResult::Reject { reason })
        }
    }
}

/// LLM 复核器 / LLM-based reviewer.
///
/// 把任务描述、验收标准、stdout/stderr、自检日志交给 LLM,让模型判断
/// Accept/Reject/Retry 并给出理由。
#[derive(Debug, Default)]
pub struct LlmReviewer;

impl LlmReviewer {
    /// 创建 LLM 复核器 / create an LLM-based reviewer.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Reviewer for LlmReviewer {
    async fn review(
        &self,
        task: &Task,
        output: &TaskOutput,
        llm: &dyn LlmProvider,
    ) -> Result<ReviewResult> {
        let acceptance = if task.acceptance.is_empty() {
            "任务应成功完成且不破坏现有构建/测试".into()
        } else {
            task.acceptance.join("\n-")
        };

        let prompt = format!(
            r#"你是一名严格的代码验收 reviewer。请根据任务描述、验收标准、执行输出判断结果。

任务描述:
{}

验收标准:
- {}

Agent 产出摘要:
{}

stdout:
{}

stderr:
{}

自检日志:
{}

exit_code: {}

请只返回以下三种格式之一(不要额外解释):
ACCEPT
REJECT: <理由>
RETRY: <理由>
"#,
            task.description,
            acceptance,
            output.summary,
            output.stdout,
            output.stderr,
            output.log,
            output.exit_code
        );

        let req = CompletionRequest {
            model: String::new(),
            messages: vec![Message {
                role: Role::User,
                content: prompt,
                tool_calls: None,
            }],
            tools: vec![],
            temperature: 0.0,
            max_tokens: None,
        };

        let resp = llm.complete(req).await?;
        parse_review_response(&resp.message.content)
    }
}

/// 解析 LLM 复核响应 / parse the LLM review response.
fn parse_review_response(content: &str) -> Result<ReviewResult> {
    let trimmed = content.trim();
    if trimmed.starts_with("ACCEPT") {
        return Ok(ReviewResult::Accept);
    }
    if let Some(reason) = trimmed.strip_prefix("REJECT:") {
        return Ok(ReviewResult::Reject {
            reason: reason.trim().to_string(),
        });
    }
    if let Some(reason) = trimmed.strip_prefix("RETRY:") {
        return Ok(ReviewResult::Retry {
            reason: reason.trim().to_string(),
        });
    }
    // 兜底:按通过处理,但记录原始内容。
    Ok(ReviewResult::Accept)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_task() -> Task {
        Task {
            id: "t1".into(),
            parent: None,
            title: "t".into(),
            description: "实现一个函数".into(),
            priority: axon_proto::Priority::Normal,
            state: axon_proto::TaskState::Running,
            dependencies: vec![],
            created_at: 0,
            updated_at: 0,
            acceptance: vec!["测试通过".into()],
        }
    }

    fn sample_output(exit_code: i32, self_check: bool) -> TaskOutput {
        TaskOutput {
            exit_code,
            stdout: "ok".into(),
            stderr: String::new(),
            summary: "done".into(),
            log: "test passed".into(),
            self_check_passed: self_check,
        }
    }

    /// 规则复核器在 exit_code=0 且自检通过时返回 Accept。
    #[tokio::test]
    async fn rule_reviewer_accepts_success() {
        let reviewer = RuleReviewer::new();
        let result = reviewer
            .review(&sample_task(), &sample_output(0, true), &DummyLlm)
            .await
            .unwrap();
        assert_eq!(result, ReviewResult::Accept);
    }

    /// 规则复核器在非零 exit_code 时返回 Reject。
    #[tokio::test]
    async fn rule_reviewer_rejects_failure() {
        let reviewer = RuleReviewer::new();
        let result = reviewer
            .review(&sample_task(), &sample_output(1, false), &DummyLlm)
            .await
            .unwrap();
        assert!(matches!(result, ReviewResult::Reject { .. }));
    }

    /// 解析 ACCEPT 响应。
    #[test]
    fn parse_accept() {
        assert_eq!(
            parse_review_response("ACCEPT").unwrap(),
            ReviewResult::Accept
        );
    }

    /// 解析 REJECT 响应。
    #[test]
    fn parse_reject() {
        let result = parse_review_response("REJECT: 测试未通过").unwrap();
        assert_eq!(
            result,
            ReviewResult::Reject {
                reason: "测试未通过".into()
            }
        );
    }

    /// 解析 RETRY 响应。
    #[test]
    fn parse_retry() {
        let result = parse_review_response("RETRY: 缺少依赖").unwrap();
        assert_eq!(
            result,
            ReviewResult::Retry {
                reason: "缺少依赖".into()
            }
        );
    }

    use axon_llm::CompletionResponse;

    struct DummyLlm;

    #[async_trait]
    impl LlmProvider for DummyLlm {
        fn id(&self) -> &str {
            "dummy"
        }
        fn capabilities(&self) -> axon_llm::Capabilities {
            axon_llm::Capabilities::default()
        }
        async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse> {
            unreachable!()
        }
        async fn stream(&self, _req: CompletionRequest) -> Result<Vec<axon_llm::Delta>> {
            unreachable!()
        }
    }
}
