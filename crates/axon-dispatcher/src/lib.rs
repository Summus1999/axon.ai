//! axon-dispatcher — 任务分发中心 / the task dispatcher.
//!
//! 接收 [`Plan`] 中的任务,入队、按 DAG 依赖与优先级调度,并为每个任务
//! 启动一个隔离执行环境(microVM/container),直到满足验收标准。
//!
//! 一期为进程内队列 + 调度器;二期(M4)接入 NATS 支持跨节点。

pub mod in_process_queue;
pub mod simple_scheduler;

use async_trait::async_trait;

use axon_core::{Result, TaskId};
use axon_isolation::VmHandle;
use axon_proto::{Task, TaskState};

pub use in_process_queue::InProcessQueue;
pub use simple_scheduler::SimpleScheduler;

/// 任务队列抽象 / the task queue trait.
#[async_trait]
pub trait TaskQueue: Send + Sync {
    /// 入队 / enqueue a task.
    async fn enqueue(&self, task: Task) -> Result<()>;

    /// 阻塞取下一个可执行任务(依赖已满足)/ dequeue next runnable task.
    async fn dequeue(&self) -> Result<Task>;

    /// 更新任务状态 / update task state.
    async fn update_state(&self, id: &TaskId, state: TaskState) -> Result<()>;

    /// 订阅任务事件(用于 Web 推送)/ subscribe to task events.
    async fn subscribe(&self) -> Result<axon_core::Id>;
}

/// 调度器 / the scheduler trait.
///
/// 组合 [`TaskQueue`] 与 [`IsolationProvider`],把任务派发到隔离环境。
#[async_trait]
pub trait Scheduler: Send + Sync {
    /// 提交一个任务 DAG / submit a plan (set of tasks + deps).
    async fn submit(&self, tasks: Vec<Task>, deps: Vec<(TaskId, TaskId)>) -> Result<()>;

    /// 运行调度循环(阻塞)/ run the scheduling loop.
    async fn run(&self) -> Result<()>;

    /// 优雅停机 / graceful shutdown.
    async fn shutdown(&self) -> Result<()>;
}

/// 调度配置 / scheduler configuration.
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// 最大并发 VM 数 / max concurrent VMs.
    pub max_concurrency: usize,
    /// 单任务超时(秒)/ per-task timeout in seconds.
    pub task_timeout_secs: u32,
    /// 失败重试次数 / max retries on failure.
    pub max_retries: u8,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_concurrency: 4,
            task_timeout_secs: 600,
            max_retries: 2,
        }
    }
}

/// 运行中任务 / a task currently being executed in a VM.
#[derive(Debug, Clone)]
pub struct RunningTask {
    pub task: Task,
    pub vm: VmHandle,
    pub attempt: u8,
}
