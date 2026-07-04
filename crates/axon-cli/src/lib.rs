//! axon-cli 库 / the axon-cli library.
//!
//! 把 CLI 的端到端流程暴露为可测试的库函数，便于集成测试调用。
//! 生产入口仍由 `src/main.rs` 的 `axon` 二进制提供。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axon_brain::{CommandAgent, Goal, Planner, SimplePlanner};
use axon_dispatcher::{InProcessQueue, Scheduler, SchedulerConfig, SimpleScheduler, TaskResult};
use axon_isolation::{DockerProvider, VmSpec};
use axon_memory::{HybridMemoryStore, InMemoryStore, MemoryStore, QdrantStore, RedbStore};

/// 创建记忆存储 / create a memory store from environment config.
///
/// 通过 `AXON_MEMORY_BACKEND` 选择后端:
/// - `memory`: 进程内 `InMemoryStore`,不依赖外部服务,适合测试
/// - `hybrid`(默认): `HybridMemoryStore` = `RedbStore` + `QdrantStore`
pub async fn create_memory_store() -> anyhow::Result<Arc<dyn MemoryStore>> {
    let backend = std::env::var("AXON_MEMORY_BACKEND").unwrap_or_else(|_| "hybrid".into());
    match backend.as_str() {
        "memory" => Ok(Arc::new(InMemoryStore::new())),
        _ => create_hybrid_memory_store().await,
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

    Ok(scheduler.results().await)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// 串行化会修改环境变量的测试。
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// 当 `AXON_MEMORY_BACKEND=memory` 时,`create_memory_store` 不依赖外部服务即可创建。
    #[tokio::test]
    async fn memory_backend_creates_in_memory_store() {
        let old;
        {
            let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            old = std::env::var("AXON_MEMORY_BACKEND").ok();
            std::env::set_var("AXON_MEMORY_BACKEND", "memory");
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
