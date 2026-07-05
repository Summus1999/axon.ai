//! 命令生成 Agent / command-generating agent.
//!
//! M1 实现：根据任务描述让 LLM 生成 Rust 代码，
//! 然后自动包装成在 Docker 容器内执行的 shell 命令。
//! 不实现 ReAct 多步循环（M5）。

use async_trait::async_trait;
use base64::Engine;

use axon_core::Result;
use axon_llm::{CompletionRequest, LlmProvider, Message, Role};
use axon_memory::{MemoryStore, RecallQuery};
use axon_proto::Task;

use crate::{Agent, AgentOutput};

/// 命令生成 Agent：把任务描述转换为可执行的 shell 命令。
#[derive(Debug, Default)]
pub struct CommandAgent;

impl CommandAgent {
    /// 创建一个新的命令生成 Agent / create a new command agent.
    pub fn new() -> Self {
        Self
    }

    /// 从 LLM 输出中提取 JSON 对象 / extract the JSON object from LLM output.
    ///
    /// LLM 可能用 markdown 代码块包裹 JSON,本函数先尝试提取 ```json 块,
    /// 再尝试直接解析整个输出。
    fn extract_code_json(raw: &str) -> Option<serde_json::Value> {
        let trimmed = raw.trim();

        // 尝试提取 ```json ... ``` 或 ``` ... ``` 代码块
        if let Some(start) = trimmed.find("```") {
            let after_start = &trimmed[start + 3..];
            let content_start = after_start.find('\n').map(|n| n + 1).unwrap_or(0);
            let after_lang = &after_start[content_start..];
            if let Some(end) = after_lang.find("```") {
                let inner = after_lang[..end].trim();
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(inner) {
                    return Some(value);
                }
            }
        }

        // 直接解析整个输出
        serde_json::from_str(trimmed).ok()
    }

    /// 构造 Rust 项目的执行命令 / build the shell command for a Rust project.
    ///
    /// `code` 为 `src/lib.rs` 的完整内容。命令会:
    /// 1. 在 `/workspace` 下创建 Cargo lib 项目
    /// 2. 用 base64 写入 `src/lib.rs`(避免引号转义问题)
    /// 3. 运行 `cargo test`
    fn build_rust_command(code: &str) -> String {
        let encoded = base64::engine::general_purpose::STANDARD.encode(code);
        // 手动创建独立 workspace,避免 `cargo new` 把项目当作父 workspace 成员
        // 从而触发对 crates.io 的网络访问。
        format!(
            "cd /workspace && mkdir -p axon_task/src && cd axon_task && printf '%s\\n' '[package]' 'name = \"axon_task\"' 'version = \"0.1.0\"' 'edition = \"2021\"' '' '[workspace]' '' > Cargo.toml && echo {encoded} | base64 -d > src/lib.rs && cargo test"
        )
    }
}

