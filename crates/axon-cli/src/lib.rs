//! axon-cli 库 / the axon-cli library.
//!
//! 把 CLI 的端到端流程暴露为可测试的库函数，便于集成测试调用。
//! 生产入口仍由 `src/main.rs` 的 `axon` 二进制提供。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axon_brain::{CommandAgent, Goal, Planner, SimplePlanner};
use axon_core::Config;
use axon_dispatcher::{InProcessQueue, Scheduler, SchedulerConfig, SimpleScheduler, TaskResult};
use axon_isolation::{DockerProvider, VmSpec};
use axon_memory::{HybridMemoryStore, InMemoryStore, MemoryStore, QdrantStore, RedbStore};
use serde::{Deserialize, Serialize};

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
    let scheduler = SimpleScheduler::new(
        queue,
        isolation,
        agent,
        llm,
        memory,
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

fn now_rfc3339() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".into())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

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
}
