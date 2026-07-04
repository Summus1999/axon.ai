#![cfg(feature = "e2e-real")]

//! 真实 DeepSeek API 端到端集成测试 / real DeepSeek API end-to-end integration test.
//!
//! 需要:
//! - `DEEPSEEK_API_KEY` 环境变量
//! - 本地 Docker 运行
//! - 联网
//!
//! 运行: `cargo test --workspace --features e2e-real`

use std::process::Command;

/// 验证 `axon run --goal "..."` 能调用真实 DeepSeek API,
/// 在 Docker 容器内生成 Rust 项目并跑通 cargo test。
#[tokio::test]
async fn real_deepseek_end_to_end() {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();

    ensure_env();
    ensure_docker();

    std::env::set_var("LLM_PROVIDER", "deepseek");
    std::env::set_var("AXON_MEMORY_BACKEND", "memory");

    let workspace = tempfile::tempdir().expect("failed to create temp workspace");
    let goal = "写一个返回 hello world 的 Rust 函数并跑通 cargo test";

    let results = axon_cli::run_goal(goal, workspace.path(), "rust:latest")
        .await
        .expect("run_goal failed");

    assert_eq!(results.len(), 1, "expected exactly one task result");
    let result = &results[0];
    assert_eq!(
        result.exit_code, 0,
        "cargo test failed: stdout={}\nstderr={}",
        result.stdout, result.stderr
    );
    assert!(
        result.stdout.contains("test result: ok"),
        "expected 'test result: ok' in stdout, got:\n{}",
        result.stdout
    );

    // 验证 Docker 无残留(以 axon- 前缀命名的容器)。
    let output = Command::new("docker")
        .args(["ps", "-a", "--format", "{{.Names}}"])
        .output()
        .expect("failed to run docker ps");
    let names = String::from_utf8_lossy(&output.stdout);
    let leftovers: Vec<&str> = names.lines().filter(|n| n.starts_with("axon-")).collect();
    assert!(
        leftovers.is_empty(),
        "found leftover axon containers: {:?}",
        leftovers
    );
}

/// 确保 DeepSeek API key 已设置。
fn ensure_env() {
    if std::env::var("DEEPSEEK_API_KEY").is_err() {
        panic!("DEEPSEEK_API_KEY must be set to run real e2e tests");
    }
}

/// 确保 Docker 可用。
fn ensure_docker() {
    let output = Command::new("docker")
        .args(["info"])
        .output()
        .expect("failed to run docker info; is Docker running?");
    if !output.status.success() {
        panic!(
            "Docker is not available: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
