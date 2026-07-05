//! 内存任务队列实现 / in-memory task queue implementation.
//!
//! 用于单机模式,与 M1/M3 的进程内调度保持一致行为。
//! 任务使用 mpsc(单消费者竞争),结果/心跳/事件使用 broadcast(多订阅者)。

use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio::time::timeout;
use tokio_stream::wrappers::BroadcastStream;

use axon_proto::Task;

use crate::{
    Heartbeat, HeartbeatStream, QueueError, RemoteTaskResult, Result, ResultFilter, ResultStream,
    TaskQueue, WorkerEvent,
};

/// 内存任务队列 / in-memory task queue.
#[derive(Debug)]
pub struct InProcessQueue {
    tasks: Mutex<mpsc::Receiver<Task>>,
    task_sender: mpsc::Sender<Task>,
    result_sender: broadcast::Sender<RemoteTaskResult>,
    heartbeat_sender: broadcast::Sender<Heartbeat>,
    worker_event_sender: broadcast::Sender<WorkerEvent>,
}

impl Default for InProcessQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl InProcessQueue {
    /// 创建一个新的内存队列 / create a new in-memory queue.
    pub fn new() -> Self {
        let (task_sender, tasks) = mpsc::channel(1024);
        let (result_sender, _result_rx) = broadcast::channel(1024);
        let (heartbeat_sender, _heartbeat_rx) = broadcast::channel(1024);
        let (worker_event_sender, _worker_event_rx) = broadcast::channel(1024);
        Self {
            tasks: Mutex::new(tasks),
            task_sender,
            result_sender,
            heartbeat_sender,
            worker_event_sender,
        }
    }
}

#[async_trait]
impl TaskQueue for InProcessQueue {
    #[tracing::instrument(skip(self, task), fields(%task.id))]
    async fn submit(&self, task: Task) -> Result<()> {
        self.task_sender
            .send(task)
            .await
            .map_err(|_| QueueError::Other("task receiver dropped".into()))?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn pull(&self, to: Duration) -> Result<Option<Task>> {
        let mut rx = self.tasks.lock().await;
        match timeout(to, rx.recv()).await {
            Ok(Some(task)) => Ok(Some(task)),
            Ok(None) => Ok(None),
            Err(_) => Ok(None),
        }
    }

    #[tracing::instrument(skip(self, result), fields(%result.task_id))]
    async fn complete(&self, result: RemoteTaskResult) -> Result<()> {
        let _ = self.result_sender.send(result);
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn subscribe_results(&self, _filter: ResultFilter) -> Result<ResultStream> {
        let rx = self.result_sender.subscribe();
        Ok(Box::pin(BroadcastStream::new(rx).map(|res| match res {
            Ok(v) => Ok(v),
            Err(_) => Err(QueueError::Other("broadcast lagged".into()).into()),
        })))
    }

    #[tracing::instrument(skip(self, beat), fields(%beat.worker_id))]
    async fn heartbeat(&self, beat: Heartbeat) -> Result<()> {
        let _ = self.heartbeat_sender.send(beat);
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn subscribe_heartbeats(&self) -> Result<HeartbeatStream> {
        let rx = self.heartbeat_sender.subscribe();
        Ok(Box::pin(BroadcastStream::new(rx).map(|res| match res {
            Ok(v) => Ok(v),
            Err(_) => Err(QueueError::Other("broadcast lagged".into()).into()),
        })))
    }

    #[tracing::instrument(skip(self, event))]
    async fn worker_event(&self, event: WorkerEvent) -> Result<()> {
        let _ = self.worker_event_sender.send(event);
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn subscribe_worker_events(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<WorkerEvent>> + Send>>> {
        let rx = self.worker_event_sender.subscribe();
        Ok(Box::pin(BroadcastStream::new(rx).map(|res| match res {
            Ok(v) => Ok(v),
            Err(_) => Err(QueueError::Other("broadcast lagged".into()).into()),
        })))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axon_proto::{Priority, Task, TaskState};
    use futures::StreamExt;

    use super::*;
    use crate::WorkerState;

    fn sample_task(id: &str) -> Task {
        Task {
            id: id.into(),
            parent: None,
            title: format!("task {id}"),
            description: "a test task".into(),
            priority: Priority::Normal,
            state: TaskState::Queued,
            dependencies: vec![],
            created_at: 1,
            updated_at: 1,
            acceptance: vec![],
        }
    }

    /// 验证任务可以提交并被拉取。
    #[tokio::test]
    async fn submit_and_pull_task() {
        let queue = InProcessQueue::new();
        queue.submit(sample_task("t1")).await.unwrap();

        let task = queue.pull(Duration::from_millis(100)).await.unwrap();
        assert!(task.is_some());
        assert_eq!(task.unwrap().id, "t1");
    }

    /// 验证空队列时 pull 返回 None(不阻塞过久)。
    #[tokio::test]
    async fn pull_empty_returns_none() {
        let queue = InProcessQueue::new();
        let task = queue.pull(Duration::from_millis(50)).await.unwrap();
        assert!(task.is_none());
    }

    /// 验证结果发布与订阅。
    #[tokio::test]
    async fn complete_and_subscribe_result() {
        let queue = InProcessQueue::new();
        let mut stream = queue.subscribe_results(ResultFilter::All).await.unwrap();

        queue
            .complete(RemoteTaskResult {
                task_id: "t1".into(),
                worker_id: "w-1".into(),
                success: true,
                summary: "done".into(),
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
                completed_at_ms: 100,
            })
            .await
            .unwrap();

        let result = tokio::time::timeout(Duration::from_millis(100), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(result.task_id, "t1");
        assert!(result.success);
    }

    /// 验证心跳发布与订阅。
    #[tokio::test]
    async fn heartbeat_and_subscribe() {
        let queue = InProcessQueue::new();
        let mut stream = queue.subscribe_heartbeats().await.unwrap();

        queue
            .heartbeat(Heartbeat {
                worker_id: "w-1".into(),
                task_id: Some("t1".into()),
                state: WorkerState::Running,
                timestamp_ms: 100,
                load: 1,
            })
            .await
            .unwrap();

        let beat = tokio::time::timeout(Duration::from_millis(100), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(beat.worker_id, "w-1");
        assert_eq!(beat.state, WorkerState::Running);
    }

    /// 验证 worker 生命周期事件发布与订阅。
    #[tokio::test]
    async fn worker_event_roundtrip() {
        let queue = InProcessQueue::new();
        let mut stream = queue.subscribe_worker_events().await.unwrap();

        queue
            .worker_event(WorkerEvent::Registered {
                worker_id: "w-1".into(),
                registered_at_ms: 100,
            })
            .await
            .unwrap();

        let event = tokio::time::timeout(Duration::from_millis(100), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        match event {
            WorkerEvent::Registered { worker_id, .. } => assert_eq!(worker_id, "w-1"),
            _ => panic!("unexpected event"),
        }
    }
}