#[async_trait]
impl Agent for CommandAgent {
    #[tracing::instrument(skip(self, task, llm, memory), fields(%task.id))]
    async fn execute(
        &self,
        task: &Task,
        llm: &dyn LlmProvider,
        memory: &dyn MemoryStore,
    ) -> Result<AgentOutput> {
        // 召回相关记忆，拼入 prompt 以提供上下文。
        let recalled = memory
            .recall(&RecallQuery {
                query: task.description.clone(),
                kind: None,
                top_k: 5,
            })
            .await?;

        let memory_context = if recalled.is_empty() {
            String::new()
        } else {
            let lines: Vec<_> = recalled
                .iter()
                .map(|m| format!("- {}", m.content))
                .collect();
            format!("Relevant memories:\n{}\n\n", lines.join("\n"))
        };

        let system = "You are a Rust code generator. \
Given a task, output ONLY a JSON object with a single field 'code' containing the full contents of src/lib.rs for a Rust library that satisfies the task and includes tests. \
The code will be placed in /workspace/axon_task/src/lib.rs inside a rust:latest Docker container and `cargo test` will be run automatically. \
No markdown, no explanations, no code fences outside the JSON string. Output only valid JSON.";
        let user = format!("{memory_context}Task: {}\n\nJSON:", task.description);

        let req = CompletionRequest {
            model: String::new(), // 使用 provider 默认模型
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
            max_tokens: Some(1024),
        };

        let resp = llm.complete(req).await?;
        let raw = resp.message.content.trim();

        let command = if let Some(json) = Self::extract_code_json(raw) {
            if let Some(code) = json.get("code").and_then(|v| v.as_str()) {
                Self::build_rust_command(code)
            } else {
                // 回退:把原始输出当作 shell 命令
                raw.into()
            }
        } else {
            // 回退:把原始输出当作 shell 命令
            raw.into()
        };

        Ok(AgentOutput {
            summary: command,
            changed_files: vec![],
            self_check_passed: true,
            log: resp.message.content,
        })
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;
    use axon_core::MemoryId;
    use axon_llm::{
        Capabilities, CompletionRequest, CompletionResponse, Delta, FinishReason, LlmProvider,
        Message, Role, Usage,
    };
    use axon_memory::{Memory, MemoryFilter, RecallQuery};

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

    struct DummyMemory;

    #[async_trait]
    impl MemoryStore for DummyMemory {
        async fn store(&self, _mem: Memory) -> Result<MemoryId> {
            Ok("m1".into())
        }
        async fn recall(&self, _query: &RecallQuery) -> Result<Vec<Memory>> {
            Ok(vec![])
        }
        async fn list(&self, _filter: &MemoryFilter) -> Result<Vec<Memory>> {
            Ok(vec![])
        }
        async fn get(&self, _id: &MemoryId) -> Result<Option<Memory>> {
            Ok(None)
        }
        async fn adjust_weight(&self, _id: &MemoryId, _weight: f32) -> Result<()> {
            Ok(())
        }
        async fn forget(&self, _id: &MemoryId) -> Result<()> {
            Ok(())
        }
    }

    fn sample_task(description: &str) -> Task {
        Task {
            id: "t1".into(),
            parent: None,
            title: "task".into(),
            description: description.into(),
            priority: axon_proto::Priority::Normal,
            state: axon_proto::TaskState::Running,
            dependencies: vec![],
            created_at: 0,
            updated_at: 0,
            acceptance: vec![],
        }
    }

    /// 验证 CommandAgent 把 JSON 中的 code 包装成可执行的 Rust 构建命令。
    #[tokio::test]
    async fn execute_builds_rust_command_from_json() {
        let agent = CommandAgent::new();
        let code = r#"pub fn hello() -> &'static str { "hello world" }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        assert_eq!(hello(), "hello world");
    }
}"#;
        let response = serde_json::json!({ "code": code }).to_string();
        let llm = MockLlm { response };

        let output = agent
            .execute(
                &sample_task("write a hello world function"),
                &llm,
                &DummyMemory,
            )
            .await
            .unwrap();

        assert!(output
            .summary
            .starts_with("cd /workspace && mkdir -p axon_task/src"));
        assert!(output.summary.contains("[package]"));
        assert!(output.summary.contains("[workspace]"));
        assert!(output.summary.contains("base64 -d > src/lib.rs"));
        assert!(output.summary.contains("cargo test"));
    }

    /// 验证 CommandAgent 能处理被 markdown 代码块包裹的 JSON。
    #[tokio::test]
    async fn execute_extracts_json_from_markdown_fence() {
        let agent = CommandAgent::new();
        let response = "```json\n{\"code\":\"pub fn hi() {}\"}\n```".into();
        let llm = MockLlm { response };

        let output = agent
            .execute(&sample_task("test"), &llm, &DummyMemory)
            .await
            .unwrap();

        assert!(output.summary.contains("base64 -d > src/lib.rs"));
    }

    /// 验证当 LLM 未返回合法 JSON 时,CommandAgent 回退使用原始输出。
    #[tokio::test]
    async fn execute_falls_back_to_raw_output() {
        let agent = CommandAgent::new();
        let response = "echo hello".into();
        let llm = MockLlm { response };

        let output = agent
            .execute(&sample_task("test"), &llm, &DummyMemory)
            .await
            .unwrap();

        assert_eq!(output.summary, "echo hello");
    }
}
