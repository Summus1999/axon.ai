//! 进程内任务队列 / in-process task queue.
//!
//! M1 实现：用 `tokio::sync::Mutex` 保护任务列表与状态表，
//! 不依赖外部 broker，让单机 CLI 可立即跑通。

use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::Mutex;

use axon_core::{Id, Result, TaskId};
use axon_proto::{Task, TaskState};

use crate::TaskQueue;

/// 进程内任务队列 / in-process task queue.
#[derive(Debug, Default)]
pub struct InProcessQueue {
    tasks: Mutex<Vec<Task>>,
    states: Mutex<HashMap<TaskId, TaskState>>,
}

impl InProcessQueue {
    /// 创建一个新的空队列 / create a new empty queue.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl TaskQueue for InProcessQueue {
    async fn enqueue(&self, mut task: Task) -> Result<()> {
        let id = task.id.clone();
        task.state = TaskState::Queued;
        self.states
            .lock()
            .await
            .insert(id.clone(), TaskState::Queued);
        self.tasks.lock().await.push(task);
        Ok(())
    }

    async fn dequeue(&self) -> Result<Task> {
        let mut tasks = self.tasks.lock().await;
        let states = self.states.lock().await;

        let idx = tasks.iter().position(|t| {
            t.state == TaskState::Queued
                && t.dependencies.iter().all(|dep| {
                    states
                        .get(dep)
                        .is_some_and(|s| matches!(s, TaskState::Completed))
                })
        });

        match idx {
            Some(i) => {
                let mut task = tasks.remove(i);
                task.state = TaskState::Scheduled;
                drop(states);
                self.states
                    .lock()
                    .await
                    .insert(task.id.clone(), TaskState::Scheduled);
                Ok(task)
            }
            None => Err(axon_core::Error::Dispatcher(
                "no runnable tasks in queue".into(),
            )),
        }
    }

    async fn update_state(&self, id: &TaskId, state: TaskState) -> Result<()> {
        self.states.lock().await.insert(id.clone(), state);
        Ok(())
    }

    async fn subscribe(&self) -> Result<Id> {
        Ok(axon_core::new_id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_task(id: &str) -> Task {
        Task {
            id: id.into(),
            parent: None,
            title: "t".into(),
            description: "desc".into(),
            priority: axon_proto::Priority::Normal,
            state: TaskState::Queued,
            dependencies: vec![],
            created_at: 0,
            updated_at: 0,
            acceptance: vec![],
        }
    }

    /// 验证入队后可出队。
    #[tokio::test]
    async fn enqueue_and_dequeue() {
        let queue = InProcessQueue::new();
        queue.enqueue(sample_task("t1")).await.unwrap();
        let task = queue.dequeue().await.unwrap();
        assert_eq!(task.id, "t1");
        assert_eq!(task.state, TaskState::Scheduled);
    }

    /// 空队列出队返回错误。
    #[tokio::test]
    async fn empty_queue_errors() {
        let queue = InProcessQueue::new();
        let err = queue.dequeue().await.unwrap_err();
        assert!(matches!(err, axon_core::Error::Dispatcher(_)));
    }

    /// 验证依赖未完成的任务不会被调度。
    #[tokio::test]
    async fn dequeue_respects_dependencies() {
        let queue = InProcessQueue::new();
        let dep_task = sample_task("dep");
        let mut task = sample_task("task");
        task.dependencies = vec!["dep".into()];
        queue.enqueue(task).await.unwrap();
        queue.enqueue(dep_task).await.unwrap();

        // 先拿到 dep，完成后更新状态。
        let first = queue.dequeue().await.unwrap();
        assert_eq!(first.id, "dep");
        queue
            .update_state(&first.id, TaskState::Completed)
            .await
            .unwrap();

        // 此时 task 的依赖已满足，可以出队。
        let second = queue.dequeue().await.unwrap();
        assert_eq!(second.id, "task");
    }
}
