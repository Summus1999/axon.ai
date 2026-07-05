//! axon-dashboard — axon.ai Web 仪表盘后端 / Web dashboard backend.
//!
//! M4 分布式架构中,dashboard 作为独立服务：
//! - 通过 HTTP 从 remote dispatcher 拉取任务/worker 初始状态
//! - 通过 WebSocket 订阅 dispatcher 的实时事件并聚合本地状态
//! - 提供静态首页与 `/ws` WebSocket,向浏览器推送任务与 worker 变化

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{State, WebSocketUpgrade};
use axum::response::Html;
use axum::routing::get;
use axum::{Json, Router};
use futures::StreamExt;
use tokio::sync::{broadcast, RwLock};

use axon_dispatcher::remote_dispatcher::{DispatchEvent, TaskRecord, WorkerRecord};

/// 仪表盘共享状态 / shared dashboard state.
pub struct DashboardState {
    /// 任务快照表 / task snapshot table.
    pub tasks: RwLock<HashMap<String, TaskRecord>>,
    /// worker 快照表 / worker snapshot table.
    pub workers: RwLock<HashMap<String, WorkerRecord>>,
    /// 向浏览器广播事件 / broadcast events to connected browsers.
    pub broadcast: broadcast::Sender<DashboardEvent>,
}

impl DashboardState {
    /// 创建空状态 / create an empty dashboard state.
    pub fn new(broadcast_capacity: usize) -> Self {
        let (sender, _receiver) = broadcast::channel(broadcast_capacity);
        Self {
            tasks: RwLock::new(HashMap::new()),
            workers: RwLock::new(HashMap::new()),
            broadcast: sender,
        }
    }

    /// 应用 dispatcher 事件并广播给浏览器 / apply a dispatcher event and broadcast it.
    pub async fn apply_dispatch_event(&self, event: DispatchEvent) {
        let broadcast_event = DashboardEvent::from(event.clone());
        match event {
            DispatchEvent::TaskUpdated(record) => {
                self.tasks
                    .write()
                    .await
                    .insert(record.task_id.clone(), record);
            }
            DispatchEvent::WorkerUpdated(record) => {
                self.workers
                    .write()
                    .await
                    .insert(record.worker_id.clone(), record);
            }
            DispatchEvent::TaskResult(result) => {
                let mut tasks = self.tasks.write().await;
                if let Some(record) = tasks.get_mut(&result.task_id) {
                    record.state = if result.success {
                        axon_proto::TaskState::Completed
                    } else {
                        axon_proto::TaskState::Failed
                    };
                    record.worker_id = Some(result.worker_id.clone());
                    record.updated_at_ms = result.completed_at_ms;
                }
            }
            DispatchEvent::Heartbeat(beat) => {
                let mut workers = self.workers.write().await;
                if let Some(record) = workers.get_mut(&beat.worker_id) {
                    record.state = beat.state;
                    record.task_id = beat.task_id.clone();
                    record.last_heartbeat_ms = beat.timestamp_ms;
                    record.load = beat.load;
                }
            }
        }
        let _ = self.broadcast.send(broadcast_event);
    }
}

impl std::fmt::Debug for DashboardState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DashboardState")
            .field("broadcast", &"<broadcast sender>")
            .finish_non_exhaustive()
    }
}

/// 浏览器可见事件 / event forwarded to the browser.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DashboardEvent {
    /// 任务状态更新 / task state updated.
    TaskUpdated(TaskRecord),
    /// worker 状态更新 / worker state updated.
    WorkerUpdated(WorkerRecord),
    /// 任务结果到达 / task result arrived.
    TaskResult(axon_queue::RemoteTaskResult),
    /// worker 心跳 / worker heartbeat.
    Heartbeat(axon_queue::Heartbeat),
}

impl From<DispatchEvent> for DashboardEvent {
    fn from(event: DispatchEvent) -> Self {
        match event {
            DispatchEvent::TaskUpdated(r) => DashboardEvent::TaskUpdated(r),
            DispatchEvent::WorkerUpdated(r) => DashboardEvent::WorkerUpdated(r),
            DispatchEvent::TaskResult(r) => DashboardEvent::TaskResult(r),
            DispatchEvent::Heartbeat(b) => DashboardEvent::Heartbeat(b),
        }
    }
}

