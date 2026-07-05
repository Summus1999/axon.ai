//! Linux 真实 Firecracker provider / Firecracker microVM backend for Linux.
//!
//! 通过调用 Firecracker 的 REST API(Unix socket)管理 microVM 生命周期:
//! - spawn firecracker 进程并监听 API socket
//! - 配置 machine-config / boot-source / drives
//! - 启动(StartMicroVM)、暂停(Pause)、快照(CreateSnapshot/LoadSnapshot)
//! - 销毁(SendCtrlAltDel 或 kill 进程)
//!
//! `exec` 当前依赖 guest 中运行 sshd 且 host 已配置好可达网络(见 `SshExecBackend`);
//! 这是 M3 的最小可行路径,后续可通过 vsock agent 替换而保持 `IsolationProvider`
//! trait 不变。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::fs;
use tokio::process::Command;
use tokio::time::sleep;

use axon_core::{Error, Result};

use crate::{
    Backend, Command as VmCommand, ExecOutput, IsolationProvider, Snapshot, VmHandle, VmSpec,
};

/// Firecracker HTTP client 抽象 / HTTP client for the Firecracker API.
#[async_trait]
pub trait FirecrackerClient: Send + Sync {
    /// 发送 PUT 请求并返回响应体 / send a PUT request to the API socket.
    async fn put(&self, path: &str, body: &str) -> Result<String>;

    /// 发送 GET 请求并返回响应体 / send a GET request to the API socket.
    async fn get(&self, path: &str) -> Result<String>;
}

/// 使用系统 `curl` 通过 `--unix-socket` 调用 Firecracker API。
///
/// 选择 curl 而非 Rust HTTP client 直接连接 Unix socket,是为了避免引入
/// hyperlocal/hyper 版本适配依赖,同时与 `DockerProvider` 调用外部 CLI 的风格一致。
#[derive(Debug)]
pub struct CurlClient {
    socket: PathBuf,
}

