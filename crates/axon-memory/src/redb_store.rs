//! redb 本地 KV 记忆存储 / redb-backed local memory store.
//!
//! 负责持久化:
//! - 语义记忆 (Semantic)
//! - 用户画像 (UserProfile)
//! - 短期记忆占位 (ShortTerm，M2 仍主要放内存，但可落盘)
//!
//! 情景记忆 (Episodic) 由 `QdrantStore` 负责向量检索。

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use redb::{Database, ReadableTable, TableDefinition};

use axon_core::{MemoryId, Result};

use crate::{Memory, MemoryFilter, MemoryKind, MemoryStore, RecallQuery, DEFAULT_WEIGHT};

const SEMANTIC_TABLE: TableDefinition<&str, &str> = TableDefinition::new("semantic_memories");
const PROFILE_TABLE: TableDefinition<&str, &str> = TableDefinition::new("user_profiles");
const SHORT_TERM_TABLE: TableDefinition<&str, &str> = TableDefinition::new("short_term_memories");

/// redb 本地记忆存储 / redb-backed memory store.
#[derive(Debug, Clone)]
pub struct RedbStore {
    path: PathBuf,
}

impl RedbStore {
    /// 打开或创建指定路径的 redb 存储 / open or create a redb store at path.
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(axon_core::Error::Io)?;
        }
        Database::create(&path).map_err(redb_error)?;
        Ok(Self { path })
    }

    fn table_for_kind(kind: MemoryKind) -> TableDefinition<'static, &'static str, &'static str> {
        match kind {
            MemoryKind::ShortTerm => SHORT_TERM_TABLE,
            MemoryKind::Semantic => SEMANTIC_TABLE,
            MemoryKind::UserProfile => PROFILE_TABLE,
            MemoryKind::Episodic => SEMANTIC_TABLE,
        }
    }

    fn db(&self) -> Result<Database> {
        Database::create(&self.path).map_err(redb_error)
    }
}

#[async_trait]
impl MemoryStore for RedbStore {
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
        if mem.updated_at == 0 {
            mem.updated_at = now;
        }

        let table = Self::table_for_kind(mem.kind);
        let json = serde_json::to_string(&mem).map_err(axon_core::Error::Json)?;

        let db = self.db()?;
        let tx = db.begin_write().map_err(redb_error)?;
        {
            let mut t = tx.open_table(table).map_err(redb_error)?;
            t.insert(mem.id.as_str(), json.as_str())
                .map_err(redb_error)?;
        }
        tx.commit().map_err(redb_error)?;

