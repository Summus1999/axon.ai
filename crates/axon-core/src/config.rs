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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// 记忆后端: `memory` | `hybrid`,默认 `memory`。
    #[serde(default = "default_memory_backend")]
    pub backend: String,
    /// Qdrant 服务地址(留空则用嵌入式)。
    pub qdrant_url: Option<String>,
    /// 本地 KV 数据库路径。
    pub kv_path: Option<String>,
    /// Qdrant collection 名称,默认 `axon_memories`。
    #[serde(default = "default_qdrant_collection")]
    pub qdrant_collection: String,
}

fn default_memory_backend() -> String {
    "memory".to_string()
}

fn default_qdrant_collection() -> String {
    "axon_memories".to_string()
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            backend: default_memory_backend(),
            qdrant_url: None,
            kv_path: None,
            qdrant_collection: default_qdrant_collection(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IsolationConfig {
    /// 隔离后端: `docker` | `firecracker`。
    #[serde(default = "default_isolation_backend")]
    pub backend: String,
    /// Firecracker 可执行文件路径 / path to the firecracker binary.
    #[serde(default)]
    pub firecracker_binary: Option<String>,
    /// Firecracker jailer 可执行文件路径(可选)/ path to the jailer binary.
    #[serde(default)]
    pub jailer_binary: Option<String>,
    /// 默认内核镜像(vmlinux)路径 / path to the default kernel image.
    #[serde(default)]
    pub kernel_image: Option<String>,
    /// 默认 rootfs 镜像路径 / path to the default rootfs image.
    #[serde(default)]
    pub rootfs_image: Option<String>,
    /// 快照存储目录 / directory for VM snapshots.
    #[serde(default)]
    pub snapshot_dir: Option<String>,
}

fn default_isolation_backend() -> String {
    "docker".to_string()
}

impl Default for IsolationConfig {
    fn default() -> Self {
        Self {
            backend: default_isolation_backend(),
            firecracker_binary: None,
            jailer_binary: None,
            kernel_image: None,
            rootfs_image: None,
            snapshot_dir: None,
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

    /// MemoryConfig 默认值符合预期。
    #[test]
    fn memory_config_defaults() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_axon_env();

        let cfg = Config::load_with_paths("/nonexistent.toml", "/nonexistent.env").unwrap();
        assert_eq!(cfg.memory.backend, "memory");
        assert_eq!(cfg.memory.qdrant_collection, "axon_memories");
        assert!(cfg.memory.qdrant_url.is_none());
        assert!(cfg.memory.kv_path.is_none());
    }

    /// 环境变量覆盖 memory 配置。
    #[test]
    fn env_overrides_memory_config() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_axon_env();

        std::env::set_var("AXON_MEMORY__BACKEND", "hybrid");
        std::env::set_var("AXON_MEMORY__QDRANT_URL", "http://localhost:6334");
        std::env::set_var("AXON_MEMORY__KV_PATH", ".axon/memory.redb");
        std::env::set_var("AXON_MEMORY__QDRANT_COLLECTION", "test_memories");

        let cfg = Config::load_with_paths("/nonexistent.toml", "/nonexistent.env").unwrap();
        assert_eq!(cfg.memory.backend, "hybrid");
        assert_eq!(
            cfg.memory.qdrant_url.as_deref(),
            Some("http://localhost:6334")
        );
        assert_eq!(cfg.memory.kv_path.as_deref(), Some(".axon/memory.redb"));
        assert_eq!(cfg.memory.qdrant_collection, "test_memories");

        std::env::remove_var("AXON_MEMORY__BACKEND");
        std::env::remove_var("AXON_MEMORY__QDRANT_URL");
        std::env::remove_var("AXON_MEMORY__KV_PATH");
        std::env::remove_var("AXON_MEMORY__QDRANT_COLLECTION");
    }

    /// `IsolationConfig` 新增 Firecracker 相关字段默认均为 None。
    #[test]
    fn isolation_config_defaults_for_firecracker() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_axon_env();

        let cfg = Config::load_with_paths("/nonexistent.toml", "/nonexistent.env").unwrap();
        assert_eq!(cfg.isolation.backend, "docker");
        assert!(cfg.isolation.firecracker_binary.is_none());
        assert!(cfg.isolation.jailer_binary.is_none());
        assert!(cfg.isolation.kernel_image.is_none());
        assert!(cfg.isolation.rootfs_image.is_none());
        assert!(cfg.isolation.snapshot_dir.is_none());
    }

    /// TOML 可覆盖 Firecracker 相关路径配置。
    #[test]
    fn toml_overrides_firecracker_paths() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_axon_env();

        let toml = temp_file(
            "firecracker",
            r#"
[isolation]
backend = "firecracker"
firecracker_binary = "/usr/bin/firecracker"
jailer_binary = "/usr/bin/jailer"
kernel_image = "/var/lib/axon/vmlinux"
rootfs_image = "/var/lib/axon/rootfs.ext4"
snapshot_dir = "/var/lib/axon/snapshots"
"#,
        );

        let cfg = Config::load_with_paths(&toml, "/nonexistent.env").unwrap();
        assert_eq!(cfg.isolation.backend, "firecracker");
        assert_eq!(
            cfg.isolation.firecracker_binary.as_deref(),
            Some("/usr/bin/firecracker")
        );
        assert_eq!(
            cfg.isolation.jailer_binary.as_deref(),
            Some("/usr/bin/jailer")
        );
        assert_eq!(
            cfg.isolation.kernel_image.as_deref(),
            Some("/var/lib/axon/vmlinux")
        );
        assert_eq!(
            cfg.isolation.rootfs_image.as_deref(),
            Some("/var/lib/axon/rootfs.ext4")
        );
        assert_eq!(
            cfg.isolation.snapshot_dir.as_deref(),
            Some("/var/lib/axon/snapshots")
        );
    }

    /// 环境变量可覆盖 Firecracker 相关路径配置。
    #[test]
    fn env_overrides_firecracker_paths() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_axon_env();

        let toml = temp_file(
            "firecracker",
            r#"
[isolation]
backend = "docker"
firecracker_binary = "/usr/bin/firecracker"
"#,
        );
        std::env::set_var("AXON_ISOLATION__BACKEND", "firecracker");
        std::env::set_var("AXON_ISOLATION__KERNEL_IMAGE", "/opt/axon/vmlinux");
        std::env::set_var("AXON_ISOLATION__ROOTFS_IMAGE", "/opt/axon/rootfs.ext4");

        let cfg = Config::load_with_paths(&toml, "/nonexistent.env").unwrap();
        assert_eq!(cfg.isolation.backend, "firecracker");
        assert_eq!(
            cfg.isolation.firecracker_binary.as_deref(),
            Some("/usr/bin/firecracker")
        );
        assert_eq!(
            cfg.isolation.kernel_image.as_deref(),
            Some("/opt/axon/vmlinux")
        );
        assert_eq!(
            cfg.isolation.rootfs_image.as_deref(),
            Some("/opt/axon/rootfs.ext4")
        );

        std::env::remove_var("AXON_ISOLATION__BACKEND");
        std::env::remove_var("AXON_ISOLATION__KERNEL_IMAGE");
        std::env::remove_var("AXON_ISOLATION__ROOTFS_IMAGE");
    }
}
