//! NATS 任务队列实现 / NATS-backed task queue implementation.
//!
//! 使用 NATS core 的 pub/sub + queue group 实现任务分发、结果收集与心跳广播。

use std::pin::Pin;
use std::time::Duration;

use async_nats::{Client, Subject};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use tokio::time::timeout;

use axon_proto::Task;

use crate::{
    Heartbeat, HeartbeatStream, QueueError, RemoteTaskResult, Result, ResultFilter, ResultStream,
    TaskQueue, WorkerEvent,
};

/// NATS 主题常量 / NATS subject constants.
mod subjects {
    /// 任务提交主题 / task submission subject.
    pub const TASKS: &str = "axon.tasks";
    /// 任务结果主题前缀 / task result subject prefix.
    pub const RESULTS_PREFIX: &str = "axon.results.";
    /// 任务结果通配主题 / task result wildcard subject.
    pub const RESULTS_WILDCARD: &str = "axon.results.>";
    /// 心跳主题 / heartbeat subject.
    pub const HEARTBEAT: &str = "axon.heartbeat";
    /// worker 生命周期事件主题 / worker lifecycle events subject.
    pub const WORKER_EVENTS: &str = "axon.worker.events";
    /// worker 消费队列组 / queue group for task consumers.
    pub const WORKERS_QUEUE_GROUP: &str = "workers";
}

/// NATS 任务队列 / NATS task queue.
#[derive(Debug, Clone)]
pub struct NatsQueue {
    client: Client,
}

/// 将任意 NATS 错误转换为 `QueueError` / convert any NATS error to QueueError.
fn nats_err<E: std::fmt::Display>(e: E) -> QueueError {
    QueueError::Nats(e.to_string())
}

impl NatsQueue {
    /// 连接到 NATS server / connect to a NATS server.
    pub async fn connect(server_url: impl Into<String>) -> Result<Self> {
        let client = async_nats::connect(server_url.into())
            .await
            .map_err(nats_err)?;
        Ok(Self { client })
    }

    /// 使用已有 NATS client 创建队列 / create from an existing NATS client.
    pub fn from_client(client: Client) -> Self {
        Self { client }
    }

    /// 序列化消息 / serialize a message to JSON bytes.
    fn serialize<T: serde::Serialize>(msg: &T) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(msg).map_err(QueueError::from)?)
    }

    /// 反序列化消息 / deserialize JSON bytes to a message.
    fn deserialize<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
        Ok(serde_json::from_slice(bytes).map_err(QueueError::from)?)
    }
}

#[async_trait]
impl TaskQueue for NatsQueue {
    #[tracing::instrument(skip(self, task), fields(%task.id))]
    async fn submit(&self, task: Task) -> Result<()> {
        let payload = Self::serialize(&task)?;
        self.client
            .publish(Subject::from(subjects::TASKS), payload.into())
            .await
            .map_err(nats_err)?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn pull(&self, to: Duration) -> Result<Option<Task>> {
        let mut sub = self
            .client
            .queue_subscribe(
                Subject::from(subjects::TASKS),
                subjects::WORKERS_QUEUE_GROUP.into(),
            )
            .await
            .map_err(nats_err)?;
        match timeout(to, sub.next()).await {
            Ok(Some(msg)) => {
                let task = Self::deserialize(&msg.payload)?;
                Ok(Some(task))
            }
            Ok(None) => Ok(None),
            Err(_) => Ok(None),
        }
    }

    #[tracing::instrument(skip(self, result), fields(%result.task_id))]
    async fn complete(&self, result: RemoteTaskResult) -> Result<()> {
        let subject = format!("{}{}", subjects::RESULTS_PREFIX, result.task_id);
        let payload = Self::serialize(&result)?;
        self.client
            .publish(Subject::from(subject), payload.into())
            .await
            .map_err(nats_err)?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn subscribe_results(&self, filter: ResultFilter) -> Result<ResultStream> {
        let subject = match filter {
            ResultFilter::All => subjects::RESULTS_WILDCARD.to_string(),
            ResultFilter::Task(task_id) => format!("{}{}", subjects::RESULTS_PREFIX, task_id),
            ResultFilter::Worker(worker_id) => {
                // NATS 不直接按 worker 过滤结果;这里仍订阅全部,上层过滤。
                // 未来可通过 `axon.results.{worker_id}.>` 约定实现。
                let _ = worker_id;
                subjects::RESULTS_WILDCARD.to_string()
            }
        };
        let sub = self
            .client
            .subscribe(Subject::from(subject))
            .await
            .map_err(nats_err)?;
        let stream = sub.filter_map(|msg| async move {
            match Self::deserialize::<RemoteTaskResult>(&msg.payload) {
                Ok(result) => Some(Ok(result)),
                Err(e) => Some(Err(e)),
            }
        });
        Ok(Box::pin(stream))
    }

    #[tracing::instrument(skip(self, beat), fields(%beat.worker_id))]
    async fn heartbeat(&self, beat: Heartbeat) -> Result<()> {
        let payload = Self::serialize(&beat)?;
        self.client
            .publish(Subject::from(subjects::HEARTBEAT), payload.into())
            .await
            .map_err(nats_err)?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn subscribe_heartbeats(&self) -> Result<HeartbeatStream> {
        let sub = self
            .client
            .subscribe(Subject::from(subjects::HEARTBEAT))
            .await
            .map_err(nats_err)?;
        let stream = sub.filter_map(|msg| async move {
            match Self::deserialize::<Heartbeat>(&msg.payload) {
                Ok(beat) => Some(Ok(beat)),
                Err(e) => Some(Err(e)),
            }
        });
        Ok(Box::pin(stream))
    }

    #[tracing::instrument(skip(self, event))]
    async fn worker_event(&self, event: WorkerEvent) -> Result<()> {
        let payload = Self::serialize(&event)?;
        self.client
            .publish(Subject::from(subjects::WORKER_EVENTS), payload.into())
            .await
            .map_err(nats_err)?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn subscribe_worker_events(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<WorkerEvent>> + Send>>> {
        let sub = self
            .client
            .subscribe(Subject::from(subjects::WORKER_EVENTS))
            .await
            .map_err(nats_err)?;
        let stream = sub.filter_map(|msg| async move {
            match Self::deserialize::<WorkerEvent>(&msg.payload) {
                Ok(event) => Some(Ok(event)),
                Err(e) => Some(Err(e)),
            }
        });
        Ok(Box::pin(stream))
    }
}
