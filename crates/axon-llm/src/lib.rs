//! axon-llm — LLM Provider 抽象层 / LLM provider abstraction layer.
//!
//! 定义统一的 [`LlmProvider`] trait,屏蔽 OpenAI / Anthropic / Ollama 等
//! 后端差异,支持按任务类型路由模型与流式输出。
//!
//! 具体实现(openai/anthropic/ollama)留待 M1。

#![allow(dead_code)]

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use axon_core::{Error, Result};

/// 消息角色 / message role.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// 一条对话消息 / a single chat message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    /// 工具调用(可选)/ optional tool calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

/// 工具调用请求 / a tool-call request from the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String, // JSON string
}

/// 工具定义 / a tool/function definition exposed to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    /// JSON Schema 描述的参数 / parameters as JSON Schema.
    pub parameters: serde_json::Value,
}

/// 模型能力声明 / declares what a provider/model can do.
#[derive(Debug, Clone, Default)]
pub struct Capabilities {
    pub function_calling: bool,
    pub vision: bool,
    pub streaming: bool,
    pub max_context_tokens: usize,
}

/// 补全请求 / a completion request.
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<Tool>,
    pub temperature: f32,
    pub max_tokens: Option<u32>,
}

/// 补全响应 / a completion response.
#[derive(Debug, Clone)]
pub struct CompletionResponse {
    pub message: Message,
    pub usage: Usage,
    pub finish_reason: FinishReason,
}

/// 流式增量 / a streaming delta chunk.
#[derive(Debug, Clone)]
pub struct Delta {
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    Other(String),
}

/// LLM Provider 抽象 / the unified LLM provider trait.
///
/// 所有后端(OpenAI / Anthropic / Ollama ...)实现此 trait。
/// 路由层(`LlmRouter`,M1)按 task type 选择具体 provider。
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Provider 标识 / provider identifier (e.g. "openai").
    fn id(&self) -> &str;

    /// 能力声明 / capability declaration.
    fn capabilities(&self) -> Capabilities;

    /// 一次性补全 / one-shot completion.
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse>;

    /// 流式补全 / streaming completion.
    ///
    /// TODO(M1): 返回 `impl Stream`;骨架先用占位签名。
    async fn stream(&self, req: CompletionRequest) -> Result<Vec<Delta>> {
        // 默认实现退化为单次调用 + 单 delta,具体 provider 覆盖。
        let resp = self.complete(req).await?;
        Ok(vec![Delta {
            content: Some(resp.message.content),
            tool_calls: resp.message.tool_calls,
        }])
    }
}

/// 占位:未实现的 provider / placeholder provider that always errors.
pub struct UnimplementedProvider {
    pub id: String,
}

#[async_trait]
impl LlmProvider for UnimplementedProvider {
    fn id(&self) -> &str {
        &self.id
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse> {
        Err(Error::Llm(format!(
            "provider `{}` not yet implemented (skeleton)",
            self.id
        )))
    }
}
