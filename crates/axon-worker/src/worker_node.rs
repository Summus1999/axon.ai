//! Worker 节点 / remote worker node.
//!
//! M4 分布式架构中,worker 是一个独立进程,从 `TaskQueue` 拉取任务,
//! 使用 `axon-brain::Agent` 生成并执行命令,然后发布结果和心跳。

use std::sync::Arc;
use std::time::Duration;

use axon_brain::Agent;
use axon_isolation::IsolationProvider;
use axon_llm::LlmProvider;
use axon_memory::MemoryStore;
use axon_proto::Task;
use axon_queue::{
    Heartbeat, RemoteTaskResult, TaskQueue, WorkerEvent, WorkerState as QueueWorkerState,
};

/// Worker 节点配置 / worker node configuration.
#[derive(Debug, Clone)]
pub struct WorkerNodeConfig {
    /// worker 唯一 id / unique worker id.
    pub worker_id: String,
    /// 心跳间隔(秒)/ heartbeat interval in seconds.
    pub heartbeat_interval_secs: u64,
    /// 任务拉取超时(秒)/ task pull timeout in seconds.
    pub pull_timeout_secs: u64,
}

impl Default for WorkerNodeConfig {
    fn default() -> Self {
        Self {
            worker_id: format!("worker-{}", axon_core::new_id()),
            heartbeat_interval_secs: 15,
            pull_timeout_secs: 5,
        }
    }
}

/// 远程 Worker 节点 / remote worker node.
pub struct WorkerNode {
    config: WorkerNodeConfig,
    queue: Arc<dyn TaskQueue>,
    agent: Arc<dyn Agent>,
    llm: Arc<dyn LlmProvider>,
    memory: Arc<dyn MemoryStore>,
    isolation: Arc<dyn IsolationProvider>,
}

impl std::fmt::Debug for WorkerNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerNode")
            .field("config", &self.config)
            .field("queue", &"<dyn TaskQueue>")
            .field("agent", &"<dyn Agent>")
            .field("llm", &"<dyn LlmProvider>")
            .field("memory", &"<dyn MemoryStore>")
            .field("isolation", &"<dyn IsolationProvider>")
            .finish_non_exhaustive()
    }
}

