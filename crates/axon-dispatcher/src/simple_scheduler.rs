//! 简单调度器 / simple scheduler.
//!
//! M1 实现：组合进程内队列、Docker 隔离、命令生成 Agent 与 LLM，
//! 串行地取出任务 → 创建容器 → Agent 生成命令 → 容器执行 → 销毁容器。
//! 不支持并发（M4）与 DAG 依赖调度（M5）。

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use axon_brain::Agent;
use axon_core::{Result, TaskId};
use axon_isolation::{Command as VmCommand, IsolationProvider, VmSpec};
use axon_llm::LlmProvider;
use axon_memory::{Memory, MemoryKind, MemoryStore};
use axon_proto::{Task, TaskState};

use crate::{Scheduler, SchedulerConfig, TaskQueue};

/// 单个任务的执行结果 / execution result for one task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    /// 任务 ID / task id.
    pub task_id: TaskId,
    /// 容器退出码 / container exit code.
    pub exit_code: i32,
    /// 标准输出 / stdout.
    pub stdout: String,
    /// 标准错误 / stderr.
    pub stderr: String,
}

/// 简单调度器 / simple scheduler.
///
/// 泛型参数:
/// - `I`: 隔离后端（如 `DockerProvider`）
/// - `A`: 任务 Agent（如 `CommandAgent`）
pub struct SimpleScheduler<I, A> {
    queue: Arc<crate::InProcessQueue>,
    isolation: Arc<I>,
    agent: Arc<A>,
    llm: Arc<dyn LlmProvider>,
    memory: Arc<dyn MemoryStore>,
    config: SchedulerConfig,
    vm_spec: VmSpec,
    results: Arc<Mutex<Vec<TaskResult>>>,
}

