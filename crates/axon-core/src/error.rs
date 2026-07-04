//! 统一错误类型 / unified error types for the axon.ai framework.

use thiserror::Error;

/// 框架级错误 / top-level framework error.
///
/// 各子 crate 可定义自己的更具体错误并通过 `#[from]` 转换到此类型。
#[derive(Debug, Error)]
pub enum Error {
    #[error("LLM provider error: {0}")]
    Llm(String),

    #[error("memory store error: {0}")]
    Memory(String),

    #[error("isolation / VM error: {0}")]
    Isolation(String),

    #[error("dispatcher / scheduling error: {0}")]
    Dispatcher(String),

    #[error("task rejected: {0}")]
    TaskRejected(String),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;
