//! axon-memory — 记忆大脑 / memory brain.
//!
//! 提供分层记忆存储:
//! - **短期记忆 (Short-term)**: 单次会话上下文(M1 实现为内存 LRU)
//! - **情景记忆 (Episodic)**: 过往任务/对话片段(Qdrant 向量检索,M2)
//! - **语义记忆 (Semantic)**: 结构化事实(redb KV,M2)
//! - **用户画像 (User Profile)**: 高权重长期记忆,沉淀用户开发特点
//!
//! 通过 [`MemoryStore`] trait 抽象,用户可经 CLI / Web 调节记忆权重与遗忘。

#![allow(dead_code)]

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use axon_core::{Id, MemoryId, Result, Timestamp};

/// 记忆类别 / memory category (分层)。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// 短期会话上下文 / short-term session context.
    ShortTerm,
    /// 情景记忆:过往任务/对话片段 / episodic.
    Episodic,
    /// 语义记忆:结构化事实 / semantic facts.
    Semantic,
    /// 用户画像:偏好/开发习惯 / user profile.
    UserProfile,
}

/// 单条记忆 / a single memory record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: MemoryId,
    pub kind: MemoryKind,
    /// 文本内容(用于检索与展示)/ textual content.
    pub content: String,
    /// 可选向量(由 store 侧计算填充)/ optional embedding.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
    /// 权重:影响召回排序,用户可调节 / weight, user-adjustable.
    pub weight: f32,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// 来源标签(如 "session-xyz")/ optional source tag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// 召回查询 / a recall query.
#[derive(Debug, Clone)]
pub struct RecallQuery {
    /// 文本查询(会被向量化)/ text query.
    pub query: String,
    /// 限定类别(可选)/ restrict to a kind.
    pub kind: Option<MemoryKind>,
    /// 返回 Top-K / top-k results.
    pub top_k: usize,
}

/// 列举/管理用过滤条件 / filter for memory management listing.
#[derive(Debug, Clone, Default)]
pub struct MemoryFilter {
    pub kind: Option<MemoryKind>,
    pub source: Option<String>,
    /// 仅返回权重 >= 此值的记忆 / minimum weight.
    pub min_weight: Option<f32>,
}

/// 记忆存储抽象 / the memory store trait.
///
/// 实现者负责持久化与检索;用户通过 `adjust_weight` / `forget` 调节记忆。
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// 写入一条记忆(返回分配的 id)/ store a memory.
    async fn store(&self, mem: Memory) -> Result<MemoryId>;

    /// 按查询召回相关记忆 / recall relevant memories.
    async fn recall(&self, query: &RecallQuery) -> Result<Vec<Memory>>;

    /// 按过滤条件列举(用于记忆管理 UI)/ list memories matching a filter.
    async fn list(&self, filter: &MemoryFilter) -> Result<Vec<Memory>>;

    /// 读取单条 / fetch one by id.
    async fn get(&self, id: &MemoryId) -> Result<Option<Memory>>;

    /// 调节权重 / adjust a memory's weight (user-controllable).
    async fn adjust_weight(&self, id: &MemoryId, weight: f32) -> Result<()>;

    /// 遗忘 / forget (delete) a memory.
    async fn forget(&self, id: &MemoryId) -> Result<()>;
}

/// 内置默认权重 / default weight for a new memory.
pub const DEFAULT_WEIGHT: f32 = 1.0;

/// 占位:生成新记忆 id / placeholder id generator.
pub fn new_memory_id() -> Id {
    axon_core::new_id()
}
