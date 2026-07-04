//! 用户画像/语义记忆提取器 / user profile and semantic memory extractor.
//!
//! 在任务成功后调用 LLM,从用户目标、任务描述与执行结果中总结
//! 可沉淀为长期记忆的偏好与项目约定。

use async_trait::async_trait;
use serde::Deserialize;

use axon_core::Result;
use axon_llm::{CompletionRequest, LlmProvider, Message, Role};
use axon_memory::{Memory, MemoryKind, DEFAULT_WEIGHT};
use axon_proto::Task;

use crate::{AgentOutput, Goal};

/// 画像提取结果 / a raw extracted profile item before converting to [`Memory`].
#[derive(Debug, Deserialize)]
struct ExtractedItem {
    /// `user_profile` 或 `semantic`。
    kind: String,
    /// 记忆文本内容。
    content: String,
    /// 可选权重,默认 `DEFAULT_WEIGHT`。
    #[serde(default = "default_extracted_weight")]
    weight: f32,
}

fn default_extracted_weight() -> f32 {
    DEFAULT_WEIGHT
}

/// 用户画像提取器 trait。
///
/// 实现者从一次任务执行中抽取出应沉淀的长期记忆。
#[async_trait]
pub trait ProfileExtractor: Send + Sync {
    /// 提取记忆 / extract memories from a completed task.
    ///
    /// 返回的记忆 `id` 为空,由下游 `MemoryStore::store` 自动分配。
    async fn extract(
        &self,
        goal: &Goal,
        task: &Task,
        output: &AgentOutput,
        llm: &dyn LlmProvider,
    ) -> Result<Vec<Memory>>;
}

/// 基于 LLM 的画像提取器 / LLM-based profile extractor.
#[derive(Debug, Default)]
pub struct LlmProfileExtractor;

impl LlmProfileExtractor {
    /// 创建新的 LLM 画像提取器 / create a new LLM profile extractor.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ProfileExtractor for LlmProfileExtractor {
    async fn extract(
        &self,
        goal: &Goal,
        task: &Task,
        output: &AgentOutput,
        llm: &dyn LlmProvider,
    ) -> Result<Vec<Memory>> {
        let system = "You are a memory extraction assistant. \
Given a user goal, a planned task, and the execution result, extract 0 to 3 concise long-term memories about the user's preferences or project conventions. \
Output ONLY a JSON array of objects with fields: \
kind ('user_profile' for personal habits, 'semantic' for project conventions), \
content (a short sentence), \
weight (optional float between 1.0 and 1.5). \
If there is nothing worth remembering, output an empty array []. Do not include markdown or explanations.";

        let user = format!(
            "User goal: {}\n\nTask: {}\n\nExecution result:\n{}\n\nJSON:",
            goal.description, task.description, output.summary
        );

        let req = CompletionRequest {
            model: String::new(),
            messages: vec![
                Message {
                    role: Role::System,
                    content: system.into(),
                    tool_calls: None,
                },
                Message {
                    role: Role::User,
                    content: user,
                    tool_calls: None,
                },
            ],
            tools: vec![],
            temperature: 0.1,
            max_tokens: Some(512),
        };

        let resp = llm.complete(req).await?;
        let raw = resp.message.content.trim();

        let items = match parse_extracted_items(raw) {
            Ok(items) => items,
            Err(_) => {
                // 解析失败时不阻塞任务,直接放弃本次提取。
                return Ok(vec![]);
            }
        };

        Ok(items
            .into_iter()
            .map(|item| Memory {
                id: String::new(),
                kind: parse_memory_kind(&item.kind).unwrap_or(MemoryKind::Semantic),
                content: item.content,
                embedding: None,
                weight: item.weight.clamp(0.0, 2.0),
                created_at: 0,
                updated_at: 0,
                source: Some("profile-extractor".into()),
            })
            .collect())
    }
}

