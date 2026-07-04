//! axon-cli — axon.ai 命令行控制面 / the CLI control plane.
//!
//! 二进制入口 `axon`,提供任务下发、状态查询、记忆管理等命令。
//! 骨架阶段仅注册子命令并打印占位信息;M1 起逐步接入各子系统。

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "axon", version, about = "axon.ai — AI harness 开发框架 CLI", long_about = None)]
struct Cli {
    /// 启用调试日志 / enable debug logging.
    #[arg(short, long)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// 下发一个开发任务 / submit a development goal.
    Run {
        /// 任务描述 / goal description.
        #[arg(short, long)]
        goal: String,
    },
    /// 查看任务状态 / list tasks / show task status.
    Tasks,
    /// 记忆管理 / memory management.
    Memory {
        #[command(subcommand)]
        action: MemoryAction,
    },
    /// 查看隔离环境(VM)状态 / list isolated environments.
    Vms,
}

#[derive(Subcommand, Debug)]
enum MemoryAction {
    /// 列出记忆 / list memories.
    List,
    /// 遗忘一条记忆 / forget a memory.
    Forget { id: String },
    /// 调节权重 / adjust a memory's weight.
    Adjust { id: String, weight: f32 },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let level = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt().with_env_filter(level).init();

    match cli.command {
        Commands::Run { goal } => {
            tracing::info!(%goal, "提交任务 (skeleton, M1 将接入 dispatcher)");
            println!(
                "✓ 已收到任务:\n  {}\n\n(骨架阶段:实际调度留待 M1 实现)",
                goal
            );
        }
        Commands::Tasks => {
            println!("(骨架阶段:任务列表留待 M1 实现)");
        }
        Commands::Memory { action } => match action {
            MemoryAction::List => println!("(骨架阶段:记忆列表留待 M2 实现)"),
            MemoryAction::Forget { id } => {
                println!("(骨架阶段:遗忘 `{}` 留待 M2 实现)", id);
            }
            MemoryAction::Adjust { id, weight } => {
                println!("(骨架阶段:调节 `{}` -> {:.2} 留待 M2 实现)", id, weight);
            }
        },
        Commands::Vms => {
            println!("(骨架阶段:VM 列表留待 M1/M3 实现)");
        }
    }

    Ok(())
}
