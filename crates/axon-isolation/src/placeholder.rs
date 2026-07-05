//! Windows 占位 Firecracker provider / placeholder Firecracker backend for Windows.
//!
//! Firecracker 依赖 Linux/KVM,Windows 无法原生运行。本模块仅在 Windows 目标下编译,
//! 提供与 `IsolationProvider` trait 兼容的占位实现,所有方法返回明确错误。
//! Linux 真实实现见 [`crate::firecracker`]。

use async_trait::async_trait;

use axon_core::{Error, Result};

use crate::{Backend, Command, ExecOutput, IsolationProvider, Snapshot, VmHandle, VmSpec};

/// Windows 占位 Firecracker provider / placeholder Firecracker backend.
#[derive(Debug, Default)]
pub struct FirecrackerProvider;

impl FirecrackerProvider {
    /// 创建占位 provider / create a placeholder provider.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl IsolationProvider for FirecrackerProvider {
    fn backend(&self) -> Backend {
        Backend::Firecracker
    }

    async fn create_vm(&self, _spec: VmSpec) -> Result<VmHandle> {
        Err(Error::Isolation(
            "Firecracker is not supported on Windows (requires Linux/KVM)".into(),
        ))
    }

    async fn exec(&self, _vm: &VmHandle, _cmd: Command) -> Result<ExecOutput> {
        Err(Error::Isolation(
            "Firecracker is not supported on Windows (requires Linux/KVM)".into(),
        ))
    }

    async fn snapshot(&self, _vm: &VmHandle) -> Result<Snapshot> {
        Err(Error::Isolation(
            "Firecracker is not supported on Windows (requires Linux/KVM)".into(),
        ))
    }

    async fn destroy(&self, _vm: VmHandle) -> Result<()> {
        Err(Error::Isolation(
            "Firecracker is not supported on Windows (requires Linux/KVM)".into(),
        ))
    }
}
