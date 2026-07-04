//! Qdrant 向量记忆存储 / Qdrant-backed vector memory store.
//!
//! 负责情景记忆 (Episodic) 的向量存储与语义检索。
//! M2 使用 `qdrant-client` 通过 gRPC/REST 与 Qdrant 交互。

use std::sync::Arc;

use async_trait::async_trait;
use qdrant_client::qdrant::{
    CollectionExistsRequest, CreateCollectionBuilder, DeletePointsBuilder, Distance,
    GetPointsBuilder, PointId, PointStruct, ScrollPointsBuilder, SearchPointsBuilder,
    UpsertPointsBuilder, Value as QdrantValue, VectorParamsBuilder, VectorsConfig,
};
use qdrant_client::Qdrant;

use axon_core::{MemoryId, Result};
use axon_llm::EmbeddingProvider;

use crate::{Memory, MemoryFilter, MemoryStore, RecallQuery, DEFAULT_WEIGHT};

/// Qdrant 向量记忆存储 / Qdrant vector memory store.
pub struct QdrantStore {
    client: Qdrant,
    collection: String,
    embedder: Arc<dyn EmbeddingProvider>,
}

impl std::fmt::Debug for QdrantStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QdrantStore")
            .field("collection", &self.collection)
            .field("embedder", &self.embedder.id())
            .finish_non_exhaustive()
    }
}

impl QdrantStore {
    /// 创建或打开 Qdrant 集合 / create or open a Qdrant collection.
    ///
    /// `url` 例如 `http://localhost:6334`；`collection` 为集合名。
    pub async fn new(
        url: impl Into<String>,
        collection: impl Into<String>,
        embedder: Arc<dyn EmbeddingProvider>,
    ) -> Result<Self> {
        let url = url.into();
        let collection = collection.into();
        let client = Qdrant::from_url(&url)
            .build()
            .map_err(|e| axon_core::Error::Memory(format!("failed to build Qdrant client: {e}")))?;

        let exists = client
            .collection_exists(CollectionExistsRequest {
                collection_name: collection.clone(),
            })
            .await
            .map_err(qdrant_error)?;

        if !exists {
            let vectors_config =
                VectorParamsBuilder::new(embedder.dimension() as u64, Distance::Cosine);
            client
                .create_collection(
                    CreateCollectionBuilder::new(&collection)
                        .vectors_config(VectorsConfig::from(vectors_config)),
                )
                .await
                .map_err(qdrant_error)?;
        }

        Ok(Self {
            client,
            collection,
            embedder,
        })
    }
}

#[async_trait]
impl MemoryStore for QdrantStore {
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

        let embedding = if let Some(ref emb) = mem.embedding {
            emb.clone()
        } else {
            self.embedder
                .embed(&[mem.content.clone()])
                .await?
                .into_iter()
                .next()
                .unwrap_or_default()
        };

        let payload = serde_json::to_value(&mem).map_err(axon_core::Error::Json)?;
        let point = PointStruct::new(mem.id.clone(), embedding, payload_to_qdrant_map(payload));

        self.client
            .upsert_points(UpsertPointsBuilder::new(&self.collection, vec![point]).wait(true))
            .await
            .map_err(qdrant_error)?;

        Ok(mem.id)
    }

    async fn recall(&self, query: &RecallQuery) -> Result<Vec<Memory>> {
        let embeddings = self
            .embedder
            .embed(std::slice::from_ref(&query.query))
            .await?;
        let vector = embeddings.into_iter().next().unwrap_or_default();

        let search = SearchPointsBuilder::new(&self.collection, vector, query.top_k as u64)
            .with_payload(true);

        let response = self
            .client
            .search_points(search)
            .await
            .map_err(qdrant_error)?;

        let mut memories = Vec::new();
        for scored_point in response.result {
            let mem = parse_memory_payload(scored_point.payload)?;
            memories.push(mem);
        }
        Ok(memories)
    }

    async fn list(&self, filter: &MemoryFilter) -> Result<Vec<Memory>> {
        let scroll = self
            .client
            .scroll(
                ScrollPointsBuilder::new(&self.collection)
                    .with_payload(true)
                    .limit(10_000),
            )
            .await
            .map_err(qdrant_error)?;

        let mut memories = Vec::new();
        for point in scroll.result {
            let mem = parse_memory_payload(point.payload)?;
            if memory_matches_filter(&mem, filter) {
                memories.push(mem);
            }
        }
        Ok(memories)
    }

    async fn get(&self, id: &MemoryId) -> Result<Option<Memory>> {
        let points = self
            .client
            .get_points(
                GetPointsBuilder::new(&self.collection, vec![PointId::from(id.as_str())])
                    .with_payload(true),
            )
            .await
            .map_err(qdrant_error)?;

        if let Some(point) = points.result.into_iter().next() {
            let mem = parse_memory_payload(point.payload)?;
            return Ok(Some(mem));
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
        self.store(mem).await.map(|_| ())
    }

    async fn forget(&self, id: &MemoryId) -> Result<()> {
        self.client
            .delete_points(
                DeletePointsBuilder::new(&self.collection).points(vec![PointId::from(id.as_str())]),
            )
            .await
            .map_err(qdrant_error)?;
        Ok(())
    }
}