impl<I, A> SimpleScheduler<I, A>
where
    I: IsolationProvider,
    A: Agent,
{
    /// 创建调度器 / create a scheduler.
    ///
    /// `vm_spec` 作为每个任务启动容器/VM 的模板规格。
    pub fn new(
        queue: Arc<crate::InProcessQueue>,
        isolation: Arc<I>,
        agent: Arc<A>,
        llm: Arc<dyn LlmProvider>,
        memory: Arc<dyn MemoryStore>,
        config: SchedulerConfig,
        vm_spec: VmSpec,
    ) -> Self {
        Self {
            queue,
            isolation,
            agent,
            llm,
            memory,
            config,
            vm_spec,
            results: Arc::new(Mutex::new(vec![])),
        }
    }

    /// 获取已收集的任务执行结果 / get collected task results.
    pub async fn results(&self) -> Vec<TaskResult> {
        self.results.lock().await.clone()
    }

    /// 处理单个任务 / process one task end-to-end.
    async fn process_task(&self, task: Task) -> Result<()> {
        let vm = match self.isolation.create_vm(self.vm_spec.clone()).await {
            Ok(vm) => vm,
            Err(e) => {
                self.queue.update_state(&task.id, TaskState::Failed).await?;
                return Err(e);
            }
        };

        // Agent 生成要在容器内执行的命令。
        let agent_output = match self.agent.execute(&task, &*self.llm, &*self.memory).await {
            Ok(out) => out,
            Err(e) => {
                let _ = self.isolation.destroy(vm).await;
                self.queue.update_state(&task.id, TaskState::Failed).await?;
                return Err(e);
            }
        };

        let command = VmCommand {
            argv: vec!["sh".into(), "-c".into(), agent_output.summary.clone()],
            cwd: Some("/workspace".into()),
            env: vec![],
            timeout_secs: Some(self.config.task_timeout_secs),
        };
        tracing::info!(command = %agent_output.summary, "executing generated command");

        let exec_result = self.isolation.exec(&vm, command).await;
        let (state, result_opt) = match &exec_result {
            Ok(out) => (
                if out.exit_code == 0 {
                    TaskState::Completed
                } else {
                    TaskState::Failed
                },
                Some(out.clone()),
            ),
            Err(_) => (TaskState::Failed, None),
        };

        if let Some(out) = result_opt {
            self.results.lock().await.push(TaskResult {
                task_id: task.id.clone(),
                exit_code: out.exit_code,
                stdout: out.stdout,
                stderr: out.stderr,
            });
        }

        if let Err(e) = self.isolation.destroy(vm).await {
            tracing::warn!(error = %e, "failed to destroy vm");
        }
        self.queue.update_state(&task.id, state).await?;

        // 将本次任务沉淀为情景记忆，供未来 recall。
        let now = now_ms();
        let mem = Memory {
            id: String::new(),
            kind: MemoryKind::Episodic,
            content: format!(
                "Task: {}\nCommand: {}",
                task.description, agent_output.summary
            ),
            embedding: None,
            weight: if matches!(state, TaskState::Completed) {
                1.2
            } else {
                0.8
            },
            created_at: now,
            updated_at: now,
            source: Some(format!("task-{}", task.id)),
        };
        if let Err(e) = self.memory.store(mem).await {
            tracing::warn!(error = %e, "failed to store episodic memory");
        }

        exec_result.map(|_| ())
    }
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[async_trait]
impl<I, A> Scheduler for SimpleScheduler<I, A>
where
    I: IsolationProvider,
    A: Agent,
{
    async fn submit(&self, tasks: Vec<Task>, _deps: Vec<(TaskId, TaskId)>) -> Result<()> {
        // M1 忽略依赖，直接全部入队。
        for task in tasks {
            self.queue.enqueue(task).await?;
        }
        Ok(())
    }

    async fn run(&self) -> Result<()> {
        loop {
            match self.queue.dequeue().await {
                Ok(task) => {
                    if let Err(e) = self.process_task(task).await {
                        tracing::warn!(error = %e, "task processing failed");
                    }
                }
                Err(axon_core::Error::Dispatcher(_)) => {
                    // 队列为空，调度结束。
                    break;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use axon_brain::AgentOutput;
    use axon_core::MemoryId;
    use axon_isolation::{Backend, ExecOutput, Snapshot, VmHandle};
    use axon_llm::{
        Capabilities, CompletionRequest, CompletionResponse, Delta, FinishReason, LlmProvider,
        Message, Role, Usage,
    };
    use axon_memory::{Memory, MemoryFilter, RecallQuery};

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
            Ok(CompletionResponse {
                message: Message {
                    role: Role::Assistant,
                    content: "echo done".into(),
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

    struct MockAgent;

    #[async_trait]
    impl Agent for MockAgent {
        async fn execute(
            &self,
            _task: &Task,
            _llm: &dyn LlmProvider,
            _memory: &dyn MemoryStore,
        ) -> Result<AgentOutput> {
            Ok(AgentOutput {
                summary: "echo hello".into(),
                changed_files: vec![],
                self_check_passed: true,
                log: String::new(),
            })
        }
    }

    struct MockIsolation {
        created: AtomicBool,
        exec_success: AtomicBool,
    }

    #[async_trait]
    impl IsolationProvider for MockIsolation {
        fn backend(&self) -> Backend {
            Backend::Docker
        }
        async fn create_vm(&self, _spec: VmSpec) -> Result<VmHandle> {
            self.created.store(true, Ordering::SeqCst);
            Ok(VmHandle {
                id: "vm-1".into(),
                backend: Backend::Docker,
            })
        }
        async fn exec(&self, _vm: &VmHandle, _cmd: VmCommand) -> Result<ExecOutput> {
            Ok(ExecOutput {
                exit_code: if self.exec_success.load(Ordering::SeqCst) {
                    0
                } else {
                    1
                },
                stdout: "hello".into(),
                stderr: String::new(),
            })
        }
        async fn snapshot(&self, _vm: &VmHandle) -> Result<Snapshot> {
            unreachable!()
        }
        async fn destroy(&self, _vm: VmHandle) -> Result<()> {
            Ok(())
        }
    }

    fn sample_task(id: &str) -> Task {
        Task {
            id: id.into(),
            parent: None,
            title: "t".into(),
            description: "desc".into(),
            priority: axon_proto::Priority::Normal,
            state: TaskState::Queued,
            dependencies: vec![],
            created_at: 0,
            updated_at: 0,
            acceptance: vec![],
        }
    }

    /// 验证调度器能完整跑通一个任务。
    #[tokio::test]
    async fn scheduler_runs_single_task() {
        let queue = Arc::new(crate::InProcessQueue::new());
        let isolation = Arc::new(MockIsolation {
            created: AtomicBool::new(false),
            exec_success: AtomicBool::new(true),
        });
        let scheduler = SimpleScheduler::new(
            queue.clone(),
            isolation.clone(),
            Arc::new(MockAgent),
            Arc::new(DummyLlm),
            Arc::new(DummyMemory),
            SchedulerConfig::default(),
            VmSpec {
                vcpus: 1,
                mem_mb: 256,
                rootfs: "alpine:latest".into(),
                kernel: None,
                workspace: None,
                env: vec![],
                network: false,
            },
        );

        scheduler
            .submit(vec![sample_task("t1")], vec![])
            .await
            .unwrap();
        scheduler.run().await.unwrap();

        assert!(isolation.created.load(Ordering::SeqCst));
    }
}
