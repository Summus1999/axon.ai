//! axon-worker — 任务执行 Worker / the per-VM agent worker.
//!
//! 运行在每个隔离环境(microVM/container)内,接收 dispatcher 下发的单个
//! [`Task`],借助 LLM + 记忆执行开发任务,自检通过后回报结果。
//!
//! 具体实现(M1)使用 `axon-brain::Agent`。

#![allow(dead_code)]

use axon_core::Result;
use axon_proto::Task;

/// Worker 配置 / worker configuration.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// worker 实例 id / worker instance id.
    pub worker_id: String,
    /// 心跳间隔(秒)/ heartbeat interval in seconds.
    pub heartbeat_interval_secs: u32,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            worker_id: "worker-0".into(),
            heartbeat_interval_secs: 15,
        }
    }
}

/// Worker 入口:执行一个任务并回报 / run a single task to completion.
///
/// TODO(M1): 接入 `axon_brain::Agent`、LLM provider、记忆 store,
/// 实现 ReAct 循环 + 自检 + 回报。
pub async fn run_task(_config: &WorkerConfig, _task: &Task) -> Result<WorkReport> {
    Err(axon_core::Error::Other(
        "worker::run_task not yet implemented (skeleton, M1)".into(),
    ))
}

/// 任务执行回报 / a task completion report from a worker.
#[derive(Debug, Clone)]
pub struct WorkReport {
    pub task_id: axon_core::TaskId,
    pub success: bool,
    pub summary: String,
    pub changed_files: Vec<String>,
    pub log: String,
}
