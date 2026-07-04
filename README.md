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
- [ ] **M1** 单机 CLI 跑通(OpenAI/Ollama + Docker 隔离)
- [ ] **M2** 记忆系统(Qdrant + redb)
- [ ] **M3** Firecracker 强隔离
- [ ] **M4** 分布式 + Web 仪表盘

---

## 技术栈 / Tech Stack

| 领域 | 选型 |
|------|------|
| 语言 | Rust (Edition 2021) |
| 异步运行时 | tokio |
| LLM 接入 | 多 Provider 抽象(OpenAI / Anthropic / Ollama) |
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

# 运行 CLI(骨架阶段,占位输出)
cargo run -p axon-cli -- run --goal "实现一个 hello world"
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