fn parse_extracted_items(raw: &str) -> Result<Vec<ExtractedItem>> {
    // 尝试提取 ```json ... ``` 代码块。
    let json_str = if let Some(start) = raw.find("```") {
        let after_start = &raw[start + 3..];
        let content_start = after_start.find('\n').map(|n| n + 1).unwrap_or(0);
        let after_lang = &after_start[content_start..];
        if let Some(end) = after_lang.find("```") {
            after_lang[..end].trim()
        } else {
            raw.trim()
        }
    } else {
        raw.trim()
    };

    serde_json::from_str::<Vec<ExtractedItem>>(json_str).map_err(axon_core::Error::Json)
}

fn parse_memory_kind(s: &str) -> Option<MemoryKind> {
    match s {
        "user_profile" => Some(MemoryKind::UserProfile),
        "semantic" => Some(MemoryKind::Semantic),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use axon_core::Result;
    use axon_llm::{
        Capabilities, CompletionRequest, CompletionResponse, Delta, FinishReason, LlmProvider,
        Message, Role, Usage,
    };
    use axon_proto::{Priority, TaskState};

    use super::*;

    struct MockLlm {
        response: String,
    }

    #[async_trait]
    impl LlmProvider for MockLlm {
        fn id(&self) -> &str {
            "mock"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse> {
            Ok(CompletionResponse {
                message: Message {
                    role: Role::Assistant,
                    content: self.response.clone(),
                    tool_calls: None,
                },
                usage: Usage::default(),
                finish_reason: FinishReason::Stop,
            })
        }
        async fn stream(&self, _req: CompletionRequest) -> Result<Vec<Delta>> {
            unreachable!()
        }
    }

    fn sample_goal() -> Goal {
        Goal {
            description: "write a Rust library using anyhow".into(),
            context: vec![],
        }
    }

    fn sample_task() -> Task {
        Task {
            id: "t1".into(),
            parent: None,
            title: "write lib".into(),
            description: "write a Rust library".into(),
            priority: Priority::Normal,
            state: TaskState::Running,
            dependencies: vec![],
            created_at: 0,
            updated_at: 0,
            acceptance: vec![],
        }
    }

    fn sample_output() -> AgentOutput {
        AgentOutput {
            summary: "cd /workspace && cargo test".into(),
            changed_files: vec![],
            self_check_passed: true,
            log: String::new(),
        }
    }

    /// 验证 LLM 返回的 JSON 数组被正确解析为 Memory。
    #[tokio::test]
    async fn extracts_memories_from_json_array() {
        let extractor = LlmProfileExtractor::new();
        let response = r#"[
            {"kind": "user_profile", "content": "prefers anyhow for binaries", "weight": 1.2},
            {"kind": "semantic", "content": "writes tests for every public function"}
        ]"#
        .into();
        let llm = MockLlm { response };

        let memories = extractor
            .extract(&sample_goal(), &sample_task(), &sample_output(), &llm)
            .await
            .unwrap();

        assert_eq!(memories.len(), 2);
        assert_eq!(memories[0].kind, MemoryKind::UserProfile);
        assert_eq!(memories[0].content, "prefers anyhow for binaries");
        assert!((memories[0].weight - 1.2).abs() < f32::EPSILON);
        assert_eq!(memories[1].kind, MemoryKind::Semantic);
    }

    /// 验证非法 JSON 不阻塞流程,返回空列表。
    #[tokio::test]
    async fn invalid_json_returns_empty() {
        let extractor = LlmProfileExtractor::new();
        let llm = MockLlm {
            response: "not valid json".into(),
        };

        let memories = extractor
            .extract(&sample_goal(), &sample_task(), &sample_output(), &llm)
            .await
            .unwrap();

        assert!(memories.is_empty());
    }

    /// 验证能处理被 markdown 代码块包裹的 JSON。
    #[tokio::test]
    async fn extracts_from_markdown_fence() {
        let extractor = LlmProfileExtractor::new();
        let response =
            "```json\n[{\"kind\": \"semantic\", \"content\": \"use cargo workspace\"}]\n```".into();
        let llm = MockLlm { response };

        let memories = extractor
            .extract(&sample_goal(), &sample_task(), &sample_output(), &llm)
            .await
            .unwrap();

        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].content, "use cargo workspace");
    }
}
