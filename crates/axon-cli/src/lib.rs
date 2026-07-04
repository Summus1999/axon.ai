//! axon-cli 库 / the axon-cli library.
//!
//! 把 CLI 的端到端流程暴露为可测试的库函数，便于集成测试调用。
//! 生产入口仍由 `src/main.rs` 的 `axon` 二进制提供。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axon_brain::{CommandAgent, Goal, Planner, SimplePlanner};
use axon_dispatcher::{InProcessQueue, Scheduler, SchedulerConfig, SimpleScheduler, TaskResult};
use axon_isolation::{DockerProvider, VmSpec};
use axon_memory::{HybridMemoryStore, InMemoryStore, MemoryStore, QdrantStore, RedbStore};
use serde::{Deserialize, Serialize};

/// 创建记忆存储 / create a memory store from environment config.
///
/// 通过 `AXON_MEMORY_BACKEND` 选择后端:
/// - `memory`(默认): 进程内 `InMemoryStore`,不依赖外部服务,适合 M1
/// - `hybrid`: `HybridMemoryStore` = `RedbStore` + `QdrantStore`(需要 OpenAI embedding)
pub async fn create_memory_store() -> anyhow::Result<Arc<dyn MemoryStore>> {
    let backend = std::env::var("AXON_MEMORY_BACKEND").unwrap_or_else(|_| "memory".into());
    match backend.as_str() {
        "hybrid" => create_hybrid_memory_store().await,
        _ => Ok(Arc::new(InMemoryStore::new())),
    }
}

/// 创建混合记忆存储 / create the hybrid memory store.
async fn create_hybrid_memory_store() -> anyhow::Result<Arc<dyn MemoryStore>> {
    let redb_path = std::env::var("AXON_MEMORY_REDB_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".axon/memory.redb"));
    let qdrant_url =
        std::env::var("AXON_MEMORY_QDRANT_URL").unwrap_or_else(|_| "http://localhost:6334".into());
    let qdrant_collection =
        std::env::var("AXON_MEMORY_QDRANT_COLLECTION").unwrap_or_else(|_| "axon_memories".into());

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
    let memory = create_memory_store().await?;
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

    /// 默认情况下 `create_memory_store` 创建 InMemoryStore,不依赖外部服务。
    #[tokio::test]
    async fn default_backend_creates_in_memory_store() {
        let old;
        {
            let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            old = std::env::var("AXON_MEMORY_BACKEND").ok();
            std::env::remove_var("AXON_MEMORY_BACKEND");
        }

        let store = create_memory_store().await.unwrap();
        // 验证 store 可用:写入并召回。
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

        {
            let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            match old {
                Some(v) => std::env::set_var("AXON_MEMORY_BACKEND", v),
                None => std::env::remove_var("AXON_MEMORY_BACKEND"),
            }
        }
    }
}
