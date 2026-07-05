//! 远程 Dispatcher 节点 / remote dispatcher node.
//!
//! M4 分布式架构中,dispatcher 不再直接执行命令,而是：
//! - 接收 CLI 远程提交的任务并入队(TaskQueue)
//! - 订阅结果流,聚合任务状态
//! - 订阅心跳与 worker 事件,维护 worker 列表
//! - 提供 HTTP API 与 WebSocket,供 CLI 和 Dashboard 查询实时状态

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{State, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use futures::StreamExt;
use tokio::sync::RwLock;

use axon_core::Result as AxonResult;
use axon_proto::{Task, TaskState};
use axon_queue::{Heartbeat, RemoteTaskResult, ResultFilter, TaskQueue, WorkerEvent};

/// Worker 快照 / worker snapshot.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkerRecord {
    /// worker 节点 id / worker node id.
    pub worker_id: String,
    /// 当前状态 / current state.
    pub state: axon_queue::WorkerState,
    /// 当前任务 id / current task id if any.
    pub task_id: Option<String>,
    /// 最后心跳时间(Unix 毫秒)/ last heartbeat timestamp.
    pub last_heartbeat_ms: u64,
    /// 当前负载 / current load.
    pub load: u32,
    /// 注册时间(Unix 毫秒)/ registration timestamp.
    pub registered_at_ms: u64,
}

/// 任务快照 / task snapshot.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskRecord {
    /// 任务 id / task id.
    pub task_id: String,
    /// 当前状态 / current state.
    pub state: TaskState,
    /// 标题 / title.
    pub title: String,
    /// 描述 / description.
    pub description: String,
    /// 分配到的 worker id / assigned worker id if any.
    pub worker_id: Option<String>,
    /// 创建时间(Unix 毫秒)/ creation timestamp.
    pub created_at_ms: u64,
    /// 最后更新时间(Unix 毫秒)/ last update timestamp.
    pub updated_at_ms: u64,
}

/// 远程 Dispatcher 共享状态 / shared state for the remote dispatcher.
pub struct RemoteDispatcherState {
    /// 任务队列 / task queue.
    pub queue: Arc<dyn TaskQueue>,
    /// 任务状态表 / task state table.
    pub tasks: RwLock<HashMap<String, TaskRecord>>,
    /// worker 状态表 / worker state table.
    pub workers: RwLock<HashMap<String, WorkerRecord>>,
    /// WebSocket 广播发送器 / websocket broadcast sender.
    pub broadcast: tokio::sync::broadcast::Sender<DispatchEvent>,
}

impl std::fmt::Debug for RemoteDispatcherState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteDispatcherState")
            .field("queue", &"<dyn TaskQueue>")
            .field("broadcast", &"<broadcast sender>")
            .finish_non_exhaustive()
    }
}

/// 分发给客户端的事件 / event broadcasted to connected clients.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum DispatchEvent {
    /// 任务状态更新 / task state updated.
    TaskUpdated(TaskRecord),
    /// worker 状态更新 / worker state updated.
    WorkerUpdated(WorkerRecord),
    /// 任务结果到达 / task result arrived.
    TaskResult(RemoteTaskResult),
    /// worker 心跳 / worker heartbeat.
    Heartbeat(Heartbeat),
}

/// 远程 Dispatcher / remote dispatcher node.
#[derive(Debug, Clone)]
pub struct RemoteDispatcher {
    state: Arc<RemoteDispatcherState>,
}

impl RemoteDispatcher {
    /// 创建 dispatcher / create a remote dispatcher.
    pub fn new(queue: Arc<dyn TaskQueue>) -> Self {
        let (sender, _receiver) = tokio::sync::broadcast::channel(1024);
        let state = Arc::new(RemoteDispatcherState {
            queue,
            tasks: RwLock::new(HashMap::new()),
            workers: RwLock::new(HashMap::new()),
            broadcast: sender,
        });
        Self { state }
    }

    /// 访问共享状态 / access the shared dispatcher state.
    pub fn state(&self) -> Arc<RemoteDispatcherState> {
        Arc::clone(&self.state)
    }

