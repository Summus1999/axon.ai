//! axon-memory 跨会话持久化集成测试 / cross-session persistence integration tests.
//!
//! 这些测试需要本地 Docker 运行 Qdrant,默认 `#[ignore]`;
//! 运行前执行:
//!   docker compose up -d qdrant
//! 或
//!   cargo test -p axon-memory --test memory_e2e -- --ignored

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use testcontainers::core::IntoContainerPort;
use testcontainers::runners::AsyncRunner;
use testcontainers::GenericImage;

use axon_llm::EmbeddingProvider;
use axon_memory::{
    HybridMemoryStore, Memory, MemoryKind, MemoryStore, QdrantStore, RecallQuery, RedbStore,
};

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
    async fn embed(&self, texts: &[String]) -> axon_core::Result<Vec<Vec<f32>>> {
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

fn temp_redb_path() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("axon-memory-e2e-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("memory.redb")
}

fn sample_user_profile() -> Memory {
    Memory {
        id: String::new(),
        kind: MemoryKind::UserProfile,
        content: "user prefers anyhow for binaries".into(),
        embedding: None,
        weight: 1.2,
        created_at: 0,
        updated_at: 0,
        source: Some("e2e".into()),
    }
}

/// 跨会话持久化:关闭 store 后重新打开同一 redb/Qdrant,仍能 recall 记忆。
#[tokio::test]
#[ignore = "requires Docker with Qdrant image"]
async fn cross_session_recall_persists() {
    let redb_path = temp_redb_path();

    let container = GenericImage::new("qdrant/qdrant", "v1.13.4")
        .with_exposed_port(6334.tcp())
        .start()
        .await
        .expect("qdrant container started");
    let port = container
        .get_host_port_ipv4(6334.tcp())
        .await
        .expect("port exposed");
    let qdrant_url = format!("http://127.0.0.1:{port}");

    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder {
        dim: 8,
        counter: AtomicUsize::new(1),
    });

    // 会话 A:写入 UserProfile。
    let semantic_a = RedbStore::new(&redb_path).unwrap();
    let episodic_a = QdrantStore::new(&qdrant_url, "e2e_memories", embedder.clone())
        .await
        .unwrap();
    let store_a = HybridMemoryStore::new(semantic_a, episodic_a);

    let id = store_a.store(sample_user_profile()).await.unwrap();

    // 会话 B:重新打开同一路径,验证 recall。
    let semantic_b = RedbStore::new(&redb_path).unwrap();
    let episodic_b = QdrantStore::new(&qdrant_url, "e2e_memories", embedder)
        .await
        .unwrap();
    let store_b = HybridMemoryStore::new(semantic_b, episodic_b);

    let recalled = store_b
        .recall(&RecallQuery {
            query: "anyhow".into(),
            kind: Some(MemoryKind::UserProfile),
            top_k: 5,
        })
        .await
        .unwrap();

    assert_eq!(recalled.len(), 1);
    assert_eq!(recalled[0].id, id);
    assert_eq!(recalled[0].content, "user prefers anyhow for binaries");

    // 清理:遗忘后确认消失。
    store_b.forget(&id).await.unwrap();
    let after_forget = store_b
        .recall(&RecallQuery {
            query: "anyhow".into(),
            kind: Some(MemoryKind::UserProfile),
            top_k: 5,
        })
        .await
        .unwrap();
    assert!(after_forget.is_empty());
}