        Ok(mem.id)
    }

    async fn recall(&self, query: &RecallQuery) -> Result<Vec<Memory>> {
        let q = query.query.to_lowercase();
        let all = self.list(&MemoryFilter::default()).await?;
        let mut hits: Vec<Memory> = all
            .into_iter()
            .filter(|m| {
                let kind_ok = query.kind.map_or(true, |k| m.kind == k);
                kind_ok && m.content.to_lowercase().contains(&q)
            })
            .collect();
        hits.sort_by(|a, b| {
            b.weight
                .partial_cmp(&a.weight)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let top_k = query.top_k.max(1);
        Ok(hits.into_iter().take(top_k).collect())
    }

    async fn list(&self, filter: &MemoryFilter) -> Result<Vec<Memory>> {
        let db = self.db()?;
        let mut items = Vec::new();

        for table in [SHORT_TERM_TABLE, SEMANTIC_TABLE, PROFILE_TABLE] {
            let tx = db.begin_read().map_err(redb_error)?;
            let table = match tx.open_table(table) {
                Ok(t) => t,
                Err(redb::TableError::TableDoesNotExist(_)) => continue,
                Err(e) => return Err(redb_error(e)),
            };
            for row in table.iter().map_err(redb_error)? {
                let (_, value) = row.map_err(redb_error)?;
                let mem: Memory =
                    serde_json::from_str(value.value()).map_err(axon_core::Error::Json)?;
                if memory_matches_filter(&mem, filter) {
                    items.push(mem);
                }
            }
        }

        items.sort_by_key(|m| std::cmp::Reverse(m.created_at));
        Ok(items)
    }

    async fn get(&self, id: &MemoryId) -> Result<Option<Memory>> {
        let db = self.db()?;
        for table in [SHORT_TERM_TABLE, SEMANTIC_TABLE, PROFILE_TABLE] {
            let tx = db.begin_read().map_err(redb_error)?;
            let t = match tx.open_table(table) {
                Ok(t) => t,
                Err(redb::TableError::TableDoesNotExist(_)) => continue,
                Err(e) => return Err(redb_error(e)),
            };
            if let Some(value) = t.get(id.as_str()).map_err(redb_error)? {
                let mem: Memory =
                    serde_json::from_str(value.value()).map_err(axon_core::Error::Json)?;
                return Ok(Some(mem));
            }
        }
        Ok(None)
    }

    async fn adjust_weight(&self, id: &MemoryId, weight: f32) -> Result<()> {
        let mut mem = self
            .get(id)
            .await?
            .ok_or_else(|| axon_core::Error::NotFound(format!("memory {id}")))?;
        mem.weight = weight;
        mem.updated_at = now_ms();
        self.store(mem).await?;
        Ok(())
    }

    async fn forget(&self, id: &MemoryId) -> Result<()> {
        let db = self.db()?;
        for table in [SHORT_TERM_TABLE, SEMANTIC_TABLE, PROFILE_TABLE] {
            let tx = db.begin_write().map_err(redb_error)?;
            let found = {
                let mut t = tx.open_table(table).map_err(redb_error)?;
                let found = t.remove(id.as_str()).map_err(redb_error)?.is_some();
                drop(t);
                found
            };
            if found {
                tx.commit().map_err(redb_error)?;
                return Ok(());
            }
            tx.abort().map_err(redb_error)?;
        }
        Err(axon_core::Error::NotFound(format!("memory {id}")))
    }

    async fn decay_weights(&self, half_life_days: f32) -> Result<()> {
        if half_life_days <= 0.0 {
            return Ok(());
        }

        let now = now_ms();
        let all = self.list(&MemoryFilter::default()).await?;
        for mut mem in all {
            let elapsed_days = (now.saturating_sub(mem.updated_at)) as f32 / MS_PER_DAY;
            let factor = 0.5_f32.powf(elapsed_days / half_life_days);
            let new_weight = mem.weight * factor;
            if (new_weight - mem.weight).abs() > f32::EPSILON {
                mem.weight = new_weight;
                mem.updated_at = now;
                self.store(mem).await?;
            }
        }
        Ok(())
    }
}

const MS_PER_DAY: f32 = 24.0 * 60.0 * 60.0 * 1_000.0;

fn redb_error<E: std::fmt::Display>(e: E) -> axon_core::Error {
    axon_core::Error::Memory(format!("redb error: {e}"))
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn memory_matches_filter(mem: &Memory, filter: &MemoryFilter) -> bool {
    let kind_ok = filter.kind.map_or(true, |k| mem.kind == k);
    let source_ok = filter
        .source
        .as_ref()
        .map_or(true, |s| mem.source.as_ref() == Some(s));
    let weight_ok = filter.min_weight.map_or(true, |w| mem.weight >= w);
    kind_ok && source_ok && weight_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> RedbStore {
        let dir = std::env::temp_dir().join(format!("axon-redb-test-{}", uuid::Uuid::new_v4()));
        RedbStore::new(dir.join("memory.redb")).unwrap()
    }

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

    /// 验证 store/get 持久化。
    #[tokio::test]
    async fn store_and_get() {
        let store = temp_db();
        let id = store
            .store(sample_memory(
                "use anyhow in binaries",
                MemoryKind::Semantic,
            ))
            .await
            .unwrap();
        let mem = store.get(&id).await.unwrap().unwrap();
        assert_eq!(mem.content, "use anyhow in binaries");
    }

    /// 验证 list 过滤。
    #[tokio::test]
    async fn list_filter() {
        let store = temp_db();
        store
            .store(sample_memory("short term", MemoryKind::ShortTerm))
            .await
            .unwrap();
        let mut profile = sample_memory("user prefers rust", MemoryKind::UserProfile);
        profile.weight = 1.5;
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
    async fn adjust_and_forget() {
        let store = temp_db();
        let id = store
            .store(sample_memory("to be forgotten", MemoryKind::Episodic))
            .await
            .unwrap();
        store.adjust_weight(&id, 0.3).await.unwrap();
        let mem = store.get(&id).await.unwrap().unwrap();
        assert!((mem.weight - 0.3).abs() < f32::EPSILON);

        store.forget(&id).await.unwrap();
        assert!(store.get(&id).await.unwrap().is_none());
    }

    /// 验证权重衰减按半衰期降低旧记忆权重。
    #[tokio::test]
    async fn decay_weights_reduces_old_memories() {
        let store = temp_db();
        let mut mem = sample_memory("old preference", MemoryKind::UserProfile);
        mem.weight = 1.0;
        // 将更新时间设为一天前,半衰期 1 天,权重应衰减为 0.5。
        mem.updated_at = now_ms() - (24 * 60 * 60 * 1_000);
        let id = store.store(mem).await.unwrap();

        store.decay_weights(1.0).await.unwrap();

        let decayed = store.get(&id).await.unwrap().unwrap();
        assert!((decayed.weight - 0.5).abs() < 0.01);
    }
}
