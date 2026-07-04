//! Embedding Provider 抽象 / embedding provider abstraction.
//!
//! 用于把文本转换为向量，供记忆系统的 Qdrant 向量检索使用。
//! M2 提供 OpenAI(`text-embedding-3-small`) 与 GLM(`embedding-3`) 实现。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use axon_core::{Error, Result};

/// Embedding provider trait.
///
/// 实现者负责把一批文本编码为稠密向量；返回顺序与输入顺序一致。
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Provider 标识 / provider identifier.
    fn id(&self) -> &str;

    /// 输出向量维度 / output vector dimension.
    fn dimension(&self) -> usize;

    /// 编码一批文本 / embed a batch of texts.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

/// 创建默认 embedding provider / create default embedding provider from env.
///
/// 优先读取 `EMBEDDING_PROVIDER`；支持 `openai` 与 `glm`。
/// 默认使用 OpenAI(`text-embedding-3-small`)，保持向后兼容。
pub fn create_embedding_provider_from_env() -> Result<Box<dyn EmbeddingProvider>> {
    match std::env::var("EMBEDDING_PROVIDER").as_deref() {
        Ok("openai") | Err(_) => Ok(Box::new(OpenAiEmbeddingProvider::from_env()?)),
        Ok("glm") => Ok(Box::new(GlmEmbeddingProvider::from_env()?)),
        Ok(other) => Err(Error::Config(format!(
            "unknown EMBEDDING_PROVIDER: {other}"
        ))),
    }
}

/// OpenAI embedding provider.
pub struct OpenAiEmbeddingProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
}

impl OpenAiEmbeddingProvider {
    /// 构造 OpenAI embedding provider.
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.into(),
            base_url: base_url.into(),
            model: model.into(),
        }
    }

    /// 从环境变量构造。
    ///
    /// 读取 `OPENAI_API_KEY`、`OPENAI_BASE_URL`、`OPENAI_EMBEDDING_MODEL`。
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| Error::Llm("missing OPENAI_API_KEY".into()))?;
        let base_url =
            std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into());
        let model = std::env::var("OPENAI_EMBEDDING_MODEL")
            .unwrap_or_else(|_| "text-embedding-3-small".into());
        Ok(Self::new(api_key, base_url, model))
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAiEmbeddingProvider {
    fn id(&self) -> &str {
        "openai-embedding"
    }

    fn dimension(&self) -> usize {
        match self.model.as_str() {
            "text-embedding-3-large" => 3_072,
            _ => 1_536,
        }
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        let url = format!("{}/embeddings", self.base_url.trim_end_matches('/'));
        let body = OpenAiEmbeddingRequest {
            model: self.model.clone(),
            input: texts.to_vec(),
        };

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Llm(format!("OpenAI embedding request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::Llm(format!(
                "OpenAI embedding HTTP {status}: {text}"
            )));
        }

        let data: OpenAiEmbeddingResponse = resp
            .json()
            .await
            .map_err(|e| Error::Llm(format!("OpenAI embedding parse failed: {e}")))?;

        Ok(data.data.into_iter().map(|d| d.embedding).collect())
    }
}

/// GLM(Zhipu AI) embedding provider.
///
/// 使用 OpenAI 兼容的 `/embeddings` 接口,默认模型为 `embedding-3`。
pub struct GlmEmbeddingProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
}

impl GlmEmbeddingProvider {
    /// 构造 GLM embedding provider。
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.into(),
            base_url: base_url.into(),
            model: model.into(),
        }
    }

    /// 从环境变量构造。
    ///
    /// 读取 `GLM_API_KEY`、`GLM_BASE_URL`、`GLM_EMBEDDING_MODEL`。
    pub fn from_env() -> Result<Self> {
        let api_key =
            std::env::var("GLM_API_KEY").map_err(|_| Error::Llm("missing GLM_API_KEY".into()))?;
        let base_url = std::env::var("GLM_BASE_URL")
            .unwrap_or_else(|_| "https://open.bigmodel.cn/api/paas/v4".into());
        let model = std::env::var("GLM_EMBEDDING_MODEL").unwrap_or_else(|_| "embedding-3".into());
        Ok(Self::new(api_key, base_url, model))
    }
}

#[async_trait]
impl EmbeddingProvider for GlmEmbeddingProvider {
    fn id(&self) -> &str {
        "glm-embedding"
    }

    fn dimension(&self) -> usize {
        // GLM embedding-3 输出 2048 维;若后续模型维度变化,在此扩展。
        match self.model.as_str() {
            "embedding-2" => 1_024,
            _ => 2_048,
        }
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        let url = format!("{}/embeddings", self.base_url.trim_end_matches('/'));
        let body = OpenAiEmbeddingRequest {
            model: self.model.clone(),
            input: texts.to_vec(),
        };

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Llm(format!("GLM embedding request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::Llm(format!("GLM embedding HTTP {status}: {text}")));
        }

        let data: OpenAiEmbeddingResponse = resp
            .json()
            .await
            .map_err(|e| Error::Llm(format!("GLM embedding parse failed: {e}")))?;

        Ok(data.data.into_iter().map(|d| d.embedding).collect())
    }
}

#[derive(Debug, Serialize)]
struct OpenAiEmbeddingRequest {
    model: String,
    input: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiEmbeddingResponse {
    data: Vec<OpenAiEmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct OpenAiEmbeddingData {
    embedding: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn mock_embedding_server(
        response_body: &'static str,
    ) -> (tokio::task::JoinHandle<()>, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base_url = format!("http://127.0.0.1:{port}");

        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _n = stream.read(&mut buf).await.unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });

        (handle, base_url)
    }

    /// 验证 OpenAI embedding 解析。
    #[tokio::test]
    async fn openai_embed_parses_response() {
        let body = r#"{"data": [{"embedding": [0.1, 0.2, 0.3]}]}"#;
        let (handle, base_url) = mock_embedding_server(body).await;
        let provider = OpenAiEmbeddingProvider::new(
            "sk-test",
            format!("{base_url}/v1"),
            "text-embedding-3-small",
        );
        let result = provider.embed(&["hello".into()]).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].len(), 3);
        let _ = handle.await;
    }

    /// 验证 GLM embedding 解析与默认维度。
    #[tokio::test]
    async fn glm_embed_parses_response() {
        let body = r#"{"data": [{"embedding": [0.1, 0.2, 0.3]}]}"#;
        let (handle, base_url) = mock_embedding_server(body).await;
        let provider =
            GlmEmbeddingProvider::new("sk-test", format!("{base_url}/v4"), "embedding-3");
        assert_eq!(provider.dimension(), 2048);
        let result = provider.embed(&["hello".into()]).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].len(), 3);
        let _ = handle.await;
    }
}
