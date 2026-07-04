//! 混合记忆存储 / hybrid memory store.
//!
//! 组合一个“语义/用户画像”存储 (`RedbStore`) 与一个“情景向量”存储 (`QdrantStore`)，
//! 对外提供统一的 `MemoryStore`。

use async_trait::async_trait;

use axon_core::{MemoryId, Result};

use crate::{Memory, MemoryFilter, MemoryKind, MemoryStore, RecallQuery};

/// 混合记忆存储 / hybrid memory store.
///
/// 泛型:
/// - `S`: 语义/用户画像/短期记忆存储（如 `RedbStore`）
/// - `Q`: 情景向量记忆存储（如 `QdrantStore`）
pub struct HybridMemoryStore<S, Q> {
    semantic: S,
    episodic: Q,
}

impl<S, Q> HybridMemoryStore<S, Q>
where
    S: MemoryStore,
    Q: MemoryStore,
{
    /// 由两个已构造的存储创建混合存储 / create from two existing stores.
    pub fn new(semantic: S, episodic: Q) -> Self {
        Self { semantic, episodic }
    }
}

impl<S, Q> std::fmt::Debug for HybridMemoryStore<S, Q>
where
    S: std::fmt::Debug,
    Q: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HybridMemoryStore")
            .field("semantic", &self.semantic)
            .field("episodic", &self.episodic)
            .finish()
    }
}

#[async_trait]
impl<S, Q> MemoryStore for HybridMemoryStore<S, Q>
where
    S: MemoryStore,
    Q: MemoryStore,
{
    async fn store(&self, mem: Memory) -> Result<MemoryId> {
        match mem.kind {
            MemoryKind::Episodic => self.episodic.store(mem).await,
            _ => self.semantic.store(mem).await,
        }
    }

    async fn recall(&self, query: &RecallQuery) -> Result<Vec<Memory>> {
        // 语义记忆：文本子串匹配。
        let semantic_hits = self.semantic.recall(query).await?;
        // 情景记忆：向量相似度。
        let episodic_hits = self.episodic.recall(query).await?;

        let mut combined = semantic_hits;
        combined.extend(episodic_hits);
        // 按权重降序；M4 可结合向量相似度分数做更精细排序。
        combined.sort_by(|a, b| {
            b.weight
                .partial_cmp(&a.weight)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let top_k = query.top_k.max(1);
        Ok(combined.into_iter().take(top_k).collect())
    }

    async fn list(&self, filter: &MemoryFilter) -> Result<Vec<Memory>> {
        let mut items = self.semantic.list(filter).await?;
        items.extend(self.episodic.list(filter).await?);
        items.sort_by_key(|m| std::cmp::Reverse(m.created_at));
        Ok(items)
    }

    async fn get(&self, id: &MemoryId) -> Result<Option<Memory>> {
        if let Some(mem) = self.semantic.get(id).await? {
            return Ok(Some(mem));
        }
        self.episodic.get(id).await
    }

    async fn adjust_weight(&self, id: &MemoryId, weight: f32) -> Result<()> {
        if let Some(mut mem) = self.get(id).await? {
            mem.weight = weight;
            self.store(mem).await?;
            return Ok(());
        }
        Err(axon_core::Error::NotFound(format!("memory {id}")))
    }

    async fn forget(&self, id: &MemoryId) -> Result<()> {
        // 两边都尝试删除；忽略未找到的错误。
        let _ = self.semantic.forget(id).await;
        let _ = self.episodic.forget(id).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::in_memory::InMemoryStore;

    fn sample_memory(content: &str, kind: MemoryKind) -> Memory {
        Memory {
            id: String::new(),
            kind,
            content: content.into(),
            embedding: None,
            weight: 1.0,
            created_at: 0,
            updated_at: 0,
            source: None,
        }
    }

    /// 验证 Hybrid 按 MemoryKind 路由到不同子存储。
    #[tokio::test]
    async fn routes_by_kind() {
        let semantic = InMemoryStore::new();
        let episodic = InMemoryStore::new();
        let hybrid = HybridMemoryStore::new(semantic, episodic);

        let semantic_id = hybrid
            .store(sample_memory("user prefers rust", MemoryKind::UserProfile))
            .await
            .unwrap();
        let episodic_id = hybrid
            .store(sample_memory(
                "previous task about axon",
                MemoryKind::Episodic,
            ))
            .await
            .unwrap();

        let semantic_mem = hybrid.get(&semantic_id).await.unwrap().unwrap();
        assert_eq!(semantic_mem.kind, MemoryKind::UserProfile);

        let episodic_mem = hybrid.get(&episodic_id).await.unwrap().unwrap();
        assert_eq!(episodic_mem.kind, MemoryKind::Episodic);

        let all = hybrid.list(&MemoryFilter::default()).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    /// 验证 recall 合并两路结果。
    #[tokio::test]
    async fn recall_merges_results() {
        let semantic = InMemoryStore::new();
        let episodic = InMemoryStore::new();
        let hybrid = HybridMemoryStore::new(semantic, episodic);

        hybrid
            .store(sample_memory("rust best practices", MemoryKind::Semantic))
            .await
            .unwrap();
        hybrid
            .store(sample_memory("we built axon in rust", MemoryKind::Episodic))
            .await
            .unwrap();

        let hits = hybrid
            .recall(&RecallQuery {
                query: "rust".into(),
                kind: None,
                top_k: 10,
            })
            .await
            .unwrap();
        assert_eq!(hits.len(), 2);
    }
}
