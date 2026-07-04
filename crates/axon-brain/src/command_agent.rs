//! 命令生成 Agent / command-generating agent.
//!
//! M1 实现：根据任务描述让 LLM 生成一个 shell 命令，
//! 由调度器在 Docker 容器内执行。不实现 ReAct 多步循环（M5）。

use async_trait::async_trait;

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

    /// 清洗 LLM 输出：去掉首尾空白与 Markdown 代码块标记。
    fn clean_command(raw: &str) -> String {
        let trimmed = raw.trim();
        let without_fences = trimmed
            .strip_prefix("```bash")
            .or_else(|| trimmed.strip_prefix("```sh"))
            .or_else(|| trimmed.strip_prefix("```shell"))
            .or_else(|| trimmed.strip_prefix("```"))
            .unwrap_or(trimmed);
        without_fences
            .strip_suffix("```")
            .unwrap_or(without_fences)
            .trim()
            .into()
    }
}

#[async_trait]
impl Agent for CommandAgent {
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

        let system = "You are a helpful coding assistant. Given a task and relevant memories, output a single shell command that accomplishes it. Output only the command, no explanation, no markdown.";
        let user = format!(
            "{memory_context}Task: {}\n\nShell command:",
            task.description
        );

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
            temperature: 0.2,
            max_tokens: Some(256),
        };

        let resp = llm.complete(req).await?;
        let command = Self::clean_command(&resp.message.content);

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
    use super::*;
    use axon_llm::{
        Capabilities, CompletionRequest, CompletionResponse, Delta, FinishReason, LlmProvider,
        Message, Role, Usage,
    };

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
        async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse> {
            assert!(req.messages.iter().any(|m| m.role == Role::User));
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
        async fn store(&self, _mem: axon_memory::Memory) -> Result<axon_core::MemoryId> {
            Ok("m1".into())
        }
        async fn recall(
            &self,
            _query: &axon_memory::RecallQuery,
        ) -> Result<Vec<axon_memory::Memory>> {
            Ok(vec![])
        }
        async fn list(
            &self,
            _filter: &axon_memory::MemoryFilter,
        ) -> Result<Vec<axon_memory::Memory>> {
            Ok(vec![])
        }
        async fn get(&self, _id: &axon_core::MemoryId) -> Result<Option<axon_memory::Memory>> {
            Ok(None)
        }
        async fn adjust_weight(&self, _id: &axon_core::MemoryId, _weight: f32) -> Result<()> {
            Ok(())
        }
        async fn forget(&self, _id: &axon_core::MemoryId) -> Result<()> {
            Ok(())
        }
    }

    /// 验证 CommandAgent 从 LLM 响应中提取出纯命令。
    #[tokio::test]
    async fn execute_extracts_command() {
        let agent = CommandAgent::new();
        let task = Task {
            id: "t1".into(),
            parent: None,
            title: "hello".into(),
            description: "create a hello.txt file".into(),
            priority: axon_proto::Priority::Normal,
            state: axon_proto::TaskState::Running,
            dependencies: vec![],
            created_at: 0,
            updated_at: 0,
            acceptance: vec![],
        };
        let llm = MockLlm {
            response: "```bash\ntouch hello.txt\n```".into(),
        };

        let output = agent.execute(&task, &llm, &DummyMemory).await.unwrap();
        assert_eq!(output.summary, "touch hello.txt");
        assert!(output.self_check_passed);
    }

    /// 验证 clean_command 能处理多种 markdown 代码块。
    #[test]
    fn clean_command_strips_fences() {
        assert_eq!(
            CommandAgent::clean_command("```sh\necho hi\n```"),
            "echo hi"
        );
        assert_eq!(CommandAgent::clean_command("  echo hi  "), "echo hi");
    }
}