    /// 提交任务到队列 / submit a task to the queue.
    #[tracing::instrument(skip(self, task), fields(%task.id))]
    pub async fn submit_task(&self, task: Task) -> AxonResult<()> {
        let mut tasks = self.state.tasks.write().await;
        tasks.insert(
            task.id.clone(),
            TaskRecord {
                task_id: task.id.clone(),
                state: TaskState::Queued,
                title: task.title.clone(),
                description: task.description.clone(),
                worker_id: None,
                created_at_ms: now_ms(),
                updated_at_ms: now_ms(),
            },
        );
        drop(tasks);
        self.state.queue.submit(task).await?;
        Ok(())
    }

    /// 运行 dispatcher：启动结果/心跳/事件聚合，并提供 HTTP API / run the dispatcher.
    #[tracing::instrument(skip(self), fields(%http_addr))]
    pub async fn run(&self, http_addr: SocketAddr) -> AxonResult<()> {
        let results_handle = tokio::spawn(self.clone().collect_results());
        let heartbeats_handle = tokio::spawn(self.clone().collect_heartbeats());
        let worker_events_handle = tokio::spawn(self.clone().collect_worker_events());

        let app = Router::new()
            .route("/tasks", get(list_tasks).post(submit_task))
            .route("/workers", get(list_workers))
            .route("/ws", get(websocket_handler))
            .with_state(self.state.clone());

        let listener = tokio::net::TcpListener::bind(http_addr).await?;
        let server = axum::serve(listener, app);

        tokio::select! {
            _ = server => {}
            _ = results_handle => {}
            _ = heartbeats_handle => {}
            _ = worker_events_handle => {}
        }

        Ok(())
    }

    /// 聚合任务结果 / collect task results from the queue.
    #[tracing::instrument(skip(self), fields(dispatcher = true))]
    async fn collect_results(self) -> AxonResult<()> {
        let mut stream = self
            .state
            .queue
            .subscribe_results(ResultFilter::All)
            .await?;
        while let Some(result) = stream.next().await {
            let result = result?;
            let mut tasks = self.state.tasks.write().await;
            if let Some(record) = tasks.get_mut(&result.task_id) {
                record.state = if result.success {
                    TaskState::Completed
                } else {
                    TaskState::Failed
                };
                record.worker_id = Some(result.worker_id.clone());
                record.updated_at_ms = now_ms();
            }
            drop(tasks);
            let _ = self
                .state
                .broadcast
                .send(DispatchEvent::TaskResult(result.clone()));
            if let Some(record) = self.state.tasks.read().await.get(&result.task_id).cloned() {
                let _ = self
                    .state
                    .broadcast
                    .send(DispatchEvent::TaskUpdated(record));
            }
        }
        Ok(())
    }

    /// 聚合 worker 心跳 / collect worker heartbeats.
    #[tracing::instrument(skip(self), fields(dispatcher = true))]
    async fn collect_heartbeats(self) -> AxonResult<()> {
        let mut stream = self.state.queue.subscribe_heartbeats().await?;
        while let Some(beat) = stream.next().await {
            let beat = beat?;
            let mut workers = self.state.workers.write().await;
            if let Some(record) = workers.get_mut(&beat.worker_id) {
                record.state = beat.state;
                record.task_id = beat.task_id.clone();
                record.last_heartbeat_ms = beat.timestamp_ms;
                record.load = beat.load;
            }
            drop(workers);
            let _ = self
                .state
                .broadcast
                .send(DispatchEvent::Heartbeat(beat.clone()));
            if let Some(record) = self
                .state
                .workers
                .read()
                .await
                .get(&beat.worker_id)
                .cloned()
            {
                let _ = self
                    .state
                    .broadcast
                    .send(DispatchEvent::WorkerUpdated(record));
            }
        }
        Ok(())
    }

    /// 聚合 worker 生命周期事件 / collect worker lifecycle events.
    #[tracing::instrument(skip(self), fields(dispatcher = true))]
    async fn collect_worker_events(self) -> AxonResult<()> {
        let mut stream = self.state.queue.subscribe_worker_events().await?;
        while let Some(event) = stream.next().await {
            let event = event?;
            let mut workers = self.state.workers.write().await;
            match event {
                WorkerEvent::Registered {
                    worker_id,
                    registered_at_ms,
                } => {
                    workers.insert(
                        worker_id.clone(),
                        WorkerRecord {
                            worker_id,
                            state: axon_queue::WorkerState::Idle,
                            task_id: None,
                            last_heartbeat_ms: registered_at_ms,
                            load: 0,
                            registered_at_ms,
                        },
                    );
                }
                WorkerEvent::Deregistered { worker_id, .. } => {
                    workers.remove(&worker_id);
                }
            }
            drop(workers);
        }
        Ok(())
    }
}

