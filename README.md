# axon.ai

> 一个面向开发者的 AI harness(线束/编排)开发框架 / A developer-oriented AI harness framework.

[![CI](https://github.com/Summus/axon.ai/actions/workflows/ci.yml/badge.svg)](./.github/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](./LICENSE)

axon.ai 把"AI 帮我写代码"从单轮对话升级为一个**可编排、可隔离、可记忆**的多智能体开发系统。它由两大核心子系统组成:

- **AI 调度大脑 (AI Brain)** — 理解用户意图、规划任务、编排 LLM;在与你的长期交互中沉淀开发习惯。
- **任务分发中心 (Task Dispatcher)** — 把任务下发到**完全隔离的 microVM** 中,由独立 AI agent 执行直到满足验收标准。

外加**记忆大脑(Memory Brain)**用于长期记忆与可调节的记忆管理,以及 **CLI + Web 仪表盘**控制面。

---

## 状态 / Status

🚧 **M0 骨架阶段 (Skeleton)** — 仅 workspace 结构 + 核心 trait 占位,尚无业务实现。

路线图见 [`docs/tech-stack.md`](./docs/tech-stack.md#6-development-roadmap--开发路线):

- [x] **M0** 骨架(workspace + trait 占位)
- [x] **M1** 单机 CLI 跑通(OpenAI/DeepSeek + Docker 隔离)
- [x] **M2** 记忆系统(Qdrant + redb)
- [ ] **M3** Firecracker 强隔离
- [ ] **M4** 分布式 + Web 仪表盘

---

## 技术栈 / Tech Stack

| 领域 | 选型 |
|------|------|
| 语言 | Rust (Edition 2021) |
| 异步运行时 | tokio |
| LLM 接入 | 多 Provider 抽象(OpenAI / DeepSeek) |
| 记忆存储 | Qdrant(向量)+ redb(KV) |
| 隔离执行 | Firecracker microVM(主)+ Docker(开发期) |
| 消息队列 | NATS(二期) |
| 控制面 | clap CLI + axum Web 仪表盘 |
| RPC | tonic (gRPC) |

完整选型理由、对比与风险分析见 **[docs/tech-stack.md](./docs/tech-stack.md)**。

---

## 快速开始 / Quick Start

### 前置要求 / Prerequisites

- Rust 1.75+(建议用 [rustup](https://rustup.rs/) 安装 stable)
- (M1 起)Docker,用于开发期隔离执行
- (M3 起)Linux + KVM,用于 Firecracker

### 构建 / Build

```bash
# 克隆
git clone https://github.com/Summus/axon.ai.git
cd axon.ai

# 构建整个 workspace
cargo build --workspace

# 运行 CLI(M1:需要配置 LLM 与 Docker)
cargo run -p axon-cli -- run --goal "创建一个 hello.txt 文件"
```

### M1 配置 / M1 Configuration

M1 支持 OpenAI 或 DeepSeek 作为 LLM 后端：

```bash
# 方案 A: OpenAI
export OPENAI_API_KEY="sk-..."
export OPENAI_MODEL="gpt-4o-mini"  # 可选,默认 gpt-4o-mini

# 方案 B: DeepSeek
export DEEPSEEK_API_KEY="sk-..."
export DEEPSEEK_MODEL="deepseek-v4-pro"  # 可选,默认 deepseek-v4-pro

# 显式指定 provider(默认按 OPENAI_API_KEY 是否存在自动选择)
export LLM_PROVIDER="deepseek"  # 或 openai
```

执行命令需要本地安装并运行 Docker。

### M2 记忆配置 / M2 Memory Configuration

M2 使用 **redb** 本地 KV 存储语义/用户画像/短期记忆，**Qdrant** 向量库存储情景记忆。

```bash
# 默认使用本地 Qdrant(http://localhost:6334) 与当前目录 .axon/memory.redb
# 可自定义：
export AXON_MEMORY_REDB_PATH="/path/to/memory.redb"
export AXON_MEMORY_QDRANT_URL="http://localhost:6334"
export AXON_MEMORY_QDRANT_COLLECTION="axon_memories"

# Embedding provider 目前仅支持 OpenAI
# DeepSeek 暂无 embedding API
export EMBEDDING_PROVIDER="openai"
export OPENAI_EMBEDDING_MODEL="text-embedding-3-small"
```

常用记忆管理命令：

```bash
# 初始化存储（验证 redb 与 Qdrant 可连接）
cargo run -p axon-cli -- memory init

# 列出记忆（支持按 kind/source/min-weight 过滤）
cargo run -p axon-cli -- memory list
cargo run -p axon-cli -- memory list --kind semantic --min-weight 1.0

# 调节/遗忘单条记忆
cargo run -p axon-cli -- memory adjust <id> 2.0
cargo run -p axon-cli -- memory forget <id>
```

### 常用命令 / Common Commands

```bash
cargo build --workspace          # 构建
cargo test --workspace           # 测试
cargo clippy --workspace -- -D warnings   # lint
cargo fmt --all                  # 格式化
```

---

## Workspace 结构 / Workspace Layout

```
axon.ai/
├── docs/tech-stack.md       # 技术选型文档
└── crates/
    ├── axon-core/           # 共享类型、错误、配置
    ├── axon-llm/            # LlmProvider trait
    ├── axon-memory/         # MemoryStore trait
    ├── axon-brain/          # Planner / Agent trait
    ├── axon-dispatcher/     # TaskQueue / Scheduler trait
    ├── axon-isolation/      # IsolationProvider trait
    ├── axon-worker/         # VM 内 agent worker
    ├── axon-proto/          # 内部消息 schema
    ├── axon-cli/            # 二进制:CLI 入口 (`axon`)
    └── axon-dashboard/      # Web 仪表盘后端
```

各 crate 职责与依赖关系详见 [技术选型文档 §3](./docs/tech-stack.md#3-module-breakdown--模块拆分)。

---

## 贡献 / Contributing

项目处于早期骨架阶段,欢迎提 issue 讨论设计。提交前请确保:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

---

## License

[MIT](./LICENSE)
