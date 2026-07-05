//! Firecracker 集成测试 / Firecracker integration tests.
//!
//! 这些测试需要 Linux + KVM + Firecracker 二进制文件 + 内核/根文件系统镜像。
//! Windows 不编译;CI 中通过 `--ignored` 运行。

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;

use axon_isolation::{FirecrackerProvider, IsolationProvider, VmSpec};

/// 从环境变量或默认路径获取 fixture / resolve a fixture path.
fn fixture(name: &str) -> Option<PathBuf> {
    let env_key = name.to_uppercase().replace('-', "_");
    if let Ok(val) = std::env::var(&env_key) {
        return Some(PathBuf::from(val));
    }
    let default = format!("tests/fixtures/{name}");
    let path = PathBuf::from(&default);
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

/// 验证能启动 microVM、拍快照并销毁。
#[tokio::test]
#[ignore = "requires Linux + KVM + Firecracker binaries"]
async fn firecracker_lifecycle_create_snapshot_destroy() {
    let binary =
        fixture("firecracker").expect("FIRECRACKER_BINARY not set and default fixture missing");
    let kernel = fixture("vmlinux").expect("KERNEL_IMAGE not set and default fixture missing");
    let rootfs = fixture("rootfs.ext4").expect("ROOTFS_IMAGE not set and default fixture missing");

    let provider = Arc::new(FirecrackerProvider::with_options(
        binary,
        Some(kernel),
        Some(rootfs),
        Some(".axon/firecracker-tests"),
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