/// HTTP: 列出所有任务 / list all tasks.
async fn list_tasks(State(state): State<Arc<RemoteDispatcherState>>) -> Json<Vec<TaskRecord>> {
    let tasks = state.tasks.read().await.values().cloned().collect();
    Json(tasks)
}

/// HTTP: 提交任务 / submit a task via HTTP.
async fn submit_task(
    State(state): State<Arc<RemoteDispatcherState>>,
    Json(task): Json<Task>,
) -> impl IntoResponse {
    let dispatcher = RemoteDispatcher { state };
    match dispatcher.submit_task(task).await {
        Ok(()) => (axum::http::StatusCode::ACCEPTED, "task accepted").into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to submit task: {e}"),
        )
            .into_response(),
    }
}

/// HTTP: 列出所有 worker / list all workers.
async fn list_workers(State(state): State<Arc<RemoteDispatcherState>>) -> Json<Vec<WorkerRecord>> {
    let workers = state.workers.read().await.values().cloned().collect();
    Json(workers)
}

/// WebSocket 处理器 / websocket handler for real-time updates.
async fn websocket_handler(
    State(state): State<Arc<RemoteDispatcherState>>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| websocket_loop(state, socket))
}

async fn websocket_loop(
    state: Arc<RemoteDispatcherState>,
    mut socket: axum::extract::ws::WebSocket,
) {
    let mut rx = state.broadcast.subscribe();
    while let Ok(event) = rx.recv().await {
        let payload = match serde_json::to_string(&event) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if socket
            .send(axum::extract::ws::Message::Text(payload))
            .await
            .is_err()
        {
            break;
        }
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
    use std::time::Duration;

    use axon_proto::{Priority, Task};
    use axon_queue::{InProcessQueue, TaskQueue, WorkerEvent};

    use super::*;

    fn sample_task(id: &str) -> Task {
        Task {
            id: id.into(),
            parent: None,
            title: format!("task {id}"),
            description: "test".into(),
            priority: Priority::Normal,
            state: TaskState::Queued,
            dependencies: vec![],
            created_at: 1,
            updated_at: 1,
            acceptance: vec![],
        }
    }

    /// 验证 dispatcher 提交任务后任务表中有记录。
    #[tokio::test]
    async fn submit_task_updates_state() {
        let queue = Arc::new(InProcessQueue::new());
        let dispatcher = RemoteDispatcher::new(queue);

        dispatcher.submit_task(sample_task("t1")).await.unwrap();

        let tasks = dispatcher.state.tasks.read().await;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks["t1"].state, TaskState::Queued);
    }

    /// 验证 dispatcher 能聚合任务结果并更新状态。
    #[tokio::test]
    async fn collect_result_updates_task_state() {
        let queue = Arc::new(InProcessQueue::new());
        let dispatcher = RemoteDispatcher::new(queue.clone());

        dispatcher.submit_task(sample_task("t1")).await.unwrap();

        // 先启动聚合任务,确保订阅建立后再发布结果。
        let dispatcher2 = dispatcher.clone();
        let handle = tokio::spawn(async move { dispatcher2.collect_results().await });
        tokio::time::sleep(Duration::from_millis(20)).await;

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

        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.abort();

        let tasks = dispatcher.state.tasks.read().await;
        assert_eq!(tasks["t1"].state, TaskState::Completed);
        assert_eq!(tasks["t1"].worker_id.as_deref(), Some("w-1"));
    }

    /// 验证 worker 注册事件会创建 worker 记录。
    #[tokio::test]
    async fn collect_worker_registered_event() {
        let queue = Arc::new(InProcessQueue::new());
        let dispatcher = RemoteDispatcher::new(queue.clone());

        // 先启动聚合任务,确保订阅建立后再发布事件。
        let dispatcher2 = dispatcher.clone();
        let handle = tokio::spawn(async move { dispatcher2.collect_worker_events().await });
        tokio::time::sleep(Duration::from_millis(20)).await;

        queue
            .worker_event(WorkerEvent::Registered {
                worker_id: "w-1".into(),
                registered_at_ms: 100,
            })
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.abort();

        let workers = dispatcher.state.workers.read().await;
        assert_eq!(workers.len(), 1);
        assert_eq!(workers["w-1"].worker_id, "w-1");
    }
}
