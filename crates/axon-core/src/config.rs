//! 全局配置 / global configuration.
//!
//! 支持从以下三源合并加载，优先级从高到低：
//! 1. 以 `AXON_` 为前缀的环境变量
//! 2. `.env` 文件
//! 3. `axon.toml` 文件
//! 4. 结构体 `Default`
//!
//! 环境变量通过 figment 的 `Env` provider 解析，使用 `__`（双下划线）作为嵌套
//! 分隔符。例如 `AXON_LLM__DEFAULT_PROVIDER` 会映射到 `llm.default_provider`，
//! `AXON_PROFILE` 会映射到顶层 `profile`。

use std::path::Path;

use figment::providers::{Env, Format, Toml};
use figment::Figment;
use serde::{Deserialize, Serialize};

/// 框架根配置 / root configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

impl Default for Config {
    fn default() -> Self {
        Self {
            profile: default_profile(),
            llm: LlmConfig::default(),
            memory: MemoryConfig::default(),
            isolation: IsolationConfig::default(),
            dispatcher: DispatcherConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlmConfig {
    /// 默认 provider: `openai` | `deepseek`。
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IsolationConfig {
    /// 隔离后端: `docker` | `firecracker`。
    #[serde(default = "default_isolation_backend")]
    pub backend: String,
}

fn default_isolation_backend() -> String {
    "docker".to_string()
}

impl Default for IsolationConfig {
    fn default() -> Self {
        Self {
            backend: default_isolation_backend(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatcherConfig {
    /// 最大并发 VM 数。
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,
}

fn default_max_concurrency() -> usize {
    4
}

impl Default for DispatcherConfig {
    fn default() -> Self {
        Self {
            max_concurrency: default_max_concurrency(),
        }
    }
}

impl Config {
    /// 从默认路径加载配置：`.env` + `axon.toml` + `AXON_` 环境变量。
    ///
    /// 合并优先级：`env > .env > axon.toml > default`。
    pub fn load() -> crate::Result<Self> {
        Self::load_with_paths("axon.toml", ".env")
    }

    /// 从指定路径加载配置，便于测试与 CLI 自定义路径。
    ///
    /// `toml_path` 为 TOML 配置文件；`dotenv_path` 为 dotenv 文件。
    /// 任一文件不存在时会被忽略，但文件存在却解析失败会返回错误。
    pub fn load_with_paths(
        toml_path: impl AsRef<Path>,
        dotenv_path: impl AsRef<Path>,
    ) -> crate::Result<Self> {
        let dotenv_path = dotenv_path.as_ref();
        if dotenv_path.is_file() {
            dotenvy::from_filename(dotenv_path).map_err(|e| {
                crate::Error::Config(format!(
                    "failed to load .env '{}': {e}",
                    dotenv_path.display()
                ))
            })?;
        }

        let toml_path = toml_path.as_ref();
        let mut figment = Figment::new();
        if toml_path.is_file() {
            figment = figment.merge(Toml::file(toml_path));
        }

        figment
            .merge(Env::prefixed("AXON_").split("__"))
            .extract()
            .map_err(|e| crate::Error::Config(e.to_string()))
    }

    /// 从 TOML 字符串解析 / parse from a TOML string.
    pub fn load_from_toml_str(s: &str) -> crate::Result<Self> {
        toml::from_str(s).map_err(|e| crate::Error::Config(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;
    use std::fs;
    use std::io::Write;
    use std::sync::Mutex;

    /// 串行化所有会修改环境变量的测试。
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// 清理所有以 `AXON_` 开头的环境变量，避免测试间污染。
    fn clear_axon_env() {
        let keys: Vec<String> = std::env::vars()
            .filter(|(k, _)| k.starts_with("AXON_"))
            .map(|(k, _)| k)
            .collect();
        for k in &keys {
            std::env::remove_var(k);
        }
    }

    /// 在临时目录创建包含 `content` 的临时文件，返回其路径。
    fn temp_file(name: &str, content: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "axon-config-test-{}-{}",
            name,
            uuid::Uuid::new_v4()
        ));
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        path
    }

    /// 当没有任何外部源时，Config 使用默认值。
    #[test]
    fn defaults_when_no_sources() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_axon_env();

        let cfg = Config::load_with_paths("/nonexistent.toml", "/nonexistent.env").unwrap();
        assert_eq!(cfg.profile, "dev");
        assert_eq!(cfg.isolation.backend, "docker");
        assert_eq!(cfg.dispatcher.max_concurrency, 4);
        assert!(cfg.llm.default_provider.is_none());
    }

    /// TOML 文件覆盖默认值。
    #[test]
    fn toml_overrides_defaults() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_axon_env();

        let toml = temp_file(
            "toml",
            r#"
profile = "prod"

[isolation]
backend = "firecracker"

[dispatcher]
max_concurrency = 8
"#,
        );

        let cfg = Config::load_with_paths(&toml, "/nonexistent.env").unwrap();
        assert_eq!(cfg.profile, "prod");
        assert_eq!(cfg.isolation.backend, "firecracker");
        assert_eq!(cfg.dispatcher.max_concurrency, 8);
    }

    /// 环境变量覆盖 TOML。
    #[test]
    fn env_overrides_toml() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_axon_env();

        let toml = temp_file(
            "toml",
            r#"
[isolation]
backend = "firecracker"
"#,
        );
        std::env::set_var("AXON_ISOLATION__BACKEND", "docker");

        let cfg = Config::load_with_paths(&toml, "/nonexistent.env").unwrap();
        assert_eq!(cfg.isolation.backend, "docker");

        std::env::remove_var("AXON_ISOLATION__BACKEND");
    }

    /// `.env` 文件覆盖 TOML。
    #[test]
    fn dotenv_overrides_toml() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_axon_env();

        let toml = temp_file(
            "toml",
            r#"
[llm]
default_provider = "openai"
"#,
        );
        let dotenv = temp_file("env", "AXON_LLM__DEFAULT_PROVIDER=deepseek\n");

        let cfg = Config::load_with_paths(&toml, &dotenv).unwrap();
        assert_eq!(cfg.llm.default_provider.as_deref(), Some("deepseek"));
    }

    /// 真实环境变量覆盖 `.env` 文件。
    #[test]
    fn env_overrides_dotenv() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_axon_env();

        let toml = temp_file(
            "toml",
            r#"
[llm]
default_provider = "openai"
"#,
        );
        let dotenv = temp_file("env", "AXON_LLM__DEFAULT_PROVIDER=deepseek\n");
        std::env::set_var("AXON_LLM__DEFAULT_PROVIDER", "openai");

        let cfg = Config::load_with_paths(&toml, &dotenv).unwrap();
        assert_eq!(cfg.llm.default_provider.as_deref(), Some("openai"));

        std::env::remove_var("AXON_LLM__DEFAULT_PROVIDER");
    }

    /// 非法 TOML 返回 Config 错误。
    #[test]
    fn invalid_toml_returns_error() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_axon_env();

        let toml = temp_file("toml", "this is not valid toml [");
        let result = Config::load_with_paths(&toml, "/nonexistent.env");
        assert!(matches!(result, Err(Error::Config(_))));
    }
}
