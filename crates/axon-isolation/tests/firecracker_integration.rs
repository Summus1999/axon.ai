//! Firecracker 集成测试 / Firecracker integration tests.
//!
//! 这些测试需要 Linux + KVM + Firecracker 二进制文件 + 内核/根文件系统镜像。
//! Windows 不编译;CI 中通过 `--ignored` 运行。

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;

use axon_isolation::{FirecrackerProvider, IsolationProvider, VmSpec};

/// 每个 fixture 名称对应的覆盖环境变量 / env override for each fixture.
///
/// 优先级:env var > 仓库内 `tests/fixtures/{name}` > None。
/// env var 名与 CI workflow(`FIRECRACKER_BINARY`/`KERNEL_IMAGE`/`ROOTFS_IMAGE`)对齐。
fn fixture(name: &str) -> Option<PathBuf> {
    let env_key = match name {
        "firecracker" => "FIRECRACKER_BINARY",
        "vmlinux" => "KERNEL_IMAGE",
        "rootfs.ext4" => "ROOTFS_IMAGE",
        other => {
            // 兜底:未知 fixture 回退到大写名,保持向后兼容。
            let fallback = other.to_uppercase().replace('-', "_");
            return std::env::var(&fallback)
                .ok()
                .map(PathBuf::from)
                .or_else(|| {
                    let path = PathBuf::from(format!("tests/fixtures/{other}"));
                    path.exists().then_some(path)
                });
        }
    };
    if let Ok(val) = std::env::var(env_key) {
        return Some(PathBuf::from(val));
    }
    let path = PathBuf::from(format!("tests/fixtures/{name}"));
    path.exists().then_some(path)
}

/// 验证能启动 microVM、拍快照并销毁。
#[tokio::test]
#[ignore = "requires Linux + KVM + Firecracker binaries"]
async fn firecracker_lifecycle_create_snapshot_destroy() {
    let binary =
        fixture("firecracker").expect("FIRECRACKER_BINARY not set and default fixture missing");
    let kernel = fixture("vmlinux").expect("KERNEL_IMAGE not set and default fixture missing");
    let rootfs = fixture("rootfs.ext4").expect("ROOTFS_IMAGE not set and default fixture missing");

    let workdir =
        std::env::var("AXON_FC_WORKDIR").unwrap_or_else(|_| ".axon/firecracker-tests".to_string());
    let provider = Arc::new(FirecrackerProvider::with_options(
        binary,
        Some(kernel),
        Some(rootfs),
        Some(workdir),
    ));

    let spec = VmSpec {
        vcpus: 2,
        mem_mb: 256,
        rootfs: String::new(),
        kernel: None,
        workspace: None,
        env: vec![],
        network: false,
    };

    let vm = provider.create_vm(spec).await.expect("failed to create vm");
    assert_eq!(vm.backend, axon_isolation::Backend::Firecracker);

    let snapshot = provider
        .snapshot(&vm)
        .await
        .expect("failed to create snapshot");
    assert!(!snapshot.mem_path.is_empty());
    assert!(!snapshot.diff_path.is_empty());

    provider.destroy(vm).await.expect("failed to destroy vm");
}
