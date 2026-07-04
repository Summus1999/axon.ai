//! 全局配置骨架 / global configuration skeleton.
//!
//! 一期支持从 TOML 文件 + 环境变量加载;figment 集成留待 M1。

use serde::{Deserialize, Serialize};

/// 框架根配置 / root configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// 运行 profile: `dev` | `test` | `prod`。
    #[serde(default = "default_profile")]
    pub profile: String,

    /// LLM 相关配置。
    #[serde(default)]
    pub llm: LlmConfig,

    /// 记忆相关配置。
    #[serde(default)]
    pub memory: MemoryConfig,

    /// 隔离执行相关配置。
    #[serde(default)]
    pub isolation: IsolationConfig,

    /// 调度器相关配置。
    #[serde(default)]
    pub dispatcher: DispatcherConfig,
}

fn default_profile() -> String {
    "dev".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlmConfig {
    /// 默认 provider: `openai` | `anthropic` | `ollama`。
    pub default_provider: Option<String>,
    /// 默认模型名。
    pub default_model: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Qdrant 服务地址(留空则用嵌入式)。
    pub qdrant_url: Option<String>,
    /// 本地 KV 数据库路径。
    pub kv_path: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IsolationConfig {
    /// 隔离后端: `docker` | `firecracker`。
    #[serde(default = "default_isolation_backend")]
    pub backend: String,
}

fn default_isolation_backend() -> String {
    "docker".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DispatcherConfig {
    /// 最大并发 VM 数。
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,
}

fn default_max_concurrency() -> usize {
    4
}

impl Config {
    /// 占位加载:从 TOML 字符串解析 / parse from a TOML string (placeholder).
    /// TODO(M1): 接入 figment 实现 toml+env 多源合并。
    pub fn load_from_toml_str(s: &str) -> crate::Result<Self> {
        toml::from_str(s).map_err(|e| crate::Error::Config(e.to_string()))
    }
}
