//! OpenAI compatible LLM provider / OpenAI 兼容的 LLM provider.
//!
//! 通过 `reqwest` 调用 OpenAI `/v1/chat/completions` 接口。
//! 支持标准 OpenAI 以及任意 OpenAI 兼容端点(如兼容代理)。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    Capabilities, CompletionRequest, CompletionResponse, Delta, FinishReason, LlmProvider, Message,
    Role, Usage,
};

/// OpenAI API 请求体 / OpenAI chat completion request body.
#[derive(Debug, Serialize)]
struct OpenAiRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenAiTool>,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAiMessage {
    role: String,
    content: String,
    /// 推理内容 / reasoning content (DeepSeek-R1/V4 等推理模型返回)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
}

#[derive(Debug, Serialize)]
struct OpenAiTool {
    #[serde(rename = "type")]
    ty: String,
    function: OpenAiFunction,
}

#[derive(Debug, Serialize)]
struct OpenAiFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

/// OpenAI API 响应体 / OpenAI chat completion response body.
#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
    #[serde(default)]
    usage: OpenAiUsage,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct OpenAiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

/// OpenAI provider.
pub struct OpenAiProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
}

impl OpenAiProvider {
    /// 构造一个 OpenAI provider / construct an OpenAI provider.
    ///
    /// `base_url` 应为不含 `/v1/chat/completions` 的端点根路径，
    /// 例如 `https://api.openai.com/v1`。
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

    /// 从环境变量构造 / construct from environment variables.
    ///
    /// 读取:
    /// - `OPENAI_API_KEY` (必须)
    /// - `OPENAI_BASE_URL` (默认 `https://api.openai.com/v1`)
    /// - `OPENAI_MODEL` (默认 `gpt-4o-mini`)
    pub fn from_env() -> axon_core::Result<Self> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| axon_core::Error::Llm("missing OPENAI_API_KEY".into()))?;
        let base_url =
            std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into());
        let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());
        Ok(Self::new(api_key, base_url, model))
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    fn id(&self) -> &str {
        "openai"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            function_calling: true,
            vision: self.model.contains("vision") || self.model.starts_with("gpt-4o"),
            streaming: true,
            max_context_tokens: 128_000,
        }
    }

    async fn complete(&self, req: CompletionRequest) -> axon_core::Result<CompletionResponse> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let body = OpenAiRequest {
            model: if req.model.is_empty() {
                self.model.clone()
            } else {
                req.model
            },
            messages: req
                .messages
                .into_iter()
                .map(|m| OpenAiMessage {
                    role: role_to_string(m.role),
                    content: m.content,
                    reasoning_content: None,
                })
                .collect(),
            tools: req
                .tools
                .into_iter()
                .map(|t| OpenAiTool {
                    ty: "function".into(),
                    function: OpenAiFunction {
                        name: t.name,
                        description: t.description,
                        parameters: t.parameters,
                    },
                })
                .collect(),
            temperature: req.temperature,
            max_tokens: req.max_tokens,
        };

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| axon_core::Error::Llm(format!("OpenAI request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(axon_core::Error::Llm(format!(
                "OpenAI HTTP {status}: {text}"
            )));
        }

        let data: OpenAiResponse = resp
            .json()
            .await
            .map_err(|e| axon_core::Error::Llm(format!("OpenAI parse failed: {e}")))?;
        let choice = data
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| axon_core::Error::Llm("OpenAI returned no choices".into()))?;

        // 推理模型(如 DeepSeek)可能把回复放在 reasoning_content 中，
        // content 为空时回退到 reasoning_content。
        let content = if choice.message.content.is_empty() {
            choice.message.reasoning_content.unwrap_or_default()
        } else {
            choice.message.content
        };

        Ok(CompletionResponse {
            message: Message {
                role: Role::Assistant,
                content,
                tool_calls: None,
            },
            usage: Usage {
                prompt_tokens: data.usage.prompt_tokens,
                completion_tokens: data.usage.completion_tokens,
                total_tokens: data.usage.total_tokens,
            },
            finish_reason: parse_finish_reason(choice.finish_reason.as_deref()),
        })
    }

    async fn stream(&self, _req: CompletionRequest) -> axon_core::Result<Vec<Delta>> {
        // M1 先实现一次性补全；流式留待 M5。
        Err(axon_core::Error::Llm(
            "OpenAI streaming not implemented in M1".into(),
        ))
    }
}

fn role_to_string(role: Role) -> String {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
    .into()
}

fn parse_finish_reason(reason: Option<&str>) -> FinishReason {
    match reason {
        Some("stop") => FinishReason::Stop,
        Some("length") => FinishReason::Length,
        Some("tool_calls") => FinishReason::ToolCalls,
        Some("content_filter") => FinishReason::ContentFilter,
        Some(other) => FinishReason::Other(other.into()),
        None => FinishReason::Stop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 启动一个返回固定 OpenAI 响应的 mock HTTP server。
    async fn mock_openai_server(
        response_body: &'static str,
    ) -> (tokio::task::JoinHandle<()>, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base_url = format!("http://127.0.0.1:{port}/v1");

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

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// 验证 OpenAI provider 能正确构造请求并解析响应。
    #[tokio::test]
    async fn complete_parses_response() {
        let body = r#"{
            "choices": [{
                "message": {"role": "assistant", "content": "hello"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 3, "completion_tokens": 1, "total_tokens": 4}
        }"#;
        let (handle, base_url) = mock_openai_server(body).await;
        let provider = OpenAiProvider::new("sk-test", base_url, "gpt-4o-mini");
        let req = CompletionRequest {
            model: "gpt-4o-mini".into(),
            messages: vec![Message {
                role: Role::User,
                content: "hi".into(),
                tool_calls: None,
            }],
            tools: vec![],
            temperature: 0.0,
            max_tokens: None,
        };

        let resp = provider.complete(req).await.unwrap();
        assert_eq!(resp.message.content, "hello");
        assert_eq!(resp.finish_reason, FinishReason::Stop);
        assert_eq!(resp.usage.total_tokens, 4);
        let _ = handle.await;
    }

    /// 验证 HTTP 错误会被包装成 Error::Llm。
    #[tokio::test]
    async fn complete_propagates_http_error() {
        let body = r#"{"error": "invalid key"}"#;
        let (handle, base_url) = mock_openai_server(body).await;
        let provider = OpenAiProvider::new("sk-bad", base_url, "gpt-4o-mini");
        let req = CompletionRequest {
            model: "gpt-4o-mini".into(),
            messages: vec![Message {
                role: Role::User,
                content: "hi".into(),
                tool_calls: None,
            }],
            tools: vec![],
            temperature: 0.0,
            max_tokens: None,
        };

        let err = provider.complete(req).await.unwrap_err();
        assert!(matches!(err, axon_core::Error::Llm(_)));
        let _ = handle.await;
    }
}
