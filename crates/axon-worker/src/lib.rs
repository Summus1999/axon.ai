//! axon-worker — 任务执行 Worker / the per-VM agent worker.
//!
//! 运行在每个隔离环境(microVM/container)内,接收 dispatcher 下发的单个
//! [`Task`],借助 LLM + 记忆执行开发任务,自检通过后回报结果。
//!
//! 具体实现(M1)使用 `axon-brain::Agent`。

use axon_brain::Agent;
use axon_core::Result;
use axon_llm::LlmProvider;
use axon_memory::MemoryStore;
use axon_proto::Task;

/// Worker 配置 / worker configuration.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// worker 实例 id / worker instance id.
    pub worker_id: String,
    /// 心跳间隔(秒)/ heartbeat interval in seconds.
    pub heartbeat_interval_secs: u32,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            worker_id: "worker-0".into(),
            heartbeat_interval_secs: 15,
        }
    }
}

/// Worker 入口:执行一个任务并回报 / run a single task to completion.
///
/// 调用 `Agent::execute` 完成实际工作，并把 `AgentOutput` 转换为 `WorkReport`。
/// M1 为单机进程内执行；M3 后 worker 将运行在 microVM 内。
pub async fn run_task(
    _config: &WorkerConfig,
    task: &Task,
    agent: &dyn Agent,
    llm: &dyn LlmProvider,
    memory: &dyn MemoryStore,
) -> Result<WorkReport> {
    let output = agent.execute(task, llm, memory).await?;
    Ok(WorkReport {
        task_id: task.id.clone(),
        success: output.self_check_passed,
        summary: output.summary,
        changed_files: output.changed_files,
        log: output.log,
    })
}

/// 任务执行回报 / a task completion report from a worker.
#[derive(Debug, Clone)]
pub struct WorkReport {
    pub task_id: axon_core::TaskId,
    pub success: bool,
    pub summary: String,
    pub changed_files: Vec<String>,
    pub log: String,
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;
    use axon_brain::AgentOutput;
    use axon_core::MemoryId;
    use axon_llm::{Capabilities, CompletionRequest, CompletionResponse, Delta, LlmProvider};
    use axon_memory::{Memory, MemoryFilter, RecallQuery};

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
            unreachable!()
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

    struct MockAgent;

    #[async_trait]
    impl Agent for MockAgent {
        async fn execute(
            &self,
            task: &Task,
            _llm: &dyn LlmProvider,
            _memory: &dyn MemoryStore,
        ) -> Result<AgentOutput> {
            Ok(AgentOutput {
                summary: format!("executed {}", task.id),
                changed_files: vec!["file.txt".into()],
                self_check_passed: true,
                log: "ok".into(),
            })
        }
    }

    fn sample_task(id: &str) -> Task {
        Task {
            id: id.into(),
            parent: None,
            title: "t".into(),
            description: "desc".into(),
            priority: axon_proto::Priority::Normal,
            state: axon_proto::TaskState::Running,
            dependencies: vec![],
            created_at: 0,
            updated_at: 0,
            acceptance: vec![],
        }
    }

    /// 验证 run_task 把 AgentOutput 正确转换为 WorkReport。
    #[tokio::test]
    async fn run_task_converts_output() {
        let config = WorkerConfig::default();
        let task = sample_task("t1");
        let report = run_task(&config, &task, &MockAgent, &DummyLlm, &DummyMemory)
            .await
            .unwrap();
        assert_eq!(report.task_id, "t1");
        assert!(report.success);
        assert_eq!(report.summary, "executed t1");
        assert_eq!(report.changed_files, vec!["file.txt"]);
    }
}
