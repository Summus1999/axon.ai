//! axon-proto — 内部消息与 RPC schema / internal message & RPC schema.
//!
//! 一期用 serde JSON 定义消息结构;二期(M4)用 prost/tonic 生成 gRPC。
//! 此 crate 作为 brain / dispatcher / worker 之间的契约层。

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use axon_core::{Id, TaskId, Timestamp};

/// 任务状态 / task lifecycle state.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    Scheduled,
    Running,
    AwaitingReview,
    Completed,
    Failed,
    Cancelled,
}

/// 任务优先级 / task priority.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Low,
    Normal,
    High,
    Urgent,
}

/// 一个开发子任务 / a development sub-task dispatched to a worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub parent: Option<TaskId>,
    pub title: String,
    pub description: String,
    pub priority: Priority,
    pub state: TaskState,
    pub dependencies: Vec<TaskId>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// 验收标准(供 worker 自检)/ acceptance criteria.
    pub acceptance: Vec<String>,
}

/// 任务事件 / a task lifecycle event (用于 WebSocket 推送与日志).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskEvent {
    Created {
        task: Task,
    },
    StateChanged {
        task_id: TaskId,
        from: TaskState,
        to: TaskState,
    },
    Progress {
        task_id: TaskId,
        message: String,
    },
    Completed {
        task_id: TaskId,
    },
    Failed {
        task_id: TaskId,
        error: String,
    },
}

/// Worker 上报心跳 / worker heartbeat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heartbeat {
    pub worker_id: Id,
    pub vm_id: Id,
    pub current_task: Option<TaskId>,
    pub timestamp: Timestamp,
}

/// 占位:未来 gRPC service 定义入口 / future tonic service entry point.
/// TODO(M4): 在 proto/ 下定义 .proto 并用 build.rs 经 tonic-build 生成。
pub mod grpc {
    // 预留 gRPC service 模块 / reserved for generated tonic code.
}
