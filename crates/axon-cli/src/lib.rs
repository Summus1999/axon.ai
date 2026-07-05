//! axon-cli 库 / the axon-cli library.
//!
//! 把 CLI 的端到端流程暴露为可测试的库函数，便于集成测试调用。
//! 生产入口仍由 `src/main.rs` 的 `axon` 二进制提供。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axon_brain::{CommandAgent, Goal, Planner, SimplePlanner};
use axon_core::Config;
use axon_dispatcher::{
    remote_dispatcher::{TaskRecord as RemoteTaskRecord, WorkerRecord as RemoteWorkerRecord},
    vm_registry::InMemoryVmRegistry,
    InProcessQueue, Scheduler, SchedulerConfig, SimpleScheduler, TaskResult,
};
use axon_isolation::{DockerProvider, VmSpec};
use axon_memory::{HybridMemoryStore, InMemoryStore, MemoryStore, QdrantStore, RedbStore};
use axon_proto::{Priority, Task, TaskState};
use axon_queue::{NatsQueue, RemoteTaskResult, ResultFilter, TaskQueue};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::time::{timeout, Duration};

/// 根据全局配置创建记忆存储 / create a memory store from the global config.
///
/// 后端由 `memory.backend` 决定:
/// - `memory`(默认): 进程内 `InMemoryStore`,不依赖外部服务
/// - `hybrid`: `HybridMemoryStore` = `RedbStore` + `QdrantStore`(需要 embedding provider)
pub async fn create_memory_store_from_config(cfg: &Config) -> anyhow::Result<Arc<dyn MemoryStore>> {
    match cfg.memory.backend.as_str() {
        "memory" => Ok(Arc::new(InMemoryStore::new())),
        "hybrid" => create_hybrid_memory_store_from_config(cfg).await,
        other => Err(anyhow::anyhow!("unsupported memory backend: {other}")),
    }
}

/// 根据配置创建混合记忆存储 / create the hybrid memory store from config.
async fn create_hybrid_memory_store_from_config(
    cfg: &Config,
) -> anyhow::Result<Arc<dyn MemoryStore>> {
    let redb_path = cfg
        .memory
        .kv_path
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".axon/memory.redb"));
    let qdrant_url = cfg
        .memory
        .qdrant_url
        .clone()
        .unwrap_or_else(|| "http://localhost:6334".into());
    let qdrant_collection = cfg.memory.qdrant_collection.clone();

    let embedder: Arc<dyn axon_llm::EmbeddingProvider> =
        axon_llm::create_embedding_provider_from_env()
            .map_err(|e| anyhow::anyhow!("failed to create embedding provider: {e}"))?
            .into();

    let semantic = RedbStore::new(&redb_path)
        .map_err(|e| anyhow::anyhow!("failed to open redb store: {e}"))?;
    let episodic = QdrantStore::new(qdrant_url, qdrant_collection, embedder)
        .await
        .map_err(|e| anyhow::anyhow!("failed to connect Qdrant store: {e}"))?;

    Ok(Arc::new(HybridMemoryStore::new(semantic, episodic)))
}

/// 执行单个目标 / execute a single goal end-to-end.
///
/// `workspace_dir` 会挂载到容器内的 `/workspace`;`image` 为容器镜像。
/// 返回每个任务的执行结果,便于测试断言。
pub async fn run_goal(
    goal: &str,
    workspace_dir: &Path,
    image: &str,
) -> anyhow::Result<Vec<TaskResult>> {
    tracing::info!(%goal, "提交任务");

    let llm = axon_llm::create_provider_from_env()
        .map_err(|e| anyhow::anyhow!("failed to create LLM provider: {e}"))?;
    let config = Config::load().map_err(|e| anyhow::anyhow!("failed to load config: {e}"))?;
    let memory = create_memory_store_from_config(&config).await?;
    let planner = SimplePlanner::new();

    let plan = planner
        .plan(
            &Goal {
                description: goal.into(),
                context: vec![],
            },
            &*memory,
            &*llm,
        )
        .await
        .map_err(|e| anyhow::anyhow!("planning failed: {e}"))?;

    tracing::info!(task_count = plan.tasks.len(), "规划完成");

    let queue = Arc::new(InProcessQueue::new());
    let isolation = Arc::new(DockerProvider::new());
    let agent = Arc::new(CommandAgent::new());
    let vm_registry = Arc::new(InMemoryVmRegistry::with_snapshot(vm_records_path()));
    let scheduler = SimpleScheduler::with_options(
        queue,
        isolation,
        agent,
        llm,
        memory,
        Some(Arc::new(axon_brain::LlmProfileExtractor::new())),
        None::<Arc<dyn axon_brain::Reviewer>>,
        Some(vm_registry),
        SchedulerConfig {
            max_concurrency: 1,
            task_timeout_secs: 600,
            max_retries: 0,
        },
        VmSpec {
            vcpus: 1,
            mem_mb: 512,
            rootfs: image.into(),
            kernel: None,
            workspace: Some(workspace_dir.to_string_lossy().to_string()),
            env: vec![],
            network: false,
        },
    );

    scheduler
        .submit(plan.tasks, plan.dependencies)
        .await
        .map_err(|e| anyhow::anyhow!("submit failed: {e}"))?;

    scheduler
        .run()
        .await
        .map_err(|e| anyhow::anyhow!("scheduler failed: {e}"))?;

    let results = scheduler.results().await;
    save_task_records(&results)?;

    Ok(results)
}

