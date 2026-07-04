//! DeepSeek provider 真实网络冒烟测试 / real-network smoke test for DeepSeek provider.
//!
//! 需要有效的 `DEEPSEEK_API_KEY` 环境变量；默认 `#[ignore]`，避免 CI 失败。

use axon_llm::{CompletionRequest, DeepSeekProvider, LlmProvider, Message, Role};

/// 验证 DeepSeek provider 能端到端返回非空回复。
#[tokio::test]
#[ignore = "requires live DEEPSEEK_API_KEY and network"]
async fn deepseek_complete_returns_content() {
    let provider = DeepSeekProvider::from_env().expect("DEEPSEEK_API_KEY must be set");
    let req = CompletionRequest {
        model: std::env::var("DEEPSEEK_MODEL").unwrap_or_else(|_| "deepseek-v4-pro".into()),
        messages: vec![Message {
            role: Role::User,
            content: "hi, reply with a single word".into(),
            tool_calls: None,
        }],
        tools: vec![],
        temperature: 0.0,
        max_tokens: Some(10),
    };

    let resp = provider
        .complete(req)
        .await
        .expect("DeepSeek API call failed");
    assert!(
        !resp.message.content.trim().is_empty(),
        "DeepSeek returned empty content: {:?}",
        resp
    );
}