/// Web 仪表盘 / Web dashboard.
#[derive(Debug, Clone)]
pub struct Dashboard {
    state: Arc<DashboardState>,
    dispatcher_url: String,
    bind_addr: SocketAddr,
}

impl Dashboard {
    /// 创建 dashboard / create a dashboard instance.
    ///
    /// `dispatcher_url` 为 remote dispatcher HTTP 地址;`bind_addr` 为本服务监听地址。
    pub fn new(dispatcher_url: impl Into<String>, bind_addr: SocketAddr) -> Self {
        Self {
            state: Arc::new(DashboardState::new(1024)),
            dispatcher_url: dispatcher_url.into(),
            bind_addr,
        }
    }

    /// 启动 dashboard：同步 dispatcher 状态并提供 HTTP/WebSocket 服务 / run the dashboard.
    pub async fn run(&self) -> anyhow::Result<()> {
        let sync_handle = tokio::spawn(sync_from_dispatcher(
            self.state.clone(),
            self.dispatcher_url.clone(),
        ));

        let app = Router::new()
            .route("/", get(index_handler))
            .route("/api/tasks", get(list_tasks_handler))
            .route("/api/workers", get(list_workers_handler))
            .route("/ws", get(websocket_handler))
            .with_state(self.state.clone());

        let listener = tokio::net::TcpListener::bind(self.bind_addr).await?;
        tracing::info!(addr = %listener.local_addr()?, "dashboard 启动");
        let server = axum::serve(listener, app);

        tokio::select! {
            _ = server => {}
            _ = sync_handle => {}
        }

        Ok(())
    }
}

/// 持续同步 dispatcher 状态 / continuously sync state from the remote dispatcher.
async fn sync_from_dispatcher(state: Arc<DashboardState>, dispatcher_url: String) {
    loop {
        if let Err(e) = sync_once(&state, &dispatcher_url).await {
            tracing::warn!(error = %e, "dispatcher 同步失败,5s 后重试");
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }
}

/// 单次同步：拉取初始状态并订阅 WebSocket / perform one sync attempt.
async fn sync_once(state: &Arc<DashboardState>, dispatcher_url: &str) -> anyhow::Result<()> {
    // 拉取初始任务与 worker 列表。
    let tasks: Vec<TaskRecord> = reqwest::get(format!("{dispatcher_url}/tasks"))
        .await?
        .json()
        .await?;
    let workers: Vec<WorkerRecord> = reqwest::get(format!("{dispatcher_url}/workers"))
        .await?
        .json()
        .await?;

    {
        let mut task_map = state.tasks.write().await;
        task_map.clear();
        for t in tasks {
            task_map.insert(t.task_id.clone(), t);
        }
    }
    {
        let mut worker_map = state.workers.write().await;
        worker_map.clear();
        for w in workers {
            worker_map.insert(w.worker_id.clone(), w);
        }
    }

    // 订阅 dispatcher WebSocket 实时事件。
    let ws_url = format!("{dispatcher_url}/ws").replacen("http", "ws", 1);
    let (mut socket, _response) = tokio_tungstenite::connect_async(&ws_url).await?;

    while let Some(msg) = socket.next().await {
        let msg = msg?;
        if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
            match serde_json::from_str::<DispatchEvent>(&text) {
                Ok(event) => state.apply_dispatch_event(event).await,
                Err(e) => tracing::warn!(error = %e, "无法解析 dispatcher 事件"),
            }
        }
    }

    Err(anyhow::anyhow!("dispatcher WebSocket 已关闭"))
}

/// HTTP: 返回首页 HTML / serve the dashboard index page.
async fn index_handler() -> Html<&'static str> {
    Html(INDEX_HTML)
}

/// HTTP: 返回当前任务列表 / list current tasks.
async fn list_tasks_handler(State(state): State<Arc<DashboardState>>) -> Json<Vec<TaskRecord>> {
    let tasks = state.tasks.read().await.values().cloned().collect();
    Json(tasks)
}

/// HTTP: 返回当前 worker 列表 / list current workers.
async fn list_workers_handler(State(state): State<Arc<DashboardState>>) -> Json<Vec<WorkerRecord>> {
    let workers = state.workers.read().await.values().cloned().collect();
    Json(workers)
}

/// WebSocket: 向浏览器推送实时事件 / websocket handler for browser clients.
async fn websocket_handler(
    State(state): State<Arc<DashboardState>>,
    ws: WebSocketUpgrade,
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(move |socket| browser_websocket_loop(state, socket))
}