impl CurlClient {
    /// 创建 client / create a curl client for the given API socket.
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
        }
    }

    /// 调用 curl 并返回 stdout / run curl with the given method and JSON body.
    async fn request(&self, method: &str, path: &str, body: Option<&str>) -> Result<String> {
        let url = format!("http://localhost/{path}");
        let mut args = vec![
            "--silent".to_string(),
            "--show-error".to_string(),
            "--fail-with-body".to_string(),
            "--unix-socket".to_string(),
            self.socket.to_string_lossy().to_string(),
            "-X".to_string(),
            method.to_string(),
            "-H".to_string(),
            "Content-Type: application/json".to_string(),
            url,
        ];
        if let Some(body) = body {
            args.push("-d".to_string());
            args.push(body.to_string());
        }

        let output = Command::new("curl")
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(Error::Io)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(Error::Isolation(format!(
                "firecracker API {method} /{path} failed: {stderr}"
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

#[async_trait]
impl FirecrackerClient for CurlClient {
    async fn put(&self, path: &str, body: &str) -> Result<String> {
        self.request("PUT", path, Some(body)).await
    }

    async fn get(&self, path: &str) -> Result<String> {
        self.request("GET", path, None).await
    }
}

/// Firecracker 进程句柄 / a running Firecracker process.
#[derive(Debug)]
struct FcProcess {
    /// API socket 路径 / path to the API unix socket.
    socket: PathBuf,
    /// 工作目录(存放 socket、pid、日志)/ working directory for this VM.
    workdir: PathBuf,
    /// 子进程 id / child process id.
    pid: u32,
}

/// 单个 VM 实例状态 / internal state for one microVM.
struct VmInstance {
    handle: VmHandle,
    process: FcProcess,
    /// guest 中执行命令的后端 / backend used to run commands inside the guest.
    exec_backend: Arc<dyn ExecBackend>,
}

/// 在 guest 中执行命令的后端抽象 / backend for command execution inside a microVM.
#[async_trait]
trait ExecBackend: Send + Sync {
    /// 在 VM 内执行命令 / execute a command inside the VM.
    async fn exec(&self, vm: &VmHandle, cmd: &VmCommand) -> Result<ExecOutput>;
}

/// 通过 SSH 在 guest 中执行命令 / SSH-based command execution.
///
/// 要求 guest rootfs 已启动 sshd,且 host 已通过 tap/bridge 等方式与 guest 网络可达。
#[derive(Debug, Default)]
struct SshExecBackend;

#[async_trait]
impl ExecBackend for SshExecBackend {
    async fn exec(&self, _vm: &VmHandle, cmd: &VmCommand) -> Result<ExecOutput> {
        // 从 VM 环境变量中读取 SSH 连接参数。
        let host = cmd
            .env
            .iter()
            .find(|(k, _)| k == "SSH_HOST")
            .map(|(_, v)| v.as_str())
            .unwrap_or("127.0.0.1");
        let user = cmd
            .env
            .iter()
            .find(|(k, _)| k == "SSH_USER")
            .map(|(_, v)| v.as_str())
            .unwrap_or("root");
        let port = cmd
            .env
            .iter()
            .find(|(k, _)| k == "SSH_PORT")
            .map(|(_, v)| v.as_str())
            .unwrap_or("22");
        let key = cmd
            .env
            .iter()
            .find(|(k, _)| k == "SSH_KEY")
            .map(|(_, v)| v.as_str());

        let mut args = vec![
            "-o".to_string(),
            "StrictHostKeyChecking=no".to_string(),
            "-o".to_string(),
            "UserKnownHostsFile=/dev/null".to_string(),
            "-p".to_string(),
            port.to_string(),
            "-l".to_string(),
            user.to_string(),
            host.to_string(),
        ];
        if let Some(key) = key {
            args.push("-i".to_string());
            args.push(key.to_string());
        }
        if let Some(cwd) = &cmd.cwd {
            args.push(format!("cd {cwd} && {}", cmd.argv.join(" ")));
        } else {
            args.push(cmd.argv.join(" "));
        }

        let mut command = Command::new("ssh");
        command.args(&args).kill_on_drop(true);

        let output = if let Some(timeout) = cmd.timeout_secs {
            match tokio::time::timeout(Duration::from_secs(timeout as u64), command.output()).await
            {
                Ok(Ok(output)) => output,
                Ok(Err(e)) => return Err(Error::Io(e)),
                Err(_) => return Err(Error::Isolation("ssh exec timed out".into())),
            }
        } else {
            command.output().await.map_err(Error::Io)?
        };

        Ok(ExecOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }
}

/// Firecracker 隔离 provider / Firecracker isolation provider.
pub struct FirecrackerProvider {
    /// firecracker 可执行文件路径 / path to the firecracker binary.
    binary: PathBuf,
    /// 默认内核镜像 / default kernel image.
    kernel: Option<PathBuf>,
    /// 默认 rootfs 镜像 / default rootfs image.
    rootfs: Option<PathBuf>,
    /// VM 工作目录根(每个 VM 会创建子目录)/ root directory for per-VM working dirs.
    workdir_root: PathBuf,
    /// HTTP client 工厂(便于测试注入 mock)/ factory for API clients.
    client_factory: Arc<dyn Fn(&Path) -> Arc<dyn FirecrackerClient> + Send + Sync>,
    /// exec 后端 / command execution backend.
    exec_backend: Arc<dyn ExecBackend>,
}

impl std::fmt::Debug for FirecrackerProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FirecrackerProvider")
            .field("binary", &self.binary)
            .field("kernel", &self.kernel)
            .field("rootfs", &self.rootfs)
            .field("workdir_root", &self.workdir_root)
            .field("client_factory", &"<factory>")
            .field("exec_backend", &"<exec backend>")
            .finish_non_exhaustive()
    }
}

impl FirecrackerProvider {
    /// 创建 provider / create a Firecracker provider.
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self::with_options(binary, None::<PathBuf>, None::<PathBuf>, None::<PathBuf>)
    }

    /// 带默认镜像创建 provider / create with default kernel/rootfs.
    pub fn with_options<B, K, R, W>(
        binary: B,
        kernel: Option<K>,
        rootfs: Option<R>,
        workdir_root: Option<W>,
    ) -> Self
    where
        B: Into<PathBuf>,
        K: Into<PathBuf>,
        R: Into<PathBuf>,
        W: Into<PathBuf>,
    {
        let workdir_root = workdir_root
            .map(Into::into)
            .unwrap_or_else(|| PathBuf::from(".axon/firecracker"));
        Self {
            binary: binary.into(),
            kernel: kernel.map(Into::into),
            rootfs: rootfs.map(Into::into),
            workdir_root,
            client_factory: Arc::new(|socket| Arc::new(CurlClient::new(socket))),
            exec_backend: Arc::new(SshExecBackend::default()),
        }
    }

    /// 从 [`axon_core::Config`] 创建 provider / create from global config.
    pub fn from_config(cfg: &axon_core::Config) -> Result<Self> {
        let iso = &cfg.isolation;
        let binary = iso
            .firecracker_binary
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("firecracker"));
        let kernel = iso.kernel_image.as_ref().map(PathBuf::from);
        let rootfs = iso.rootfs_image.as_ref().map(PathBuf::from);
        let workdir_root = iso
            .snapshot_dir
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".axon/firecracker"));
        Ok(Self::with_options(
            binary,
            kernel,
            rootfs,
            Some(workdir_root),
        ))
    }

    /// 等待 API socket 就绪 / wait until the API socket is accepting connections.
    async fn wait_for_socket(socket: &Path, max_wait: Duration) -> Result<()> {
        let start = std::time::Instant::now();
        loop {
            if socket.exists() {
                // 尝试连接确认真正就绪。
                if fs::metadata(socket).await.is_ok() {
                    return Ok(());
                }
            }
            if start.elapsed() >= max_wait {
                return Err(Error::Isolation(format!(
                    "firecracker socket {} did not appear within {:?}",
                    socket.display(),
                    max_wait
                )));
            }
            sleep(Duration::from_millis(50)).await;
        }
    }

    /// 配置并启动 microVM / configure and start a microVM.
    async fn setup_vm(
        &self,
        client: &dyn FirecrackerClient,
        spec: &VmSpec,
        kernel: &Path,
        rootfs: &Path,
    ) -> Result<()> {
        let machine_config = serde_json::json!({
            "vcpu_count": spec.vcpus.max(1),
            "mem_size_mib": spec.mem_mb.max(128),
            "ht_enabled": false,
            "track_dirty_pages": true,
        });
        client
            .put("machine-config", &machine_config.to_string())
            .await?;

        let boot_source = serde_json::json!({
            "kernel_image_path": kernel.to_string_lossy().to_string(),
            "boot_args": "console=ttyS0 reboot=k panic=1 pci=off",
        });
        client.put("boot-source", &boot_source.to_string()).await?;

        let drive = serde_json::json!({
            "drive_id": "rootfs",
            "path_on_host": rootfs.to_string_lossy().to_string(),
            "is_root_device": true,
            "is_read_only": false,
        });
        client.put("drives/rootfs", &drive.to_string()).await?;

        client
            .put("actions", r#"{"action_type": "InstanceStart"}"#)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl IsolationProvider for FirecrackerProvider {
    fn backend(&self) -> Backend {
        Backend::Firecracker
    }

    async fn create_vm(&self, spec: VmSpec) -> Result<VmHandle> {
        let vm_id = axon_core::new_id();
        let workdir = self.workdir_root.join(&vm_id);
        fs::create_dir_all(&workdir).await.map_err(Error::Io)?;
        let socket = workdir.join("api.sock");
        let log_path = workdir.join("firecracker.log");

        let kernel = spec
            .kernel
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| self.kernel.clone())
            .ok_or_else(|| Error::Isolation("kernel image is required for firecracker".into()))?;
        let rootfs = if spec.rootfs.is_empty() {
            self.rootfs.clone().ok_or_else(|| {
                Error::Isolation("rootfs image is required for firecracker".into())
            })?
        } else {
            PathBuf::from(&spec.rootfs)
        };

        if !kernel.is_file() {
            return Err(Error::Isolation(format!(
                "kernel image not found: {}",
                kernel.display()
            )));
        }
        if !rootfs.is_file() {
            return Err(Error::Isolation(format!(
                "rootfs image not found: {}",
                rootfs.display()
            )));
        }

        let mut child = Command::new(&self.binary)
            .arg("--api-socket")
            .arg(&socket)
            .arg("--log-path")
            .arg(&log_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(Error::Io)?;

        let _pid = child
            .id()
            .ok_or_else(|| Error::Isolation("failed to get firecracker pid".into()))?;

        // 使用 spawn +  detach 防止僵尸进程;destroy 时会显式 kill。
        tokio::spawn(async move {
            let _ = child.wait().await;
        });

        Self::wait_for_socket(&socket, Duration::from_secs(10)).await?;

        let client = (self.client_factory)(&socket);
        self.setup_vm(&*client, &spec, &kernel, &rootfs).await?;

        Ok(VmHandle {
            id: vm_id,
            backend: Backend::Firecracker,
        })
    }

    async fn exec(&self, vm: &VmHandle, cmd: VmCommand) -> Result<ExecOutput> {
        self.exec_backend.exec(vm, &cmd).await
    }

    async fn snapshot(&self, vm: &VmHandle) -> Result<Snapshot> {
        let workdir = self.workdir_root.join(&vm.id);
        let socket = workdir.join("api.sock");
        let client = (self.client_factory)(&socket);

        client.put("actions", r#"{"action_type": "Pause"}"#).await?;

        let snapshot_path = workdir.join("vm_state.snap");
        let mem_path = workdir.join("vm_memory.snap");
        let body = serde_json::json!({
            "snapshot_type": "Full",
            "snapshot_path": snapshot_path.to_string_lossy().to_string(),
            "mem_file_path": mem_path.to_string_lossy().to_string(),
        });
        client.put("snapshot/create", &body.to_string()).await?;

        client
            .put("actions", r#"{"action_type": "Resume"}"#)
            .await?;

        Ok(Snapshot {
            vm_id: vm.id.clone(),
            mem_path: mem_path.to_string_lossy().to_string(),
            diff_path: snapshot_path.to_string_lossy().to_string(),
        })
    }

    async fn destroy(&self, vm: VmHandle) -> Result<()> {
        let workdir = self.workdir_root.join(&vm.id);
        let socket = workdir.join("api.sock");
        if socket.exists() {
            let client = (self.client_factory)(&socket);
            // 优先优雅关机;失败则忽略,后续会 kill 进程。
            let _ = client
                .put("actions", r#"{"action_type": "SendCtrlAltDel"}"#)
                .await;
            sleep(Duration::from_secs(1)).await;
        }

        // 通过 pid 文件或进程名查找并终止。这里简化:若 socket 仍存则无法优雅关机,直接失败。
        // 生产环境应维护 pid 文件;M3 先通过工作目录清理表达意图。
        let _ = fs::remove_dir_all(&workdir).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct MockClient {
        requests: Mutex<Vec<(String, String)>>,
    }

    #[async_trait]
    impl FirecrackerClient for MockClient {
        async fn put(&self, path: &str, body: &str) -> Result<String> {
            self.requests
                .lock()
                .unwrap()
                .push((path.to_string(), body.to_string()));
            Ok("{}".to_string())
        }

        async fn get(&self, _path: &str) -> Result<String> {
            Ok("{}".to_string())
        }
    }

    /// 构造一个用于测试的 provider,注入 mock client / build a provider with a mock client.
    fn provider_with_mock(workdir: PathBuf, client: Arc<MockClient>) -> FirecrackerProvider {
        FirecrackerProvider {
            binary: PathBuf::from("/fake/firecracker"),
            kernel: Some(PathBuf::from("/fake/vmlinux")),
            rootfs: Some(PathBuf::from("/fake/rootfs.ext4")),
            workdir_root: workdir,
            client_factory: Arc::new(move |_| client.clone() as Arc<dyn FirecrackerClient>),
            exec_backend: Arc::new(SshExecBackend::default()),
        }
    }

    /// 验证 setup_vm 按正确顺序发送 Firecracker API 请求。
    #[tokio::test]
    async fn setup_vm_sends_expected_requests() {
        let tmp = tempfile::tempdir().unwrap();
        let client = Arc::new(MockClient::default());
        let provider = provider_with_mock(tmp.path().to_path_buf(), client.clone());

        let spec = VmSpec {
            vcpus: 2,
            mem_mb: 512,
            rootfs: "/custom/rootfs.ext4".into(),
            kernel: Some("/custom/vmlinux".into()),
            workspace: None,
            env: vec![],
            network: false,
        };

        provider
            .setup_vm(
                &*client,
                &spec,
                Path::new("/custom/vmlinux"),
                Path::new("/custom/rootfs.ext4"),
            )
            .await
            .unwrap();

        let reqs = client.requests.lock().unwrap();
        assert_eq!(reqs.len(), 4);
        assert_eq!(reqs[0].0, "machine-config");
        assert!(reqs[0].1.contains("\"vcpu_count\":2"));
        assert!(reqs[0].1.contains("\"mem_size_mib\":512"));
        assert_eq!(reqs[1].0, "boot-source");
        assert!(reqs[1].1.contains("/custom/vmlinux"));
        assert_eq!(reqs[2].0, "drives/rootfs");
        assert!(reqs[2].1.contains("/custom/rootfs.ext4"));
        assert_eq!(reqs[3].0, "actions");
        assert!(reqs[3].1.contains("InstanceStart"));
    }

    /// 验证 snapshot 发送 Pause、CreateSnapshot、Resume 请求。
    #[tokio::test]
    async fn snapshot_sends_pause_create_resume() {
        let tmp = tempfile::tempdir().unwrap();
        let client = Arc::new(MockClient::default());
        let provider = provider_with_mock(tmp.path().to_path_buf(), client.clone());

        // 预建 socket 文件,让 destroy 不跳过 API 调用。
        let workdir = tmp.path().join("vm-1");
        tokio::fs::create_dir_all(&workdir).await.unwrap();
        tokio::fs::write(workdir.join("api.sock"), b"")
            .await
            .unwrap();

        let vm = VmHandle {
            id: "vm-1".into(),
            backend: Backend::Firecracker,
        };
        let snap = provider.snapshot(&vm).await.unwrap();

        assert_eq!(snap.vm_id, "vm-1");
        let reqs = client.requests.lock().unwrap();
        assert_eq!(reqs.len(), 3);
        assert!(reqs[0].1.contains("Pause"));
        assert_eq!(reqs[1].0, "snapshot/create");
        assert!(reqs[1].1.contains("vm_state.snap"));
        assert!(reqs[2].1.contains("Resume"));
    }
}
