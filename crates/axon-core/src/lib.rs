//! axon-core — axon.ai 框架的共享基础 / shared foundation for the axon.ai framework.
//!
//! 本 crate 提供所有子 crate 复用的基础类型:
//! - 统一的错误类型 [`Error`]
//! - 全局配置骨架 [`Config`]
//! - 任务/记忆等通用 ID 与标识类型
//!
//! 不依赖任何业务 crate,是依赖图的根。

use serde::{Deserialize, Serialize};

pub mod config;
pub mod error;

pub use config::Config;
pub use error::{Error, Result};

/// 通用标识符 / generic identifier (ULID/UUID 字符串)。
pub type Id = String;

/// 任务 ID / task identifier.
pub type TaskId = Id;

/// 记忆 ID / memory identifier.
pub type MemoryId = Id;

/// VM 句柄 ID / microVM handle identifier.
pub type VmId = Id;

/// 时间戳(Unix 毫秒)/ timestamp in Unix milliseconds.
pub type Timestamp = u64;

/// 标准化标识生成(占位)/ standardized id generation (placeholder).
/// TODO(M1): 替换为 ULID。
pub fn new_id() -> Id {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    format!("id-{ms}")
}

/// 框架全局版本号 / framework version string.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 通用的带时间戳的记录 / a timestamped record envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Timestamped<T> {
    pub id: Id,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub payload: T,
}