async fn browser_websocket_loop(
    state: Arc<DashboardState>,
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

/// 内嵌首页 / embedded dashboard HTML.
const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>axon.ai Dashboard</title>
  <style>
    body { font-family: system-ui, sans-serif; margin: 2rem; background: #f7f7f8; color: #111; }
    h1 { font-size: 1.5rem; margin-bottom: 1rem; }
    h2 { font-size: 1.1rem; margin-top: 2rem; }
    table { width: 100%; border-collapse: collapse; background: #fff; margin-top: 0.5rem; }
    th, td { padding: 0.5rem; border: 1px solid #e5e5e5; text-align: left; font-size: 0.9rem; }
    th { background: #f0f0f0; }
    #status { font-size: 0.85rem; color: #666; margin-bottom: 1rem; }
    .connected { color: #16a34a; }
    .disconnected { color: #dc2626; }
  </style>
</head>
<body>
  <h1>axon.ai Dashboard</h1>
  <div id="status" class="disconnected">WebSocket 未连接</div>

  <h2>Tasks</h2>
  <table>
    <thead><tr><th>ID</th><th>Title</th><th>State</th><th>Worker</th><th>Updated</th></tr></thead>
    <tbody id="tasks-body"></tbody>
  </table>

  <h2>Workers</h2>
  <table>
    <thead><tr><th>ID</th><th>State</th><th>Task</th><th>Load</th><th>Heartbeat</th></tr></thead>
    <tbody id="workers-body"></tbody>
  </table>

  <script>
    const tasks = new Map();
    const workers = new Map();
    const ws = new WebSocket(`ws://${location.host}/ws`);

    ws.onopen = () => {
      document.getElementById('status').textContent = 'WebSocket 已连接';
      document.getElementById('status').className = 'connected';
    };
    ws.onclose = () => {
      document.getElementById('status').textContent = 'WebSocket 已断开';
      document.getElementById('status').className = 'disconnected';
    };
    ws.onmessage = (event) => {
      const msg = JSON.parse(event.data);
      if (msg.type === 'task_updated' || msg.type === 'task_result') {
        tasks.set(msg.task_updated?.task_id || msg.task_result?.task_id, msg.task_updated || msg.task_result);
      } else if (msg.type === 'worker_updated' || msg.type === 'heartbeat') {
        workers.set(msg.worker_updated?.worker_id || msg.heartbeat?.worker_id, msg.worker_updated || msg.heartbeat);
      }
      render();
    };

    function render() {
      document.getElementById('tasks-body').innerHTML = Array.from(tasks.values()).map(t =>
        `<tr><td>${t.task_id}</td><td>${t.title}</td><td>${t.state}</td><td>${t.worker_id || '-'}</td><td>${t.updated_at_ms}</td></tr>`
      ).join('');
      document.getElementById('workers-body').innerHTML = Array.from(workers.values()).map(w =>
        `<tr><td>${w.worker_id}</td><td>${w.state}</td><td>${w.task_id || '-'}</td><td>${w.load}</td><td>${w.last_heartbeat_ms}</td></tr>`
      ).join('');
    }

    // 初始拉取。
    fetch('/api/tasks').then(r => r.json()).then(list => { list.forEach(t => tasks.set(t.task_id, t)); render(); });
    fetch('/api/workers').then(r => r.json()).then(list => { list.forEach(w => workers.set(w.worker_id, w)); render(); });
  </script>
</body>
</html>"#;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axon_dispatcher::remote_dispatcher::RemoteDispatcher;
    use axon_proto::{Priority, Task, TaskState};
    use axon_queue::{InProcessQueue, RemoteTaskResult, WorkerState};

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

    /// 状态应用 TaskUpdated 会更新任务表并广播事件。
    #[tokio::test]
    async fn state_applies_task_updated_and_broadcasts() {
        let state = Arc::new(DashboardState::new(16));
        let mut rx = state.broadcast.subscribe();

        let record = TaskRecord {
            task_id: "t1".into(),
            state: TaskState::Queued,
            title: "task".into(),
            description: "desc".into(),
            worker_id: None,
            created_at_ms: 1,
            updated_at_ms: 2,
        };
        state
            .apply_dispatch_event(DispatchEvent::TaskUpdated(record.clone()))
            .await;

        assert_eq!(state.tasks.read().await.len(), 1);
        assert_eq!(state.tasks.read().await["t1"].state, TaskState::Queued);

        let event = rx.try_recv().expect("应收到广播事件");
        match event {
            DashboardEvent::TaskUpdated(r) => assert_eq!(r.task_id, "t1"),
            _ => panic!("预期 TaskUpdated 事件"),
        }
    }

    /// 状态应用 WorkerUpdated 会更新 worker 表并广播事件。
    #[tokio::test]
    async fn state_applies_worker_updated_and_broadcasts() {
        let state = Arc::new(DashboardState::new(16));
        let mut rx = state.broadcast.subscribe();

        let record = WorkerRecord {
            worker_id: "w1".into(),
            state: WorkerState::Idle,
            task_id: None,
            last_heartbeat_ms: 100,
            load: 0,
            registered_at_ms: 1,
        };
        state
            .apply_dispatch_event(DispatchEvent::WorkerUpdated(record.clone()))
            .await;

        assert_eq!(state.workers.read().await.len(), 1);
        assert_eq!(state.workers.read().await["w1"].state, WorkerState::Idle);

        let event = rx.try_recv().expect("应收到广播事件");
        match event {
            DashboardEvent::WorkerUpdated(r) => assert_eq!(r.worker_id, "w1"),
            _ => panic!("预期 WorkerUpdated 事件"),
        }
    }

    /// 状态应用 TaskResult 会更新对应任务状态;若任务不存在则不 panic。
    #[tokio::test]
    async fn state_applies_task_result_updates_existing_task() {
        let state = Arc::new(DashboardState::new(16));
        let record = TaskRecord {
            task_id: "t1".into(),
            state: TaskState::Running,
            title: "task".into(),
            description: "desc".into(),
            worker_id: Some("w1".into()),
            created_at_ms: 1,
            updated_at_ms: 2,
        };
        state
            .apply_dispatch_event(DispatchEvent::TaskUpdated(record))
            .await;

        state
            .apply_dispatch_event(DispatchEvent::TaskResult(RemoteTaskResult {
                task_id: "t1".into(),
                worker_id: "w1".into(),
                success: true,
                summary: "done".into(),
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
                completed_at_ms: 10,
            }))
            .await;

        assert_eq!(state.tasks.read().await["t1"].state, TaskState::Completed);
    }

    /// 状态应用未知任务 ID 的 TaskResult 不会 panic。
    #[tokio::test]
    async fn state_applies_task_result_for_unknown_task_is_noop() {
        let state = Arc::new(DashboardState::new(16));
        state
            .apply_dispatch_event(DispatchEvent::TaskResult(RemoteTaskResult {
                task_id: "unknown".into(),
                worker_id: "w1".into(),
                success: false,
                summary: String::new(),
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 1,
                completed_at_ms: 10,
            }))
            .await;
        assert!(state.tasks.read().await.is_empty());
    }

    /// dashboard HTTP 服务返回首页 HTML。
    #[tokio::test]
    async fn dashboard_serves_index_html() {
        let dashboard = Dashboard::new("http://localhost:1", "127.0.0.1:0".parse().unwrap());
        let state = dashboard.state.clone();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/", get(index_handler))
            .route("/api/tasks", get(list_tasks_handler))
            .route("/api/workers", get(list_workers_handler))
            .route("/ws", get(websocket_handler))
            .with_state(state);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let html = reqwest::get(format!("http://{addr}/"))
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(html.contains("axon.ai Dashboard"));
    }

    /// dashboard HTTP API 返回当前任务与 worker 列表。
    #[tokio::test]
    async fn dashboard_api_returns_tasks_and_workers() {
        let state = Arc::new(DashboardState::new(16));
        state
            .apply_dispatch_event(DispatchEvent::TaskUpdated(TaskRecord {
                task_id: "t1".into(),
                state: TaskState::Queued,
                title: "task".into(),
                description: "desc".into(),
                worker_id: None,
                created_at_ms: 1,
                updated_at_ms: 2,
            }))
            .await;
        state
            .apply_dispatch_event(DispatchEvent::WorkerUpdated(WorkerRecord {
                worker_id: "w1".into(),
                state: WorkerState::Idle,
                task_id: None,
                last_heartbeat_ms: 100,
                load: 0,
                registered_at_ms: 1,
            }))
            .await;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/api/tasks", get(list_tasks_handler))
            .route("/api/workers", get(list_workers_handler))
            .with_state(state);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let tasks: Vec<TaskRecord> = reqwest::get(format!("http://{addr}/api/tasks"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let workers: Vec<WorkerRecord> = reqwest::get(format!("http://{addr}/api/workers"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        assert_eq!(tasks.len(), 1);
        assert_eq!(workers.len(), 1);
    }

    /// dashboard WebSocket 将状态变更事件推送给浏览器客户端。
    #[tokio::test]
    async fn dashboard_websocket_forwards_events() {
        let state = Arc::new(DashboardState::new(16));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/ws", get(websocket_handler))
            .with_state(state.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let ws_url = format!("ws://{addr}/ws");
        let (mut ws_stream, _response) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();

        // 等待订阅建立。
        tokio::time::sleep(Duration::from_millis(20)).await;

        state
            .apply_dispatch_event(DispatchEvent::TaskUpdated(TaskRecord {
                task_id: "t1".into(),
                state: TaskState::Queued,
                title: "task".into(),
                description: "desc".into(),
                worker_id: None,
                created_at_ms: 1,
                updated_at_ms: 2,
            }))
            .await;

        let msg = ws_stream.next().await.unwrap().unwrap();
        let text = msg.to_text().unwrap();
        let event: DashboardEvent = serde_json::from_str(text).unwrap();
        match event {
            DashboardEvent::TaskUpdated(r) => assert_eq!(r.task_id, "t1"),
            _ => panic!("预期 TaskUpdated 事件"),
        }
    }

    /// sync_once 能从 mock dispatcher 拉取状态并订阅 WebSocket 事件。
    #[tokio::test]
    async fn sync_once_pulls_state_and_events_from_dispatcher() {
        let queue = Arc::new(InProcessQueue::new());
        let dispatcher = RemoteDispatcher::new(queue.clone());

        dispatcher.submit_task(sample_task("t1")).await.unwrap();

        // mock dispatcher HTTP API。
        let state_for_dispatcher = dispatcher.state();
        let app = Router::new()
            .route("/tasks", get(list_tasks_mock))
            .route("/workers", get(list_workers_mock))
            .route("/ws", get(dispatcher_websocket_handler))
            .with_state(state_for_dispatcher);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dispatcher_addr = listener.local_addr().unwrap();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let server = axum::serve(listener, app);
            let _ = ready_tx.send(());
            server.await.unwrap();
        });
        ready_rx.await.unwrap();

        let dashboard_state = Arc::new(DashboardState::new(16));
        let dispatcher_url = format!("http://{dispatcher_addr}");
        let dashboard_state_clone = Arc::clone(&dashboard_state);

        // 启动单次同步,稍后在后台发布事件。
        let sync_handle = tokio::spawn(async move {
            sync_once(&dashboard_state_clone, &dispatcher_url)
                .await
                .ok();
        });

        // 给 sync_once 时间完成 HTTP 拉取与 WebSocket 订阅。
        tokio::time::sleep(Duration::from_millis(100)).await;

        // 通过 dispatcher 的广播通道发送事件(模拟 worker 完成)。
        let _ = dispatcher
            .state()
            .broadcast
            .send(DispatchEvent::TaskResult(RemoteTaskResult {
                task_id: "t1".into(),
                worker_id: "w1".into(),
                success: true,
                summary: "done".into(),
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
                completed_at_ms: 100,
            }));

        tokio::time::sleep(Duration::from_millis(50)).await;
        sync_handle.abort();

        let tasks = dashboard_state.tasks.read().await;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks["t1"].state, TaskState::Completed);
    }

    async fn list_tasks_mock(
        State(state): State<Arc<axon_dispatcher::remote_dispatcher::RemoteDispatcherState>>,
    ) -> Json<Vec<TaskRecord>> {
        let tasks = state.tasks.read().await.values().cloned().collect();
        Json(tasks)
    }

    async fn list_workers_mock() -> Json<Vec<WorkerRecord>> {
        Json(vec![])
    }

    async fn dispatcher_websocket_handler(
        State(state): State<Arc<axon_dispatcher::remote_dispatcher::RemoteDispatcherState>>,
        ws: WebSocketUpgrade,
    ) -> impl axum::response::IntoResponse {
        ws.on_upgrade(move |socket| mock_dispatcher_websocket_loop(state, socket))
    }

    async fn mock_dispatcher_websocket_loop(
        state: Arc<axon_dispatcher::remote_dispatcher::RemoteDispatcherState>,
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
}
