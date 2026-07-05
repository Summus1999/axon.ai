//! axon-isolation — 隔离执行环境 / isolated execution environments.
//!
//! 通过 [`IsolationProvider`] trait 抽象"任务执行沙箱":
//! - [`DockerProvider`]: 开发期默认,跨平台(Windows Docker Desktop 可用)
//! - [`FirecrackerProvider`]: 生产强隔离 microVM(需 Linux/KVM)
//!
//! 两者可经配置切换,具体实现留待 M1(Docker)/ M3(Firecracker)。

pub mod docker;

#[cfg(unix)]
pub mod firecracker;
#[cfg(unix)]
pub use firecracker::FirecrackerProvider;

#[cfg(windows)]
pub mod placeholder;
#[cfg(windows)]
pub use placeholder::FirecrackerProvider;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use axon_core::{Result, VmId};

pub use docker::DockerProvider;

/// VM 资源规格 / resource spec for a microVM/container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmSpec {
    /// CPU 核数(权重)/ vCPU count.
    pub vcpus: u8,
    /// 内存 MB / memory in MB.
    pub mem_mb: u32,
    /// 根文件系统镜像 / rootfs image (path or image ref).
    pub rootfs: String,
    /// 内核镜像(Firecracker 专用, Docker 可空)/ kernel image.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kernel: Option<String>,
    /// 工作目录挂载源 / mounted workspace source path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// 环境变量 / environment variables.
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// 是否允许出站网络 / allow outbound network.
    #[serde(default)]
    pub network: bool,
}

/// 运行中 VM 的句柄 / a handle to a running isolated environment.
#[derive(Debug, Clone)]
pub struct VmHandle {
    pub id: VmId,
    pub backend: Backend,
}

/// 隔离后端标识 / which isolation backend.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Docker,
    Firecracker,
}

/// 在 VM 内执行的命令 / a command to execute inside the VM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    pub argv: Vec<String>,
    pub cwd: Option<String>,
    pub env: Vec<(String, String)>,
    pub timeout_secs: Option<u32>,
}

/// 命令执行输出 / execution output of a command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// VM 快照 / a snapshot for fast restore (Firecracker).
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub vm_id: VmId,
    pub mem_path: String,
    pub diff_path: String,
}

/// 隔离 Provider 抽象 / the isolation provider trait.
#[async_trait]
pub trait IsolationProvider: Send + Sync {
    /// 后端类型 / backend identifier.
    fn backend(&self) -> Backend;

    /// 创建并启动一个隔离环境 / create & start an isolated environment.
    async fn create_vm(&self, spec: VmSpec) -> Result<VmHandle>;

    /// 在 VM 内执行命令 / execute a command inside the VM.
    async fn exec(&self, vm: &VmHandle, cmd: Command) -> Result<ExecOutput>;

    /// 拍快照(支持快速恢复,Firecracker 专用)/ snapshot the VM.
    async fn snapshot(&self, vm: &VmHandle) -> Result<Snapshot>;

    /// 销毁 VM / destroy the VM, releasing all resources.
    async fn destroy(&self, vm: VmHandle) -> Result<()>;
}
