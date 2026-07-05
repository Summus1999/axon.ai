//! axon-brain — AI 调度大脑 / the AI scheduling brain.
//!
//! 负责理解用户意图、规划任务、编排 LLM。核心抽象:
//! - [`Planner`]: 将用户高层目标拆解为可执行的 [`Task`] DAG
//! - [`Agent`]: 在 worker 内执行单个任务的 ReAct 循环
//!
//! Planner 读取记忆大脑沉淀的用户画像以个性化规划。
//! 具体实现(ReAct 多步、工具调用)留待 M1/M5。

pub mod command_agent;
pub mod profile_extractor;
pub mod reviewer;
pub mod simple_planner;

use async_trait::async_trait;

use axon_core::{Result, TaskId};
use axon_llm::{LlmProvider, Message};
use axon_memory::MemoryStore;
use axon_proto::Task;

pub use command_agent::CommandAgent;
pub use profile_extractor::{LlmProfileExtractor, ProfileExtractor};
pub use reviewer::{LlmReviewer, ReviewResult, Reviewer, RuleReviewer, TaskOutput};
pub use simple_planner::SimplePlanner;

/// 用户下达的高层目标 / a high-level goal from the user.
#[derive(Debug, Clone)]
pub struct Goal {
    pub description: String,
    /// 用户附加的上下文 / extra context from the user.
    pub context: Vec<Message>,
}

/// 规划结果 / planning output: a DAG of tasks.
#[derive(Debug, Clone)]
pub struct Plan {
    pub tasks: Vec<Task>,
    /// 任务依赖邻接表(task_id → 依赖的 task_ids)/ dependency adjacency.
    pub dependencies: Vec<(TaskId, TaskId)>,
}

/// 规划器 / the planner trait.
///
/// 实现者用 LLM + 记忆把 [`Goal`] 拆解为 [`Plan`]。
#[async_trait]
pub trait Planner: Send + Sync {
    async fn plan(
        &self,
        goal: &Goal,
        memory: &dyn MemoryStore,
        llm: &dyn LlmProvider,
    ) -> Result<Plan>;
}

/// 执行器 / the per-task agent trait (runs inside a worker/VM).
#[async_trait]
pub trait Agent: Send + Sync {
    /// 执行单个任务,返回产物路径与自检结果 / execute one task.
    async fn execute(
        &self,
        task: &Task,
        llm: &dyn LlmProvider,
        memory: &dyn MemoryStore,
    ) -> Result<AgentOutput>;
}

/// Agent 执行产物 / output of a single task execution.
#[derive(Debug, Clone)]
pub struct AgentOutput {
    /// 产出说明 / textual summary of artifacts.
    pub summary: String,
    /// 改动文件列表 / changed file paths.
    pub changed_files: Vec<String>,
    /// 自检是否通过 / self-check passed.
    pub self_check_passed: bool,
    /// 自检日志 / self-check log.
    pub log: String,
}

/// 占位规划器 / placeholder planner (errors out, M1 implements).
pub struct UnimplementedPlanner;

#[async_trait]
impl Planner for UnimplementedPlanner {
    async fn plan(
        &self,
        _goal: &Goal,
        _memory: &dyn MemoryStore,
        _llm: &dyn LlmProvider,
    ) -> Result<Plan> {
        Err(axon_core::Error::Other(
            "Planner not yet implemented (skeleton, M1)".into(),
        ))
    }
}
