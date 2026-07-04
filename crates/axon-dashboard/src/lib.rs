//! axon-dashboard — Web 仪表盘后端 / the Web dashboard backend.
//!
//! 基于 axum 提供 REST API + WebSocket 实时推送:
//! - 任务队列与状态查询
//! - 隔离环境(VM)监控
//! - 记忆浏览与调节
//!
//! 具体路由与 dispatcher / memory 集成留待 M4。骨架阶段仅占位。

#![allow(dead_code)]

/// 启动 Web 服务(占位)/ start the web server (placeholder).
///
/// TODO(M4): 接入 axum Router,挂载 REST + WebSocket 路由。
pub async fn serve(_addr: &str) -> axon_core::Result<()> {
    Err(axon_core::Error::Other(
        "dashboard::serve not yet implemented (skeleton, M4)".into(),
    ))
}

/// 仪表盘配置 / dashboard configuration.
#[derive(Debug, Clone)]
pub struct DashboardConfig {
    pub bind_addr: String,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:8080".into(),
        }
    }
}