/// VM 状态记录 / persisted VM status record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmRecord {
    /// VM id / vm id.
    pub vm_id: String,
    /// 隔离后端 / isolation backend.
    pub backend: String,
    /// 当前任务 id / current task id.
    pub task_id: String,
    /// 创建时间(Unix 秒)/ created at.
    pub created_at: u64,
    /// 最新心跳时间(Unix 毫秒)/ latest heartbeat timestamp.
    pub last_heartbeat_ms: Option<u64>,
}

/// 任务状态记录 / persisted task status record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    /// 任务 ID / task id.
    pub task_id: String,
    /// 任务状态 / task status.
    pub status: String,
    /// 容器退出码 / container exit code.
    pub exit_code: i32,
    /// 标准输出摘要 / stdout summary.
    pub stdout: String,
    /// 标准错误摘要 / stderr summary.
    pub stderr: String,
    /// 完成时间 / finished at.
    pub finished_at: String,
}

impl TaskRecord {
    /// 从 `TaskResult` 创建记录 / create a record from a task result.
    fn from_result(result: &TaskResult) -> Self {
        let status = if result.exit_code == 0 {
            "completed".into()
        } else {
            "failed".into()
        };
        Self {
            task_id: result.task_id.clone(),
            status,
            exit_code: result.exit_code,
            stdout: result.stdout.trim().into(),
            stderr: result.stderr.trim().into(),
            finished_at: now_rfc3339(),
        }
    }
}

/// 任务记录默认存储路径 / default task records path.
pub fn task_records_path() -> PathBuf {
    PathBuf::from(".axon/tasks.json")
}

/// VM 记录默认存储路径 / default VM records path.
pub fn vm_records_path() -> PathBuf {
    PathBuf::from(".axon/vms.json")
}

/// 从本地文件加载 VM 记录 / load VM records from local file.
///
/// 若文件不存在则返回空列表。
pub fn list_vms() -> anyhow::Result<Vec<VmRecord>> {
    let path = vm_records_path();
    if !path.is_file() {
        return Ok(vec![]);
    }

    let content = fs::read_to_string(&path)?;
    let records: Vec<VmRecord> = serde_json::from_str(&content)?;
    Ok(records)
}

/// 保存任务记录到本地文件 / persist task records to local file.
///
/// 文件保存在 `.axon/tasks.json`,便于 `axon tasks` 查看历史状态。
pub fn save_task_records(results: &[TaskResult]) -> anyhow::Result<()> {
    let path = task_records_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let records: Vec<TaskRecord> = results.iter().map(TaskRecord::from_result).collect();
    let json = serde_json::to_string_pretty(&records)?;
    fs::write(&path, json)?;
    Ok(())
}

/// 从本地文件加载任务记录 / load task records from local file.
///
/// 若文件不存在则返回空列表。
pub fn list_tasks() -> anyhow::Result<Vec<TaskRecord>> {
    let path = task_records_path();
    if !path.is_file() {
        return Ok(vec![]);
    }

    let content = fs::read_to_string(&path)?;
    let records: Vec<TaskRecord> = serde_json::from_str(&content)?;
    Ok(records)
}

/// 生成唯一任务 id / generate a unique task id.
fn generate_task_id() -> String {
    format!("task-{}", uuid::Uuid::new_v4())
}

/// 提交一个远程任务到 NATS 队列并等待结果 / submit a remote task and wait for its result.
///
/// `goal` 会被直接作为任务标题与描述;`nats_url` 用于任务投递与结果订阅;
/// `dispatcher_url` 当前保留给未来 HTTP 状态查询,本函数暂未使用。
pub async fn run_remote(
    goal: &str,
    nats_url: &str,
    _dispatcher_url: &str,
) -> anyhow::Result<RemoteTaskResult> {
    let queue = Arc::new(NatsQueue::connect(nats_url).await?);
    run_remote_with_queue(goal, queue).await
}