impl WorkerNode {
    /// 创建 worker 节点 / create a worker node.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: WorkerNodeConfig,
        queue: Arc<dyn TaskQueue>,
        agent: Arc<dyn Agent>,
        llm: Arc<dyn LlmProvider>,
        memory: Arc<dyn MemoryStore>,
        isolation: Arc<dyn IsolationProvider>,
    ) -> Self {
        Self {
            config,
            queue,
            agent,
            llm,
            memory,
            isolation,
        }
    }

    /// 运行 worker 节点 / run the worker node until shutdown signal.
    #[tracing::instrument(skip(self, shutdown), fields(worker_id = %self.config.worker_id))]
    pub async fn run(&self, shutdown: tokio::sync::watch::Receiver<bool>) -> axon_core::Result<()> {
        self.register().await?;

        // 心跳任务：周期性上报 Idle 状态。
        let heartbeat_stop = Arc::new(tokio::sync::Notify::new());
        let heartbeat_handle = {
            let queue = self.queue.clone();
            let worker_id = self.config.worker_id.clone();
            let interval = Duration::from_secs(self.config.heartbeat_interval_secs);
            let stop = heartbeat_stop.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(interval);
                loop {
                    tokio::select! {
                        _ = tick.tick() => {
                            let _ = queue.heartbeat(Heartbeat {
                                worker_id: worker_id.clone(),
                                task_id: None,
                                state: QueueWorkerState::Idle,
                                timestamp_ms: now_ms(),
                                load: 0,
                            }).await;
                        }
                        _ = stop.notified() => break,
                    }
                }
            })
        };

        let result = self.task_loop(shutdown).await;

        heartbeat_stop.notify_one();
        let _ = heartbeat_handle.await;
        self.deregister().await?;
        result
    }

    /// 注册 worker / register the worker.
    async fn register(&self) -> axon_core::Result<()> {
        self.queue
            .worker_event(WorkerEvent::Registered {
                worker_id: self.config.worker_id.clone(),
                registered_at_ms: now_ms(),
            })
            .await?;
        tracing::info!(worker_id = %self.config.worker_id, "worker registered");
        Ok(())
    }

    /// 注销 worker / deregister the worker.
    async fn deregister(&self) -> axon_core::Result<()> {
        self.queue
            .worker_event(WorkerEvent::Deregistered {
                worker_id: self.config.worker_id.clone(),
                deregistered_at_ms: now_ms(),
            })
            .await?;
        tracing::info!(worker_id = %self.config.worker_id, "worker deregistered");
        Ok(())
    }

    /// 任务拉取与执行循环 / main task pull-and-execute loop.
    async fn task_loop(
        &self,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> axon_core::Result<()> {
        let pull_timeout = Duration::from_secs(self.config.pull_timeout_secs.max(1));

        loop {
            if *shutdown.borrow() {
                break;
            }

            let task = match self.queue.pull(pull_timeout).await? {
                Some(t) => t,
                None => continue,
            };

            tracing::info!(task_id = %task.id, "worker pulled task");
            self.report_task_heartbeat(Some(&task.id), QueueWorkerState::Running)
                .await;

            let result = self.execute_task(&task).await;
            let remote_result = match result {
                Ok(report) => RemoteTaskResult {
                    task_id: task.id.clone(),
                    worker_id: self.config.worker_id.clone(),
                    success: report.success,
                    summary: report.summary,
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: if report.success { 0 } else { 1 },
                    completed_at_ms: now_ms(),
                },
                Err(e) => RemoteTaskResult {
                    task_id: task.id.clone(),
                    worker_id: self.config.worker_id.clone(),
                    success: false,
                    summary: format!("execution failed: {e}"),
                    stdout: String::new(),
                    stderr: e.to_string(),
                    exit_code: -1,
                    completed_at_ms: now_ms(),
                },
            };

            if let Err(e) = self.queue.complete(remote_result).await {
                tracing::warn!(error = %e, "failed to publish task result");
            }
            self.report_task_heartbeat(None::<&str>, QueueWorkerState::Idle)
                .await;
        }

        Ok(())
    }

    /// 执行单个任务 / execute a single task.
    #[tracing::instrument(skip(self, task), fields(%task.id))]
    async fn execute_task(&self, task: &Task) -> axon_core::Result<crate::WorkReport> {
        let agent_output = self.agent.execute(task, &*self.llm, &*self.memory).await?;

        // 使用 Agent 产出的 summary 作为 shell 命令执行。
        let command = axon_isolation::Command {
            argv: vec!["sh".into(), "-c".into(), agent_output.summary.clone()],
            cwd: Some("/workspace".into()),
            env: vec![],
            timeout_secs: Some(600),
        };

        let vm_spec = axon_isolation::VmSpec {
            vcpus: 1,
            mem_mb: 512,
            rootfs: "alpine:latest".into(),
            kernel: None,
            workspace: None,
            env: vec![],
            network: false,
        };

        let vm = self.isolation.create_vm(vm_spec).await?;
        let exec_result = self.isolation.exec(&vm, command).await;
        let _ = self.isolation.destroy(vm).await;

        let exec_output = exec_result?;
        let success = exec_output.exit_code == 0 && agent_output.self_check_passed;

        Ok(crate::WorkReport {
            task_id: task.id.clone(),
            success,
            summary: agent_output.summary,
            changed_files: agent_output.changed_files,
            log: format!(
                "{}\nstdout:\n{}\nstderr:\n{}",
                agent_output.log, exec_output.stdout, exec_output.stderr
            ),
        })
    }

    /// 上报任务相关心跳 / report a heartbeat tied to the current task.
    async fn report_task_heartbeat(
        &self,
        task_id: Option<impl Into<String>>,
        state: QueueWorkerState,
    ) {
        let beat = Heartbeat {
            worker_id: self.config.worker_id.clone(),
            task_id: task_id.map(Into::into),
            state,
            timestamp_ms: now_ms(),
            load: if state == QueueWorkerState::Running {
                1
            } else {
                0
            },
        };
        if let Err(e) = self.queue.heartbeat(beat).await {
            tracing::warn!(error = %e, "failed to publish heartbeat");
        }
    }
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use futures::StreamExt;

    use axon_brain::AgentOutput;
    use axon_core::MemoryId;
    use axon_llm::{CompletionRequest, CompletionResponse, LlmProvider};
    use axon_memory::{Memory, MemoryFilter, MemoryStore, RecallQuery};
    use axon_proto::Task;
    use axon_queue::{InProcessQueue, TaskQueue};

    use super::*;

    struct DummyAgent;

    #[async_trait]
    impl Agent for DummyAgent {
        async fn execute(
            &self,
            task: &Task,
            _llm: &dyn LlmProvider,
            _memory: &dyn MemoryStore,
        ) -> axon_core::Result<AgentOutput> {
            Ok(AgentOutput {
                summary: format!("echo 'done {}'", task.id),
                changed_files: vec![],
                log: "ok".into(),
                self_check_passed: true,
            })
        }
    }

    struct DummyLlm;

    #[async_trait]
    impl LlmProvider for DummyLlm {
        fn id(&self) -> &str {
            "dummy"
        }
        fn capabilities(&self) -> axon_llm::Capabilities {
            axon_llm::Capabilities::default()
        }
        async fn complete(&self, _req: CompletionRequest) -> axon_core::Result<CompletionResponse> {
            unreachable!()
        }
        async fn stream(&self, _req: CompletionRequest) -> axon_core::Result<Vec<axon_llm::Delta>> {
            unreachable!()
        }
    }

    struct DummyMemory;

    #[async_trait]
    impl MemoryStore for DummyMemory {
        async fn store(&self, _memory: Memory) -> axon_core::Result<MemoryId> {
            Ok("id-1".into())
        }
        async fn recall(&self, _query: &RecallQuery) -> axon_core::Result<Vec<Memory>> {
            Ok(vec![])
        }
        async fn list(&self, _filter: &MemoryFilter) -> axon_core::Result<Vec<Memory>> {
            Ok(vec![])
        }
        async fn get(&self, _id: &MemoryId) -> axon_core::Result<Option<Memory>> {
            Ok(None)
        }
        async fn adjust_weight(&self, _id: &MemoryId, _weight: f32) -> axon_core::Result<()> {
            Ok(())
        }
        async fn forget(&self, _id: &MemoryId) -> axon_core::Result<()> {
            Ok(())
        }
        async fn decay_weights(&self, _half_life_days: f32) -> axon_core::Result<()> {
            Ok(())
        }
    }

    /// 验证 worker 注册事件被发布。
    #[tokio::test]
    async fn worker_registers_on_run() {
        let queue = Arc::new(InProcessQueue::new());
        let (tx, rx) = tokio::sync::watch::channel(false);
        let worker = WorkerNode::new(
            WorkerNodeConfig {
                worker_id: "w-1".into(),
                heartbeat_interval_secs: 60,
                pull_timeout_secs: 1,
            },
            queue.clone(),
            Arc::new(DummyAgent),
            Arc::new(DummyLlm),
            Arc::new(DummyMemory),
            Arc::new(axon_isolation::DockerProvider::new()),
        );

        // 先订阅事件,确保 worker 启动前 receiver 已存在。
        let mut events = queue.subscribe_worker_events().await.unwrap();

        let handle = tokio::spawn(async move { worker.run(rx).await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        tx.send(true).unwrap();
        let _ = handle.await;

        let event = tokio::time::timeout(Duration::from_millis(100), events.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        match event {
            WorkerEvent::Registered { worker_id, .. } => assert_eq!(worker_id, "w-1"),
            _ => panic!("unexpected event"),
        }
    }
}
