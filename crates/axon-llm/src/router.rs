//! LLM Provider 路由器 / LLM provider router.
//!
//! 根据配置或环境变量选择具体 provider，屏蔽 OpenAI / DeepSeek 构造细节。

use std::sync::Arc;

use crate::{DeepSeekProvider, LlmProvider, OpenAiProvider};

/// 创建一个 LLM provider 实例 / create an LLM provider instance.
///
/// 优先级:
/// 1. 若显式设置 `LLM_PROVIDER`，按指定值创建
/// 2. 若环境变量 `OPENAI_API_KEY` 存在，使用 OpenAI provider
/// 3. 若环境变量 `DEEPSEEK_API_KEY` 存在，使用 DeepSeek provider
///
/// 用户可通过 `LLM_PROVIDER=openai|deepseek` 显式指定。
pub fn create_provider_from_env() -> axon_core::Result<Arc<dyn LlmProvider>> {
    let explicit = std::env::var("LLM_PROVIDER").ok();
    match explicit.as_deref() {
        Some("openai") => Ok(Arc::new(OpenAiProvider::from_env()?)),
        Some("deepseek") => Ok(Arc::new(DeepSeekProvider::from_env()?)),
        Some(other) => Err(axon_core::Error::Config(format!(
            "unknown LLM_PROVIDER: {other}"
        ))),
        None => {
            if std::env::var("OPENAI_API_KEY").is_ok() {
                Ok(Arc::new(OpenAiProvider::from_env()?))
            } else {
                Ok(Arc::new(DeepSeekProvider::from_env()?))
            }
        }
    }
}

/// 根据 provider id 创建 provider / create provider by id.
///
/// 用于测试或明确指定场景；模型从对应环境变量读取。
pub fn create_provider(id: &str) -> axon_core::Result<Arc<dyn LlmProvider>> {
    match id {
        "openai" => Ok(Arc::new(OpenAiProvider::from_env()?)),
        "deepseek" => Ok(Arc::new(DeepSeekProvider::from_env()?)),
        _ => Err(axon_core::Error::Config(format!(
            "unknown LLM provider: {id}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// 环境变量是全局状态，串行化环境相关测试避免并发干扰。
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// 当未设置 OPENAI_API_KEY 时，默认创建 DeepSeek provider。
    #[test]
    fn default_to_deepseek_without_openai_key() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard_key = EnvGuard::without("OPENAI_API_KEY");
        let _guard_deepseek = EnvGuard::with("DEEPSEEK_API_KEY", "sk-test");
        let _guard_provider = EnvGuard::without("LLM_PROVIDER");
        let provider = create_provider_from_env().unwrap();
        assert_eq!(provider.id(), "deepseek");
    }

    /// 当 OPENAI_API_KEY 存在时，默认创建 OpenAI provider。
    #[test]
    fn prefer_openai_with_api_key() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard_provider = EnvGuard::without("LLM_PROVIDER");
        let _guard_key = EnvGuard::with("OPENAI_API_KEY", "sk-test");
        let _guard_deepseek = EnvGuard::without("DEEPSEEK_API_KEY");
        let provider = create_provider_from_env().unwrap();
        assert_eq!(provider.id(), "openai");
    }

    /// 当只设置 DEEPSEEK_API_KEY 时，默认创建 DeepSeek provider。
    #[test]
    fn prefer_deepseek_when_only_deepseek_key() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard_provider = EnvGuard::without("LLM_PROVIDER");
        let _guard_key = EnvGuard::without("OPENAI_API_KEY");
        let _guard_deepseek = EnvGuard::with("DEEPSEEK_API_KEY", "sk-test");
        let provider = create_provider_from_env().unwrap();
        assert_eq!(provider.id(), "deepseek");
    }

    /// 显式 LLM_PROVIDER=deepseek 会覆盖 OPENAI_API_KEY 默认。
    #[test]
    fn explicit_provider_wins() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard_key = EnvGuard::with("OPENAI_API_KEY", "sk-test");
        let _guard_provider = EnvGuard::with("LLM_PROVIDER", "deepseek");
        let _guard_deepseek = EnvGuard::with("DEEPSEEK_API_KEY", "sk-test");
        let provider = create_provider_from_env().unwrap();
        assert_eq!(provider.id(), "deepseek");
    }

    /// 显式 LLM_PROVIDER=deepseek 会创建 DeepSeek provider。
    #[test]
    fn explicit_deepseek_provider() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard_key = EnvGuard::without("OPENAI_API_KEY");
        let _guard_deepseek = EnvGuard::with("DEEPSEEK_API_KEY", "sk-test");
        let _guard_provider = EnvGuard::with("LLM_PROVIDER", "deepseek");
        let provider = create_provider_from_env().unwrap();
        assert_eq!(provider.id(), "deepseek");
    }

    /// 辅助结构：临时设置/清除环境变量，测试结束后恢复。
    struct EnvGuard {
        key: &'static str,
        old: Option<String>,
    }

    impl EnvGuard {
        fn with(key: &'static str, value: &str) -> Self {
            let old = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, old }
        }

        fn without(key: &'static str) -> Self {
            let old = std::env::var(key).ok();
            std::env::remove_var(key);
            Self { key, old }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.old {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}
