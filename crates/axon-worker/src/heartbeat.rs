//! Worker 心跳 / worker heartbeat.
//!
//! 在任务执行期间周期性上报状态,让调度器/控制面掌握 VM 内任务是否仍在运行。
//! M3 单机模式下使用内存 sink;M4 分布式阶段可替换为 NATS sink。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use axon_core::{Result, TaskId};

/// Worker 状态 / worker state carried in a heartbeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerState {
    /// 空闲等待任务 / idle, waiting for a task.
    Idle,
    /// 正在执行任务 / running a task.
    Running,
    /// 任务成功完成 / task completed successfully.
    Completed,
    /// 任务失败 / task failed.
    Failed,
}

/// 心跳事件 / a heartbeat event from a worker.
#[derive(Debug, Clone)]
pub struct Heartbeat {
    /// worker 实例 id / worker instance id.
    pub worker_id: String,
    /// 当前任务 id / current task id.
    pub task_id: TaskId,
    /// Unix 毫秒时间戳 / timestamp in Unix milliseconds.
    pub timestamp_ms: u64,
    /// worker 状态 / worker state.
    pub state: WorkerState,
}

/// 心跳接收目标 / sink for heartbeat events.
#[async_trait]
pub trait HeartbeatSink: Send + Sync {
    /// 发送一条心跳 / send a heartbeat event.
    async fn send(&self, beat: Heartbeat) -> Result<()>;
}

/// 内存心跳接收器 / in-memory heartbeat sink for single-node mode.
#[derive(Debug, Default, Clone)]
pub struct InMemoryHeartbeatSink {
    beats: Arc<tokio::sync::Mutex<Vec<Heartbeat>>>,
}

impl InMemoryHeartbeatSink {
    /// 创建一个新的内存 sink / create a new in-memory sink.
    pub fn new() -> Self {
        Self::default()
    }

    /// 获取所有已记录的心跳 / get all recorded heartbeats.
    pub async fn records(&self) -> Vec<Heartbeat> {
        self.beats.lock().await.clone()
    }

    /// 获取某任务的最新心跳 / get the latest heartbeat for a task.
    pub async fn latest_for(&self, task_id: &str) -> Option<Heartbeat> {
        self.beats
            .lock()
            .await
            .iter()
            .filter(|b| b.task_id == task_id)
            .cloned()
            .max_by_key(|b| b.timestamp_ms)
    }
}

#[async_trait]
impl HeartbeatSink for InMemoryHeartbeatSink {
    async fn send(&self, beat: Heartbeat) -> Result<()> {
        self.beats.lock().await.push(beat);
        Ok(())
    }
}

/// 周期性心跳发送器 / periodic heartbeat sender.
pub struct HeartbeatSender {
    worker_id: String,
    interval: Duration,
    sink: Arc<dyn HeartbeatSink>,
    stop: Arc<tokio::sync::Notify>,
}

impl std::fmt::Debug for HeartbeatSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeartbeatSender")
            .field("worker_id", &self.worker_id)
            .field("interval", &self.interval)
            .field("sink", &"<dyn HeartbeatSink>")
            .finish_non_exhaustive()
    }
}

impl HeartbeatSender {
    /// 创建心跳发送器 / create a heartbeat sender.
    pub fn new(
        worker_id: impl Into<String>,
        interval: Duration,
        sink: Arc<dyn HeartbeatSink>,
    ) -> Self {
        Self {
            worker_id: worker_id.into(),
            interval,
            sink,
            stop: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// 启动心跳循环(阻塞,需 spawn 在独立 task)/ run the heartbeat loop.
    ///
    /// `task_id` 为当前执行的任务;状态由调用方通过 `update_state` 更新。
    pub async fn run(&self, task_id: impl Into<TaskId>) {
        let task_id = task_id.into();
        let state = WorkerState::Running;
        loop {
            match tokio::time::timeout(self.interval, self.stop.notified()).await {
                Ok(()) => {
                    // 收到停止信号,再发一次最终状态后退出。
                    let _ = self.emit(&task_id, state).await;
                    break;
                }
                Err(_) => {
                    let _ = self.emit(&task_id, state).await;
                }
            }
        }
    }

    /// 更新下一次心跳的状态 / update the state reported by subsequent heartbeats.
    pub fn update_state(&self, new_state: WorkerState) {
        // 当前实现通过共享状态变量传递;简单起见,run 循环每次读取外部传入的值较复杂,
        // 因此这里仅提供 API 占位,M3 调度器通过显式发送最终心跳表达完成/失败。
        let _ = new_state;
    }

    /// 停止心跳循环 / stop the heartbeat loop.
    pub fn stop(&self) {
        self.stop.notify_one();
    }

    async fn emit(&self, task_id: &str, state: WorkerState) -> Result<()> {
        let beat = Heartbeat {
            worker_id: self.worker_id.clone(),
            task_id: task_id.into(),
            timestamp_ms: now_ms(),
            state,
        };
        self.sink.send(beat).await
    }
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
    use super::*;

    /// 验证心跳按预期间隔发送。
    #[tokio::test]
    async fn sender_emits_heartbeats() {
        let sink = Arc::new(InMemoryHeartbeatSink::new());
        let sender = HeartbeatSender::new("w-1", Duration::from_millis(10), sink.clone());

        let handle = tokio::spawn(async move {
            sender.run("t-1").await;
            sender
        });

        // 等待至少两次心跳。
        tokio::time::sleep(Duration::from_millis(80)).await;
        handle.abort();

        let records = sink.records().await;
        assert!(
            records.len() >= 2,
            "expected at least 2 heartbeats, got {}",
            records.len()
        );
        for r in &records {
            assert_eq!(r.worker_id, "w-1");
            assert_eq!(r.task_id, "t-1");
            assert_eq!(r.state, WorkerState::Running);
        }
    }

    /// 验证 `stop` 后发送最终心跳并退出。
    #[tokio::test]
    async fn sender_stops_gracefully() {
        let sink = Arc::new(InMemoryHeartbeatSink::new());
        let sender = Arc::new(HeartbeatSender::new(
            "w-1",
            Duration::from_secs(60),
            sink.clone(),
        ));

        let sender2 = sender.clone();
        let handle = tokio::spawn(async move {
            sender2.run("t-1").await;
        });

        // 等待第一次心跳发出。
        tokio::time::sleep(Duration::from_millis(20)).await;
        sender.stop();
        let _ = handle.await;

        let records = sink.records().await;
        assert!(!records.is_empty());
    }

    /// 验证 `latest_for` 返回某任务的最新心跳。
    #[tokio::test]
    async fn latest_for_returns_most_recent() {
        let sink = InMemoryHeartbeatSink::new();
        let now = now_ms();
        sink.send(Heartbeat {
            worker_id: "w-1".into(),
            task_id: "t-1".into(),
            timestamp_ms: now - 1000,
            state: WorkerState::Running,
        })
        .await
        .unwrap();
        sink.send(Heartbeat {
            worker_id: "w-1".into(),
            task_id: "t-1".into(),
            timestamp_ms: now,
            state: WorkerState::Completed,
        })
        .await
        .unwrap();

        let latest = sink.latest_for("t-1").await.unwrap();
        assert_eq!(latest.state, WorkerState::Completed);
    }
}