/// 使用给定队列提交远程任务并等待结果 / submit a remote task using the provided queue.
///
/// 将底层队列实现与 CLI 流程解耦,便于单元测试使用内存队列。
async fn run_remote_with_queue(
    goal: &str,
    queue: Arc<dyn TaskQueue>,
) -> anyhow::Result<RemoteTaskResult> {
    run_remote_with_queue_and_id(goal, queue, generate_task_id(), Duration::from_secs(60)).await
}

/// 使用给定队列与指定任务 id 提交远程任务并等待结果 / submit a remote task with explicit id.
///
/// `timeout_dur` 控制等待结果的最大时长,便于测试使用较短超时。
async fn run_remote_with_queue_and_id(
    goal: &str,
    queue: Arc<dyn TaskQueue>,
    task_id: String,
    timeout_dur: Duration,
) -> anyhow::Result<RemoteTaskResult> {
    let task = Task {
        id: task_id.clone(),
        parent: None,
        title: goal.into(),
        description: goal.into(),
        priority: Priority::Normal,
        state: TaskState::Queued,
        dependencies: vec![],
        created_at: now_ms(),
        updated_at: now_ms(),
        acceptance: vec![],
    };

    queue.submit(task).await?;
    tracing::info!(%task_id, "远程任务已提交到队列");

    let mut stream = queue
        .subscribe_results(ResultFilter::Task(task_id.clone()))
        .await?;
    let result = timeout(timeout_dur, stream.next())
        .await
        .map_err(|_| anyhow::anyhow!("等待远程任务 {task_id} 结果超时"))?;

    match result {
        Some(Ok(r)) => Ok(r),
        Some(Err(e)) => Err(anyhow::anyhow!("结果流返回错误: {e}")),
        None => Err(anyhow::anyhow!("结果流在收到结果前已关闭")),
    }
}

/// 从远程 dispatcher 查询任务列表 / list remote tasks from the dispatcher HTTP API.
pub async fn list_remote_tasks(dispatcher_url: &str) -> anyhow::Result<Vec<RemoteTaskRecord>> {
    let url = format!("{dispatcher_url}/tasks");
    let records: Vec<RemoteTaskRecord> = reqwest::get(&url).await?.json().await?;
    Ok(records)
}

/// 从远程 dispatcher 查询 worker 列表 / list remote workers from the dispatcher HTTP API.
pub async fn list_remote_workers(dispatcher_url: &str) -> anyhow::Result<Vec<RemoteWorkerRecord>> {
    let url = format!("{dispatcher_url}/workers");
    let records: Vec<RemoteWorkerRecord> = reqwest::get(&url).await?.json().await?;
    Ok(records)
}

