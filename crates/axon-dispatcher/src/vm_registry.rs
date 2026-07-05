//! VM 运行时注册表 / VM runtime registry.
//!
//! 跟踪当前运行中的隔离环境(VM/container),供 `axon vms` 查询与心跳更新。
//! M3 单机模式下使用内存 + 文件快照;M4 分布式阶段可替换为共享存储。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::sync::Mutex;

use axon_core::{Result, Timestamp};
use axon_isolation::Backend;

/// 一个运行中 VM 的状态 / state of a running isolated environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmState {
    /// VM id / vm id.
    pub vm_id: String,
    /// 隔离后端 / isolation backend.
    pub backend: Backend,
    /// 当前执行的任务 id / current task id.
    pub task_id: String,
    /// VM 创建时间(Unix 毫秒)/ creation timestamp.
    pub created_at: Timestamp,
    /// 最新心跳时间(Unix 毫秒)/ latest heartbeat timestamp.
    pub last_heartbeat_ms: Option<Timestamp>,
}

/// VM 注册表抽象 / registry for running VMs.
#[async_trait]
pub trait VmRegistry: Send + Sync {
    /// 注册一个运行中 VM / register a running VM.
    async fn register(&self, state: VmState) -> Result<()>;

    /// 注销一个 VM(destroy 后调用)/ unregister a VM.
    async fn unregister(&self, vm_id: &str) -> Result<()>;

    /// 列出所有运行中 VM / list all running VMs.
    async fn list(&self) -> Result<Vec<VmState>>;

    /// 更新 VM 心跳 / update heartbeat for a VM.
    async fn heartbeat(&self, vm_id: &str, timestamp_ms: Timestamp) -> Result<()>;
}

/// 内存 VM 注册表,可选持久化到 JSON 文件 / in-memory VM registry with optional file snapshot.
#[derive(Debug)]
pub struct InMemoryVmRegistry {
    states: Mutex<Vec<VmState>>,
    snapshot_path: Option<PathBuf>,
}

impl InMemoryVmRegistry {
    /// 创建纯内存注册表 / create an in-memory registry.
    pub fn new() -> Self {
        Self {
            states: Mutex::new(vec![]),
            snapshot_path: None,
        }
    }

    /// 创建带文件快照的注册表 / create a registry that persists to a JSON file.
    pub fn with_snapshot(path: impl Into<PathBuf>) -> Self {
        Self {
            states: Mutex::new(vec![]),
            snapshot_path: Some(path.into()),
        }
    }

    /// 从文件加载注册表 / load registry from a snapshot file.
    pub async fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let states = if path.is_file() {
            let content = fs::read_to_string(path)
                .await
                .map_err(axon_core::Error::Io)?;
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            vec![]
        };
        Ok(Self {
            states: Mutex::new(states),
            snapshot_path: Some(path.to_path_buf()),
        })
    }

    /// 持久化当前状态到文件 / persist current states to the snapshot file.
    async fn persist(&self, states: &[VmState]) -> Result<()> {
        if let Some(path) = &self.snapshot_path {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .await
                    .map_err(axon_core::Error::Io)?;
            }
            let json = serde_json::to_string_pretty(states).map_err(|e| {
                axon_core::Error::Other(format!("failed to serialize vm states: {e}"))
            })?;
            fs::write(path, json).await.map_err(axon_core::Error::Io)?;
        }
        Ok(())
    }
}

impl Default for InMemoryVmRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VmRegistry for InMemoryVmRegistry {
    async fn register(&self, state: VmState) -> Result<()> {
        let mut states = self.states.lock().await;
        states.push(state);
        self.persist(&states).await?;
        Ok(())
    }

    async fn unregister(&self, vm_id: &str) -> Result<()> {
        let mut states = self.states.lock().await;
        states.retain(|s| s.vm_id != vm_id);
        self.persist(&states).await?;
        Ok(())
    }

    async fn list(&self) -> Result<Vec<VmState>> {
        Ok(self.states.lock().await.clone())
    }

    async fn heartbeat(&self, vm_id: &str, timestamp_ms: Timestamp) -> Result<()> {
        let mut states = self.states.lock().await;
        if let Some(state) = states.iter_mut().find(|s| s.vm_id == vm_id) {
            state.last_heartbeat_ms = Some(timestamp_ms);
        }
        self.persist(&states).await?;
        Ok(())
    }
}

/// 基于 Arc 的 VM registry(便于在多个组件间共享)/ an Arc-wrapped VM registry.
pub type SharedVmRegistry = Arc<dyn VmRegistry>;

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_state(vm_id: &str) -> VmState {
        VmState {
            vm_id: vm_id.into(),
            backend: Backend::Docker,
            task_id: "t1".into(),
            created_at: 1000,
            last_heartbeat_ms: None,
        }
    }

    /// 验证注册、列出、注销流程。
    #[tokio::test]
    async fn register_list_unregister() {
        let registry = InMemoryVmRegistry::new();
        registry.register(sample_state("vm-1")).await.unwrap();
        registry.register(sample_state("vm-2")).await.unwrap();

        let list = registry.list().await.unwrap();
        assert_eq!(list.len(), 2);

        registry.unregister("vm-1").await.unwrap();
        let list = registry.list().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].vm_id, "vm-2");
    }

    /// 验证心跳更新。
    #[tokio::test]
    async fn heartbeat_updates_timestamp() {
        let registry = InMemoryVmRegistry::new();
        registry.register(sample_state("vm-1")).await.unwrap();
        registry.heartbeat("vm-1", 2000).await.unwrap();

        let list = registry.list().await.unwrap();
        assert_eq!(list[0].last_heartbeat_ms, Some(2000));
    }

    /// 验证文件快照可加载。
    #[tokio::test]
    async fn snapshot_persists_and_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("vms.json");
        let registry = InMemoryVmRegistry::with_snapshot(&path);
        registry.register(sample_state("vm-1")).await.unwrap();
        drop(registry);

        let loaded = InMemoryVmRegistry::load(&path).await.unwrap();
        let list = loaded.list().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].vm_id, "vm-1");
    }
}
