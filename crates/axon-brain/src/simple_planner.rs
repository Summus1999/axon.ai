//! 简单规划器 / simple planner.
//!
//! M1 占位实现：把用户目标直接封装为单个任务，不调用 LLM 做复杂拆解。
//! 用于先跑通 `CLI → Planner → Dispatcher → Worker` 的完整链路。

use async_trait::async_trait;

use axon_core::{Result, TaskId};
use axon_llm::LlmProvider;
use axon_memory::{MemoryKind, MemoryStore, RecallQuery};
use axon_proto::{Priority, Task, TaskState};

use crate::{Goal, Plan, Planner};

/// 简单规划器：一个目标对应一个任务 / simple planner: one goal, one task.
#[derive(Debug, Default)]
pub struct SimplePlanner;

impl SimplePlanner {
    /// 创建一个新的简单规划器 / create a new simple planner.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Planner for SimplePlanner {
    async fn plan(
        &self,
        goal: &Goal,
        memory: &dyn MemoryStore,
        _llm: &dyn LlmProvider,
    ) -> Result<Plan> {
        let now = axon_core::Timestamp::default();
        let task_id: TaskId = axon_core::new_id();

        // 召回用户画像与语义记忆，拼入任务描述以个性化规划。
        let memories = memory
            .recall(&RecallQuery {
                query: goal.description.clone(),
                kind: Some(MemoryKind::UserProfile),
                top_k: 3,
            })
            .await?;
        let semantic = memory
            .recall(&RecallQuery {
                query: goal.description.clone(),
                kind: Some(MemoryKind::Semantic),
                top_k: 3,
            })
            .await?;

        let description = if memories.is_empty() && semantic.is_empty() {
            goal.description.clone()
        } else {
            let mut lines = vec!["Relevant memories:".to_string()];
            for m in memories.iter().chain(semantic.iter()) {
                lines.push(format!("- {}", m.content));
            }
            lines.push(format!("\nTask: {}", goal.description));
            lines.join("\n")
        };

        let task = Task {
            id: task_id.clone(),
            parent: None,
            title: goal.description.clone(),
            description,
            priority: Priority::Normal,
            state: TaskState::Queued,
            dependencies: vec![],
            created_at: now,
            updated_at: now,
            acceptance: vec!["command executes without error".into()],
        };

        Ok(Plan {
            tasks: vec![task],
            dependencies: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_llm::{Capabilities, CompletionRequest, CompletionResponse, Delta, LlmProvider};

    struct DummyLlm;

    #[async_trait]
    impl LlmProvider for DummyLlm {
        fn id(&self) -> &str {
            "dummy"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse> {
            unreachable!("simple planner should not call llm")
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

    /// 验证简单规划器把单个目标转换为一个任务。
    #[tokio::test]
    async fn plan_creates_single_task() {
        let planner = SimplePlanner::new();
        let goal = Goal {
            description: "create hello.txt".into(),
            context: vec![],
        };
        let plan = planner.plan(&goal, &DummyMemory, &DummyLlm).await.unwrap();
        assert_eq!(plan.tasks.len(), 1);
        assert_eq!(plan.tasks[0].description, "create hello.txt");
        assert!(plan.dependencies.is_empty());
    }
}
