//! axon-cli — axon.ai 命令行控制面 / the CLI control plane.
//!
//! 二进制入口 `axon`,提供任务下发、状态查询、记忆管理等命令。
//! M2 实现 `axon run --goal` 使用 HybridMemoryStore(redb + Qdrant)，
//! 并支持 `axon memory list/forget/adjust/init`。

use clap::{Parser, Subcommand};

use axon_core::Config;
use axon_memory::{MemoryFilter, MemoryKind};

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
    /// 初始化记忆存储（创建 redb 文件与 Qdrant collection）/ init memory stores.
    Init,
    /// 列出记忆 / list memories.
    List {
        /// 按类别过滤: short_term | episodic | semantic | user_profile
        #[arg(short, long)]
        kind: Option<String>,
        /// 按来源过滤
        #[arg(short, long)]
        source: Option<String>,
        /// 最小权重
        #[arg(long)]
        min_weight: Option<f32>,
    },
    /// 遗忘一条记忆 / forget a memory.
    Forget { id: String },
    /// 调节权重 / adjust a memory's weight.
    Adjust { id: String, weight: f32 },
    /// 按半衰期衰减记忆权重 / decay memory weights by half-life days.
    Decay {
        /// 半衰期天数,默认 30 / half-life in days.
        #[arg(long, default_value_t = 30.0)]
        half_life_days: f32,
    },
    /// 语义搜索记忆 / semantic search memories.
    Search {
        /// 查询文本 / query text.
        query: String,
        /// 返回 Top-K 结果,默认 5 / top-k results.
        #[arg(short, long, default_value_t = 5)]
        top_k: usize,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let level = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt().with_env_filter(level).init();

    match cli.command {
        Commands::Run { goal } => {
            let results =
                axon_cli::run_goal(&goal, std::path::Path::new("."), "rust:latest").await?;
            println!("✓ 任务调度完成，共 {} 个结果", results.len());
            for r in results {
                println!(
                    "  - 任务 {}: exit_code={}\n    stdout: {}\n    stderr: {}",
                    r.task_id, r.exit_code, r.stdout, r.stderr
                );
            }
        }
        Commands::Tasks => {
            let records = axon_cli::list_tasks()?;
            if records.is_empty() {
                println!("暂无任务记录，先运行 `axon run --goal ...`");
            } else {
                println!("共 {} 条任务记录:", records.len());
                for r in records {
                    println!(
                        "  - [{}] {} | status={} | exit_code={} | finished_at={}",
                        r.task_id, r.task_id, r.status, r.exit_code, r.finished_at
                    );
                    if !r.stdout.is_empty() {
                        println!("    stdout: {}", r.stdout.lines().next().unwrap_or(""));
                    }
                    if !r.stderr.is_empty() {
                        println!("    stderr: {}", r.stderr.lines().next().unwrap_or(""));
                    }
                }
            }
        }
        Commands::Memory { action } => handle_memory(action).await?,
        Commands::Vms => {
            let records = axon_cli::list_vms()?;
            if records.is_empty() {
                println!("暂无运行中 VM");
            } else {
                println!("共 {} 个运行中 VM:", records.len());
                for r in records {
                    let hb = r
                        .last_heartbeat_ms
                        .map(|t| format!("{t}"))
                        .unwrap_or_else(|| "-".into());
                    println!(
                        "  - [{}] {} | task={} | backend={} | created_at={} | heartbeat={}",
                        r.vm_id, r.vm_id, r.task_id, r.backend, r.created_at, hb
                    );
                }
            }
        }
    }

    Ok(())
}

/// 从默认路径加载配置 / load global config from default paths.
fn load_config() -> anyhow::Result<Config> {
    Config::load().map_err(|e| anyhow::anyhow!("failed to load config: {e}"))
}

/// 处理 memory 子命令 / handle memory subcommands.
async fn handle_memory(action: MemoryAction) -> anyhow::Result<()> {
    let cfg = load_config()?;
    let store = axon_cli::create_memory_store_from_config(&cfg).await?;

    match action {
        MemoryAction::Init => {
            // 通过 list 验证 store 可用。
            let list = store.list(&MemoryFilter::default()).await?;
            println!("✓ 记忆存储初始化完成，当前共有 {} 条记忆", list.len());
        }
        MemoryAction::List {
            kind,
            source,
            min_weight,
        } => {
            let filter = MemoryFilter {
                kind: kind.as_deref().and_then(parse_memory_kind),
                source,
                min_weight,
            };
            let memories = store.list(&filter).await?;
            println!("共 {} 条记忆:", memories.len());
            for m in memories {
                println!(
                    "  [{}] {} | kind={:?} | weight={:.2} | source={:?}",
                    m.id, m.content, m.kind, m.weight, m.source
                );
            }
        }
        MemoryAction::Forget { id } => {
            store.forget(&id).await?;
            println!("✓ 已遗忘记忆 {}", id);
        }
        MemoryAction::Adjust { id, weight } => {
            store.adjust_weight(&id, weight).await?;
            println!("✓ 已调节记忆 {} 权重为 {:.2}", id, weight);
        }
        MemoryAction::Decay { half_life_days } => {
            store.decay_weights(half_life_days).await?;
            println!("✓ 已按半衰期 {:.1} 天衰减记忆权重", half_life_days);
        }
        MemoryAction::Search { query, top_k } => {
            let memories = store
                .recall(&axon_memory::RecallQuery {
                    query,
                    kind: None,
                    top_k,
                })
                .await?;
            println!("共召回 {} 条记忆:", memories.len());
            for m in memories {
                println!(
                    "  [{}] {} | kind={:?} | weight={:.2}",
                    m.id, m.content, m.kind, m.weight
                );
            }
        }
    }
    Ok(())
}

/// 将字符串解析为 MemoryKind / parse a memory kind string.
fn parse_memory_kind(s: &str) -> Option<MemoryKind> {
    match s {
        "short_term" => Some(MemoryKind::ShortTerm),
        "episodic" => Some(MemoryKind::Episodic),
        "semantic" => Some(MemoryKind::Semantic),
        "user_profile" => Some(MemoryKind::UserProfile),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    /// 验证 CLI 能正确解析 `run --goal` 参数。
    #[test]
    fn cli_parses_run_command() {
        let cli = Cli::parse_from(["axon", "run", "--goal", "create a file"]);
        match cli.command {
            Commands::Run { goal } => assert_eq!(goal, "create a file"),
            _ => panic!("expected Run command"),
        }
    }

    /// 验证 kind 字符串解析。
    #[test]
    fn parse_kind_works() {
        assert_eq!(parse_memory_kind("semantic"), Some(MemoryKind::Semantic));
        assert_eq!(parse_memory_kind("unknown"), None);
    }

    /// 验证 `memory decay` 默认半衰期为 30 天。
    #[test]
    fn cli_parses_decay_command() {
        let cli = Cli::parse_from(["axon", "memory", "decay"]);
        match cli.command {
            Commands::Memory {
                action: MemoryAction::Decay { half_life_days },
            } => assert!((half_life_days - 30.0).abs() < f32::EPSILON),
            _ => panic!("expected Decay command"),
        }
    }

    /// 验证 `memory search` 解析 query 与 top_k。
    #[test]
    fn cli_parses_search_command() {
        let cli = Cli::parse_from(["axon", "memory", "search", "rust", "--top-k", "3"]);
        match cli.command {
            Commands::Memory {
                action: MemoryAction::Search { query, top_k },
            } => {
                assert_eq!(query, "rust");
                assert_eq!(top_k, 3);
            }
            _ => panic!("expected Search command"),
        }
    }
}
