//! Docker 隔离 Provider / Docker isolation provider.
//!
//! M1 使用系统 `docker` CLI 而非 bollard，避免引入额外未缓存依赖，
//! 同时兼容 Windows Docker Desktop。
//!
//! 实现策略:
//! - `create_vm`: `docker run -d` 启动一个长期运行的容器作为 "VM"
//! - `exec`: `docker exec` 在容器内执行命令
//! - `destroy`: `docker stop && docker rm` 清理容器
//! - `snapshot`: Docker 不支持快照，返回错误

use std::process::Stdio;

use async_trait::async_trait;
use tokio::process::Command;

use axon_core::{Error, Result};

use crate::{
    Backend, Command as VmCommand, ExecOutput, IsolationProvider, Snapshot, VmHandle, VmSpec,
};

/// Docker 隔离 provider / Docker isolation provider.
#[derive(Debug, Default)]
pub struct DockerProvider;

impl DockerProvider {
    /// 创建一个新的 Docker provider / create a new Docker provider.
    pub fn new() -> Self {
        Self
    }

    /// 执行一条 `docker` 子命令并返回 stdout / run a docker subcommand and return stdout.
    async fn docker(&self, args: &[String]) -> Result<String> {
        let output = Command::new("docker")
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(Error::Io)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(Error::Isolation(format!(
                "docker {} failed ({}): {stderr}",
                args.join(" "),
                output.status
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// 根据 `VmSpec` 构建 `docker run` 参数(不含 image 与命令)/ build docker run args.
    ///
    /// 提取为独立方法以便单元测试验证资源限制参数。
    fn build_run_args(&self, vm_id: &str, spec: &VmSpec) -> Vec<String> {
        let mut args = vec![
            "run".into(),
            "-d".into(),
            "--rm".into(),
            "--name".into(),
            vm_id.into(),
        ];

        if !spec.network {
            args.push("--network".into());
            args.push("none".into());
        }

        if let Some(workspace) = &spec.workspace {
            args.push("-v".into());
            args.push(format!("{}:/workspace", workspace));
        }

        for (k, v) in &spec.env {
            args.push("-e".into());
            args.push(format!("{k}={v}"));
        }

        // CPU / memory limits are best-effort via Docker flags.
        if spec.vcpus > 0 {
            args.push("--cpus".into());
            args.push(spec.vcpus.to_string());
        }
        if spec.mem_mb > 0 {
            args.push("-m".into());
            args.push(format!("{}m", spec.mem_mb));
            // 禁止 swap,避免任务因 swap 拖慢或泄漏到磁盘。
            args.push("--memory-swap".into());
            args.push(format!("{}m", spec.mem_mb));
        }

        args
    }
}

#[async_trait]
impl IsolationProvider for DockerProvider {
    fn backend(&self) -> Backend {
        Backend::Docker
    }

    async fn create_vm(&self, spec: VmSpec) -> Result<VmHandle> {
        let vm_id = axon_core::new_id();
        let image = if spec.rootfs.is_empty() {
            "alpine:latest".into()
        } else {
            spec.rootfs.clone()
        };

        // 预拉镜像，避免 run 时首次拉取超时 / pull image explicitly.
        self.docker(&["pull".into(), image.clone()]).await?;

        let mut args = self.build_run_args(&vm_id, &spec);
        args.push(image);
        // 让容器保持运行，便于后续 docker exec。
        args.push("sleep".into());
        args.push("3600".into());

        self.docker(&args).await?;

        Ok(VmHandle {
            id: vm_id,
            backend: Backend::Docker,
        })
    }

    async fn exec(&self, vm: &VmHandle, cmd: VmCommand) -> Result<ExecOutput> {
        let mut args = vec!["exec".into()];

        if let Some(cwd) = &cmd.cwd {
            args.push("-w".into());
            args.push(cwd.clone());
        }

        for (k, v) in &cmd.env {
            args.push("-e".into());
            args.push(format!("{k}={v}"));
        }

        args.push(vm.id.clone());
        for arg in &cmd.argv {
            args.push(arg.clone());
        }

        let output = if let Some(timeout) = cmd.timeout_secs {
            // Docker CLI 本身不支持单次 exec 超时，tokio 做外部超时。
            let mut command = Command::new("docker");
            command.args(&args).kill_on_drop(true);
            match tokio::time::timeout(
                tokio::time::Duration::from_secs(timeout as u64),
                command.output(),
            )
            .await
            {
                Ok(Ok(output)) => output,
                Ok(Err(e)) => return Err(Error::Io(e)),
                Err(_) => return Err(Error::Isolation("docker exec timed out".into())),
            }
        } else {
            Command::new("docker")
                .args(&args)
                .output()
                .await
                .map_err(Error::Io)?
        };

        Ok(ExecOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    async fn snapshot(&self, _vm: &VmHandle) -> Result<Snapshot> {
        Err(Error::Isolation("Docker does not support snapshot".into()))
    }

    async fn destroy(&self, vm: VmHandle) -> Result<()> {
        // 忽略 stop 错误，容器可能已经退出 / ignore stop errors.
        let _ = self
            .docker(&["stop".into(), "-t".into(), "5".into(), vm.id.clone()])
            .await;
        let _ = self.docker(&["rm".into(), vm.id]).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证 Docker provider 的后端标识。
    #[test]
    fn backend_is_docker() {
        let provider = DockerProvider::new();
        assert_eq!(provider.backend(), Backend::Docker);
    }

    /// 验证 snapshot 返回不支持的错误。
    #[tokio::test]
    async fn snapshot_unsupported() {
        let provider = DockerProvider::new();
        let vm = VmHandle {
            id: "vm-test".into(),
            backend: Backend::Docker,
        };
        let err = provider.snapshot(&vm).await.unwrap_err();
        assert!(matches!(err, Error::Isolation(_)));
    }

    /// 验证 `build_run_args` 正确生成资源限制参数。
    #[test]
    fn run_args_apply_resource_limits() {
        let provider = DockerProvider::new();
        let spec = VmSpec {
            vcpus: 2,
            mem_mb: 512,
            rootfs: "alpine:latest".into(),
            kernel: None,
            workspace: Some("/tmp/ws".into()),
            env: vec![("FOO".into(), "bar".into())],
            network: false,
        };

        let args = provider.build_run_args("vm-42", &spec);
        let args = args.join(" ");

        assert!(args.contains("--name vm-42"));
        assert!(args.contains("--network none"));
        assert!(args.contains("-v /tmp/ws:/workspace"));
        assert!(args.contains("-e FOO=bar"));
        assert!(args.contains("--cpus 2"));
        assert!(args.contains("-m 512m"));
        assert!(args.contains("--memory-swap 512m"));
    }

    /// 验证 `network = true` 时不加 `--network none`。
    #[test]
    fn run_args_allow_network() {
        let provider = DockerProvider::new();
        let spec = VmSpec {
            vcpus: 1,
            mem_mb: 256,
            rootfs: "alpine:latest".into(),
            kernel: None,
            workspace: None,
            env: vec![],
            network: true,
        };

        let args = provider.build_run_args("vm-net", &spec).join(" ");
        assert!(!args.contains("--network none"));
    }
}