fn now_rfc3339() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".into())
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
    use std::sync::Mutex;

    use axon_queue::{InProcessQueue, WorkerState};

    use super::*;

    /// 串行化会修改环境变量/工作目录的测试。
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// 验证任务记录可以保存并重新加载。
    #[test]
    fn save_and_load_task_records_roundtrip() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(&tmp).unwrap();

        let results = vec![TaskResult {
            task_id: "t1".into(),
            exit_code: 0,
            stdout: "hello".into(),
            stderr: "".into(),
        }];
        save_task_records(&results).unwrap();

        let records = list_tasks().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].task_id, "t1");
        assert_eq!(records[0].status, "completed");
        assert_eq!(records[0].exit_code, 0);

        std::env::set_current_dir(original).unwrap();
    }

    /// 当任务记录文件不存在时,`list_tasks` 返回空列表。
    #[test]
    fn list_tasks_returns_empty_when_missing() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(&tmp).unwrap();

        let records = list_tasks().unwrap();
        assert!(records.is_empty());

        std::env::set_current_dir(original).unwrap();
    }

    /// `memory` backend 返回可用的 InMemoryStore,不依赖外部服务。
    #[tokio::test]
    async fn memory_backend_creates_in_memory_store() {
        let mut cfg = Config::default();
        cfg.memory.backend = "memory".into();

        let store = create_memory_store_from_config(&cfg).await.unwrap();
        let id = store
            .store(axon_memory::Memory {
                id: String::new(),
                kind: axon_memory::MemoryKind::ShortTerm,
                content: "hello test".into(),
                embedding: None,
                weight: 1.0,
                created_at: 0,
                updated_at: 0,
                source: None,
            })
            .await
            .unwrap();
        assert!(!id.is_empty());
    }

    /// 未知 backend 返回明确错误。
    #[tokio::test]
    async fn unknown_backend_returns_error() {
        let mut cfg = Config::default();
        cfg.memory.backend = "unknown".into();

        let result = create_memory_store_from_config(&cfg).await;
        assert!(result.is_err(), "unknown backend should fail");
    }

    /// `hybrid` backend 在缺少 embedding provider 时返回错误。
    #[tokio::test]
    async fn hybrid_backend_fails_without_embedding_provider() {
        {
            let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            for key in ["OPENAI_API_KEY", "GLM_API_KEY", "EMBEDDING_PROVIDER"] {
                std::env::remove_var(key);
            }
        }

        let mut cfg = Config::default();
        cfg.memory.backend = "hybrid".into();

        let result = create_memory_store_from_config(&cfg).await;
        assert!(
            result.is_err(),
            "hybrid backend should fail when no embedder is available"
        );
    }

    /// VM 记录文件不存在时,`list_vms` 返回空列表。
    #[test]
    fn list_vms_returns_empty_when_missing() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(&tmp).unwrap();

        let records = list_vms().unwrap();
        assert!(records.is_empty());

        std::env::set_current_dir(original).unwrap();
    }

    /// VM 记录可保存并重新加载。
    #[test]
    fn save_and_load_vm_records_roundtrip() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(&tmp).unwrap();

        let records = vec![VmRecord {
            vm_id: "vm-1".into(),
            backend: "docker".into(),
            task_id: "t1".into(),
            created_at: 1000,
            last_heartbeat_ms: Some(2000),
        }];
        let path = vm_records_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let json = serde_json::to_string_pretty(&records).unwrap();
        fs::write(path, json).unwrap();

        let loaded = list_vms().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].vm_id, "vm-1");
        assert_eq!(loaded[0].backend, "docker");

        std::env::set_current_dir(original).unwrap();
    }

    /// 远程任务提交后收到结果 / remote task receives its result from the queue.
    #[tokio::test]
    async fn run_remote_with_queue_returns_result() {
        let queue = Arc::new(InProcessQueue::new());
        let queue_clone = Arc::clone(&queue);
        let task_id = "task-1".to_string();
        let task_id_for_complete = task_id.clone();

        // 在后台模拟 worker:任务入队后发布匹配 task_id 的结果。
        let handle = tokio::spawn(async move {
            // 给订阅方一点时间建立。
            tokio::time::sleep(Duration::from_millis(10)).await;
            queue_clone
                .complete(RemoteTaskResult {
                    task_id: task_id_for_complete,
                    worker_id: "worker-1".into(),
                    success: true,
                    summary: "done".into(),
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: 0,
                    completed_at_ms: 100,
                })
                .await
                .unwrap();
        });

        let result =
            run_remote_with_queue_and_id("test goal", queue, task_id, Duration::from_secs(1))
                .await
                .expect("应收到结果");
        handle.await.unwrap();

        assert!(result.success);
        assert_eq!(result.worker_id, "worker-1");
    }

    /// 远程任务在结果流关闭时返回错误 / run_remote reports error when result stream closes early.
    #[tokio::test]
    async fn run_remote_reports_error_on_closed_stream() {
        let queue = Arc::new(InProcessQueue::new());
        let result = run_remote_with_queue_and_id(
            "closed stream",
            queue,
            "task-x".into(),
            Duration::from_millis(100),
        )
        .await;
        assert!(result.is_err(), "空队列无结果,应返回超时错误");
    }

    /// 从 dispatcher HTTP API 拉取任务列表 / list_remote_tasks fetches tasks from HTTP endpoint.
    #[tokio::test]
    async fn list_remote_tasks_returns_records() {
        let app = axum::Router::new().route(
            "/tasks",
            axum::routing::get(|| async {
                axum::Json(vec![RemoteTaskRecord {
                    task_id: "t1".into(),
                    state: TaskState::Queued,
                    title: "task".into(),
                    description: "desc".into(),
                    worker_id: None,
                    created_at_ms: 1,
                    updated_at_ms: 2,
                }])
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let url = format!("http://{addr}");
        let records = list_remote_tasks(&url).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].task_id, "t1");
    }

    /// 从 dispatcher HTTP API 拉取 worker 列表 / list_remote_workers fetches workers from HTTP endpoint.
    #[tokio::test]
    async fn list_remote_workers_returns_records() {
        let app = axum::Router::new().route(
            "/workers",
            axum::routing::get(|| async {
                axum::Json(vec![RemoteWorkerRecord {
                    worker_id: "w1".into(),
                    state: WorkerState::Idle,
                    task_id: None,
                    last_heartbeat_ms: 100,
                    load: 0,
                    registered_at_ms: 1,
                }])
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let url = format!("http://{addr}");
        let workers = list_remote_workers(&url).await.unwrap();
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].worker_id, "w1");
    }

    /// 无法连接的 dispatcher URL 返回错误 / list_remote_tasks reports error for unreachable URL.
    #[tokio::test]
    async fn list_remote_tasks_reports_unreachable_error() {
        let result = list_remote_tasks("http://127.0.0.1:1").await;
        assert!(result.is_err(), "不可达地址应返回错误");
    }
}
