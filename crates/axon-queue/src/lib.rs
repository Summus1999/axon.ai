//! axon-queue — 分布式任务队列抽象 / distributed task queue abstraction.
//!
//! 提供与具体消息中间件无关的 [`TaskQueue`] trait,支持单机内存实现
//! ([`InProcessQueue`]) 与 NATS 实现 ([`NatsQueue`])。
//! 任务、结果、心跳统一使用 JSON 序列化,便于调试与后续迁移到 protobuf。

mod in_process;
mod nats;
mod queue;

pub use in_process::InProcessQueue;
pub use nats::NatsQueue;
pub use queue::{
    Heartbeat, HeartbeatStream, QueueError, Result, ResultFilter, ResultStream, TaskQueue,
    WorkerEvent, WorkerState,
};

/// 远程任务结果 / the result of a remotely executed task.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct RemoteTaskResult {
    /// 任务 id / task id.
    pub task_id: String,
    /// worker 节点 id / worker node id.
    pub worker_id: String,
    /// 是否成功 / whether the task succeeded.
    pub success: bool,
    /// 产出摘要 / summary of the output.
    pub summary: String,
    /// 标准输出 / stdout.
    pub stdout: String,
    /// 标准错误 / stderr.
    pub stderr: String,
    /// 退出码 / exit code.
    pub exit_code: i32,
    /// 完成时间戳(Unix 毫秒)/ completion timestamp.
    pub completed_at_ms: u64,
}
