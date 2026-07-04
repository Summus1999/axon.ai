//! DeepSeek LLM provider / DeepSeek 大模型 provider.
//!
//! DeepSeek API 与 OpenAI 兼容，本实现复用 [`OpenAiProvider`] 的协议逻辑，
//! 仅固定 base URL、模型与 provider id。

use async_trait::async_trait;

use crate::{
    Capabilities, CompletionRequest, CompletionResponse, Delta, LlmProvider, OpenAiProvider,
};

/// DeepSeek provider.
///
/// 默认调用 `https://api.deepseek.com/v1/chat/completions`，
/// 默认模型 `deepseek-v4-pro`。
pub struct DeepSeekProvider {
    inner: OpenAiProvider,
}

impl DeepSeekProvider {
    /// 构造 DeepSeek provider / construct a DeepSeek provider.
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            inner: OpenAiProvider::new(api_key, "https://api.deepseek.com/v1", model),
        }
    }

    /// 从环境变量构造 / construct from environment variables.
    ///
    /// 读取:
    /// - `DEEPSEEK_API_KEY` (必须)
    /// - `DEEPSEEK_BASE_URL` (默认 `https://api.deepseek.com/v1`)
    /// - `DEEPSEEK_MODEL` (默认 `deepseek-v4-pro`)
    pub fn from_env() -> axon_core::Result<Self> {
        let api_key = std::env::var("DEEPSEEK_API_KEY")
            .map_err(|_| axon_core::Error::Llm("missing DEEPSEEK_API_KEY".into()))?;
        let base_url = std::env::var("DEEPSEEK_BASE_URL")
            .unwrap_or_else(|_| "https://api.deepseek.com/v1".into());
        let model = std::env::var("DEEPSEEK_MODEL").unwrap_or_else(|_| "deepseek-v4-pro".into());
        Ok(Self {
            inner: OpenAiProvider::new(api_key, base_url, model),
        })
    }
}

#[async_trait]
impl LlmProvider for DeepSeekProvider {
    fn id(&self) -> &str {
        "deepseek"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            function_calling: true,
            vision: false,
            streaming: true,
            max_context_tokens: 128_000,
        }
    }

    async fn complete(&self, req: CompletionRequest) -> axon_core::Result<CompletionResponse> {
        self.inner.complete(req).await
    }

    async fn stream(&self, req: CompletionRequest) -> axon_core::Result<Vec<Delta>> {
        self.inner.stream(req).await
    }
}