fn qdrant_error(e: qdrant_client::QdrantError) -> axon_core::Error {
    axon_core::Error::Memory(format!("qdrant error: {e}"))
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

fn parse_memory_payload(payload: std::collections::HashMap<String, QdrantValue>) -> Result<Memory> {
    let json = serde_json::Value::Object(
        payload
            .into_iter()
            .map(|(k, v)| (k, qdrant_value_to_json(v)))
            .collect(),
    );
    serde_json::from_value(json).map_err(axon_core::Error::Json)
}

fn payload_to_qdrant_map(
    payload: serde_json::Value,
) -> std::collections::HashMap<String, QdrantValue> {
    let mut map = std::collections::HashMap::new();
    if let serde_json::Value::Object(obj) = payload {
        for (k, v) in obj {
            map.insert(k, json_to_qdrant_value(v));
        }
    }
    map
}

fn json_to_qdrant_value(value: serde_json::Value) -> QdrantValue {
    use qdrant_client::qdrant::value::Kind;
    use qdrant_client::qdrant::{ListValue, Struct};
    let kind = match value {
        serde_json::Value::Null => Kind::NullValue(0),
        serde_json::Value::Bool(b) => Kind::BoolValue(b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Kind::IntegerValue(i)
            } else if let Some(f) = n.as_f64() {
                Kind::DoubleValue(f)
            } else {
                Kind::StringValue(n.to_string())
            }
        }
        serde_json::Value::String(s) => Kind::StringValue(s),
        serde_json::Value::Array(arr) => Kind::ListValue(ListValue {
            values: arr.into_iter().map(json_to_qdrant_value).collect(),
        }),
        serde_json::Value::Object(obj) => Kind::StructValue(Struct {
            fields: obj
                .into_iter()
                .map(|(k, v)| (k, json_to_qdrant_value(v)))
                .collect(),
        }),
    };
    QdrantValue { kind: Some(kind) }
}

fn qdrant_value_to_json(value: QdrantValue) -> serde_json::Value {
    use qdrant_client::qdrant::value::Kind;
    match value.kind {
        Some(Kind::NullValue(_)) => serde_json::Value::Null,
        Some(Kind::BoolValue(b)) => serde_json::Value::Bool(b),
        Some(Kind::IntegerValue(i)) => serde_json::Value::Number(i.into()),
        Some(Kind::DoubleValue(f)) => serde_json::Value::Number(
            serde_json::Number::from_f64(f).unwrap_or_else(|| serde_json::Number::from(0)),
        ),
        Some(Kind::StringValue(s)) => serde_json::Value::String(s),
        Some(Kind::ListValue(l)) => {
            serde_json::Value::Array(l.values.into_iter().map(qdrant_value_to_json).collect())
        }
        Some(Kind::StructValue(s)) => serde_json::Value::Object(
            s.fields
                .into_iter()
                .map(|(k, v)| (k, qdrant_value_to_json(v)))
                .collect(),
        ),
        None => serde_json::Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use testcontainers::core::IntoContainerPort;
    use testcontainers::runners::AsyncRunner;
    use testcontainers::GenericImage;

    use super::*;
    use crate::MemoryKind;

    struct MockEmbedder {
        dim: usize,
        counter: AtomicUsize,
    }

    #[async_trait]
    impl EmbeddingProvider for MockEmbedder {
        fn id(&self) -> &str {
            "mock"
        }
        fn dimension(&self) -> usize {
            self.dim
        }
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            let n = self.counter.fetch_add(texts.len(), Ordering::SeqCst);
            Ok(texts
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    let mut v = vec![0.0f32; self.dim];
                    v[0] = ((n + i) as f32) * 0.1;
                    v
                })
                .collect())
        }
    }

    fn sample_memory(content: &str) -> Memory {
        Memory {
            id: String::new(),
            kind: MemoryKind::Episodic,
            content: content.into(),
            embedding: None,
            weight: 1.0,
            created_at: 0,
            updated_at: 0,
            source: Some("test".into()),
        }
    }

    /// Qdrant 端到端集成测试。需要本地 Docker，默认忽略。
    #[tokio::test]
    #[ignore = "requires Docker with Qdrant image"]
    async fn qdrant_store_roundtrip() {
        let container = GenericImage::new("qdrant/qdrant", "v1.13.4")
            .with_exposed_port(6334.tcp())
            .start()
            .await
            .expect("qdrant container started");
        let port = container
            .get_host_port_ipv4(6334.tcp())
            .await
            .expect("port exposed");
        let url = format!("http://127.0.0.1:{port}");

        let embedder = Arc::new(MockEmbedder {
            dim: 8,
            counter: AtomicUsize::new(1),
        });
        let store = QdrantStore::new(url, "test_memories", embedder)
            .await
            .expect("store created");

        let id = store
            .store(sample_memory("user prefers rust"))
            .await
            .unwrap();
        let mem = store.get(&id).await.unwrap().unwrap();
        assert_eq!(mem.content, "user prefers rust");

        let recalled = store
            .recall(&RecallQuery {
                query: "rust preference".into(),
                kind: None,
                top_k: 5,
            })
            .await
            .unwrap();
        assert_eq!(recalled.len(), 1);

        store.forget(&id).await.unwrap();
        assert!(store.get(&id).await.unwrap().is_none());
    }
}
