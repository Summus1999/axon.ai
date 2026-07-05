//! 队列 trait 与核心类型 / queue trait and core types.

use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use futures::Stream;

use axon_core::Result as AxonResult;
use axon_proto::Task;

use crate::RemoteTaskResult;

/// `axon-queue` 专用 Result 别名 / result alias for this crate.
pub type Result<T> = AxonResult<T>;

/// 队列错误 / queue errors.
#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    /// 底层 IO 或网络错误 / underlying IO/network error.
    #[error("queue io error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON 序列化/反序列化错误 / serialization error.
    #[error("queue serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    /// NATS 特定错误 / NATS error.
    #[error("nats error: {0}")]
    Nats(String),
    /// 超时 / timeout.
    #[error("queue operation timed out")]
    Timeout,
    /// 其他错误 / other error.
    #[error("queue error: {0}")]
    Other(String),
}

impl From<QueueError> for axon_core::Error {
    fn from(e: QueueError) -> Self {
        axon_core::Error::Other(e.to_string())
    }
}

impl From<async_nats::Error> for QueueError {
    fn from(e: async_nats::Error) -> Self {
        QueueError::Nats(e.to_string())
    }
}

/// Worker 状态 / worker state reported in heartbeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WorkerState {
    /// 空闲 / idle.
    Idle,
    /// 运行任务 / running a task.
    Running,
    /// 离线 / offline.
    Offline,
}

/// 心跳事件 / heartbeat event from a worker node.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Heartbeat {
    /// worker 节点 id / worker node id.
    pub worker_id: String,
    /// 当前任务 id(可为空)/ current task id if any.
    pub task_id: Option<String>,
    /// worker 状态 / worker state.
    pub state: WorkerState,
    /// 时间戳(Unix 毫秒)/ timestamp.
    pub timestamp_ms: u64,
    /// worker 负载信息(并发数)/ current load.
    pub load: u32,
}

/// Worker 生命周期事件 / worker lifecycle event.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum WorkerEvent {
    /// worker 上线 / worker came online.
    Registered {
        worker_id: String,
        registered_at_ms: u64,
    },
    /// worker 离线 / worker went offline.
    Deregistered {
        worker_id: String,
        deregistered_at_ms: u64,
    },
}

/// 结果过滤条件 / filter for result subscription.
#[derive(Debug, Clone, Default)]
pub enum ResultFilter {
    /// 订阅所有结果 / subscribe to all results.
    #[default]
    All,
    /// 订阅特定任务的结果 / subscribe to results of a specific task.
    Task(String),
    /// 订阅特定 worker 的结果 / subscribe to results from a specific worker.
    Worker(String),
}

/// 结果流 / stream of remote task results.
pub type ResultStream = Pin<Box<dyn Stream<Item = Result<RemoteTaskResult>> + Send>>;

/// 心跳流 / stream of heartbeats.
pub type HeartbeatStream = Pin<Box<dyn Stream<Item = Result<Heartbeat>> + Send>>;

/// 任务队列抽象 / task queue abstraction.
#[async_trait]
pub trait TaskQueue: Send + Sync {
    /// 提交任务到队列 / submit a task to the queue.
    async fn submit(&self, task: Task) -> Result<()>;

    /// 拉取下一个任务(阻塞直到超时)/ pull the next task, waiting up to `timeout`.
    async fn pull(&self, timeout: Duration) -> Result<Option<Task>>;

    /// 发布任务结果 / publish the result of a completed task.
    async fn complete(&self, result: RemoteTaskResult) -> Result<()>;

    /// 订阅结果流 / subscribe to a stream of results.
    async fn subscribe_results(&self, filter: ResultFilter) -> Result<ResultStream>;

    /// 发布心跳 / publish a heartbeat.
    async fn heartbeat(&self, beat: Heartbeat) -> Result<()>;

    /// 订阅心跳流 / subscribe to a stream of heartbeats.
    async fn subscribe_heartbeats(&self) -> Result<HeartbeatStream>;

    /// 发布 worker 生命周期事件 / publish a worker lifecycle event.
    async fn worker_event(&self, event: WorkerEvent) -> Result<()>;

    /// 订阅 worker 生命周期事件 / subscribe to worker lifecycle events.
    async fn subscribe_worker_events(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<WorkerEvent>> + Send>>>;
}
