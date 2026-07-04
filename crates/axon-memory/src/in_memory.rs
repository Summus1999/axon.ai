//! 内存记忆存储 / in-memory memory store.
//!
//! M1 占位实现：用 `std::sync::RwLock<HashMap>` 存储记忆，
//! 不依赖外部 Qdrant/redb 服务，让单机 CLI 可立即跑通。
//! `recall` 暂按文本子串匹配 + 权重排序，M2 替换为向量检索。

use std::collections::HashMap;
use std::sync::RwLock;

use async_trait::async_trait;

use axon_core::{MemoryId, Result, Timestamp};

use crate::{Memory, MemoryFilter, MemoryStore, RecallQuery, DEFAULT_WEIGHT};

/// 内存记忆存储 / in-memory memory store.
#[derive(Debug, Default)]
pub struct InMemoryStore {
    data: RwLock<HashMap<MemoryId, Memory>>,
}

impl InMemoryStore {
    /// 创建一个新的空存储 / create a new empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl MemoryStore for InMemoryStore {
    async fn store(&self, mut mem: Memory) -> Result<MemoryId> {
        if mem.id.is_empty() {
            mem.id = crate::new_memory_id();
        }
        if mem.weight == 0.0 {
            mem.weight = DEFAULT_WEIGHT;
        }
        let now = now_ms();
        if mem.created_at == 0 {
            mem.created_at = now;
        }
        mem.updated_at = now;
        let id = mem.id.clone();
        self.data.write().unwrap().insert(id.clone(), mem);
        Ok(id)
    }

    async fn recall(&self, query: &RecallQuery) -> Result<Vec<Memory>> {
        let q = query.query.to_lowercase();
        let guard = self.data.read().unwrap();
        let mut hits: Vec<&Memory> = guard
            .values()
            .filter(|m| {
                let kind_ok = query.kind.map_or(true, |k| m.kind == k);
                kind_ok && m.content.to_lowercase().contains(&q)
            })
            .collect();
        // 按权重降序排列；M2 再按向量相似度排序。
        hits.sort_by(|a, b| {
            b.weight
                .partial_cmp(&a.weight)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let top_k = query.top_k.max(1);
        Ok(hits.into_iter().take(top_k).cloned().collect())
    }

    async fn list(&self, filter: &MemoryFilter) -> Result<Vec<Memory>> {
        let guard = self.data.read().unwrap();
        let mut items: Vec<Memory> = guard
            .values()
            .filter(|m| {
                let kind_ok = filter.kind.map_or(true, |k| m.kind == k);
                let source_ok = filter
                    .source
                    .as_ref()
                    .map_or(true, |s| m.source.as_ref() == Some(s));
                let weight_ok = filter.min_weight.map_or(true, |w| m.weight >= w);
                kind_ok && source_ok && weight_ok
            })
            .cloned()
            .collect();
        items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(items)
    }

    async fn get(&self, id: &MemoryId) -> Result<Option<Memory>> {
        Ok(self.data.read().unwrap().get(id).cloned())
    }

    async fn adjust_weight(&self, id: &MemoryId, weight: f32) -> Result<()> {
        let mut guard = self.data.write().unwrap();
        let mem = guard
            .get_mut(id)
            .ok_or_else(|| axon_core::Error::NotFound(format!("memory {id}")))?;
        mem.weight = weight;
        mem.updated_at = now_ms();
        Ok(())
    }

    async fn forget(&self, id: &MemoryId) -> Result<()> {
        self.data
            .write()
            .unwrap()
            .remove(id)
            .ok_or_else(|| axon_core::Error::NotFound(format!("memory {id}")))?;
        Ok(())
    }
}

fn now_ms() -> Timestamp {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryKind;

    fn sample_memory(content: &str, kind: MemoryKind) -> Memory {
        Memory {
            id: String::new(),
            kind,
            content: content.into(),
            embedding: None,
            weight: DEFAULT_WEIGHT,
            created_at: 0,
            updated_at: 0,
            source: None,
        }
    }

    /// 验证 store 返回有效 id 且能 get 到原记录。
    #[tokio::test]
    async fn store_and_get() {
        let store = InMemoryStore::new();
        let mem = sample_memory("use anyhow in binaries", MemoryKind::Semantic);
        let id = store.store(mem.clone()).await.unwrap();
        assert!(!id.is_empty());

        let fetched = store.get(&id).await.unwrap().unwrap();
        assert_eq!(fetched.content, "use anyhow in binaries");
        assert_eq!(fetched.kind, MemoryKind::Semantic);
    }

    /// 验证 recall 按查询文本召回并按权重排序。
    #[tokio::test]
    async fn recall_matches_content_and_sorts_by_weight() {
        let store = InMemoryStore::new();
        let mut high = sample_memory("prefer tokio for async runtime", MemoryKind::Semantic);
        high.weight = 2.0;
        let mut low = sample_memory("tokio is the standard async runtime", MemoryKind::Semantic);
        low.weight = 1.0;
        store.store(high).await.unwrap();
        store.store(low).await.unwrap();

        let query = RecallQuery {
            query: "tokio".into(),
            kind: None,
            top_k: 10,
        };
        let hits = store.recall(&query).await.unwrap();
        assert_eq!(hits.len(), 2);
        assert!(hits[0].weight > hits[1].weight);
    }

    /// 验证 list 支持按类别与最小权重过滤。
    #[tokio::test]
    async fn list_filter_by_kind_and_weight() {
        let store = InMemoryStore::new();
        let mut profile = sample_memory("user prefers rust", MemoryKind::UserProfile);
        profile.weight = 1.5;
        store
            .store(sample_memory(
                "some short term context",
                MemoryKind::ShortTerm,
            ))
            .await
            .unwrap();
        store.store(profile).await.unwrap();

        let filter = MemoryFilter {
            kind: Some(MemoryKind::UserProfile),
            source: None,
            min_weight: Some(1.0),
        };
        let items = store.list(&filter).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].content, "user prefers rust");
    }

    /// 验证 adjust_weight 和 forget。
    #[tokio::test]
    async fn adjust_weight_and_forget() {
        let store = InMemoryStore::new();
        let id = store
            .store(sample_memory("to be forgotten", MemoryKind::Episodic))
            .await
            .unwrap();

        store.adjust_weight(&id, 0.5).await.unwrap();
        let mem = store.get(&id).await.unwrap().unwrap();
        assert!((mem.weight - 0.5).abs() < f32::EPSILON);

        store.forget(&id).await.unwrap();
        assert!(store.get(&id).await.unwrap().is_none());
    }
}
