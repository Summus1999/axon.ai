# axon.ai 技术选型文档 / Technology Stack Document

> 版本 / Version: 0.1.0 (Draft)
> 日期 / Date: 2026-07-04
> 状态 / Status: 评审中 / Under Review

---

## 目录 / Table of Contents

1. [Overview / 概述](#1-overview--概述)
2. [System Architecture / 系统架构](#2-system-architecture--系统架构)
3. [Module Breakdown / 模块拆分](#3-module-breakdown--模块拆分)
4. [Key Technology Decisions / 关键技术决策](#4-key-technology-decisions--关键技术决策)
5. [Risk & Mitigation / 风险与缓解](#5-risk--mitigation--风险与缓解)
6. [Development Roadmap / 开发路线](#6-development-roadmap--开发路线)
7. [Workspace Layout / 工作区布局](#7-workspace-layout--工作区布局)

---

## 1. Overview / 概述

**axon.ai** 是一个面向开发者的 AI harness(线束/编排)开发框架。它的目标是把"AI 帮我写代码"从单轮对话升级为一个可编排、可隔离、可记忆的多智能体开发系统。

系统由两大核心子系统组成:

### 1.1 AI 调度大脑 (AI Brain / Scheduling Brain)

负责理解用户意图、拆解开发任务、规划执行路径。核心能力:

- **LLM 编排**:多 Provider 路由,按任务类型选择模型(规划用强模型,执行可用小模型)
- **ReAct / Planner 循环**:Thought → Action → Observation 闭环,支持多步推理
- **与用户协作**:任务下发前可向用户确认,执行中可中断/纠正

### 1.2 记忆大脑 (Memory Brain)

负责在与用户的长期交互中沉淀知识。核心能力:

- **短期记忆 (Short-term)**:单次任务会话的上下文窗口管理
- **长期记忆 (Long-term)**:跨会话的事实、偏好、用户开发习惯画像
- **记忆管理 (Memory Management)**:用户可查看 / 编辑 / 遗忘 / 调节记忆权重,避免上下文污染

### 1.3 任务分发中心 (Task Dispatcher)

接收调度大脑下发的子任务,创建任务队列,并为每个任务启动**完全隔离的执行环境**(microVM),在其中独立运行 AI agent 完成开发任务,直到满足验收标准。核心能力:

- **任务队列**:优先级调度、依赖编排(DAG)
- **隔离执行**:每个任务一个 microVM,资源/网络/文件系统隔离
- **验收闭环**:worker 自检 + 用户/调度大脑复核

### 1.4 设计目标 / Design Goals

| 目标 | 说明 |
|------|------|
| **安全隔离** | 任务执行在 microVM 内,互不影响,失败可丢弃 |
| **可记忆** | 长期沉淀用户画像与项目知识,越用越懂你 |
| **可调节** | 记忆、模型、隔离级别均可配置与人工干预 |
| **分布式原生** | 架构解耦,单机可跑,可平滑扩展到多节点 |
| **Rust 优先** | 性能、内存安全、与 Firecracker 同语言生态 |

---

## 2. System Architecture / 系统架构

### 2.1 顶层架构图 / Top-Level Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                         控制面 / Control Plane                       │
│   ┌───────────────────┐         ┌──────────────────────────────┐   │
│   │   axon-cli (clap) │         │  axon-dashboard (axum + WS)  │   │
│   │  任务下发/状态/记忆 │         │   任务队列/VM状态/记忆调节     │   │
│   └────────┬──────────┘         └──────────┬───────────────────┘   │
└────────────┼─────────────────────────────────┼─────────────────────┘
             │                                 │
             ▼                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│                       AI Brain / 调度大脑                            │
│  ┌──────────────┐   ┌──────────────────┐   ┌──────────────────┐    │
│  │  Planner     │◄─►│  LLM Orchestrator│◄─►│  Memory Brain    │    │
│  │  (ReAct/DAG) │   │  (多Provider路由)  │   │  (短期/长期/画像)  │    │
│  └──────┬───────┘   └──────────────────┘   └────────┬─────────┘    │
│         │                                            │              │
│         │  下发子任务                                 │ 读写记忆       │
└─────────┼────────────────────────────────────────────┼──────────────┘
          │                                            │
          ▼                                            ▼
┌──────────────────────────────┐        ┌──────────────────────────────┐
│   Task Dispatcher / 任务分发  │        │      Memory Store            │
│   ┌────────────────────────┐ │        │  ┌────────────┐ ┌─────────┐  │
│   │   Task Queue (DAG)     │ │        │  │  Qdrant    │ │ sled/   │  │
│   │   优先级 / 依赖编排      │ │        │  │ (向量检索)  │ │ redb    │  │
│   └───────────┬────────────┘ │        │  │            │ │ (KV)    │  │
│               │ 调度           │        │  └────────────┘ └─────────┘  │
│   ┌───────────▼────────────┐ │        └──────────────────────────────┘
│   │  VM Lifecycle Mgr      │ │
│   └───────────┬────────────┘ │
└───────────────┼──────────────┘
                │ 启动
                ▼
┌─────────────────────────────────────────────────────────────────────┐
│              Isolated Execution Environments / 隔离执行环境            │
│                                                                     │
│   ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌──────────┐  │
│   │ microVM #1  │  │ microVM #2  │  │ microVM #3  │  │   ...    │  │
│   │ ┌─────────┐ │  │ ┌─────────┐ │  │ ┌─────────┐ │  │          │  │
│   │ │ worker  │ │  │ │ worker  │ │  │ │ worker  │ │  │          │  │
│   │ │ (agent) │ │  │ │ (agent) │ │  │ │ (agent) │ │  │          │  │
│   │ └─────────┘ │  │ └─────────┘ │  │ └─────────┘ │  │          │  │
│   │  Firecracker│  │  Firecracker│  │  Firecracker│  │          │  │
│   └─────────────┘  └─────────────┘  └─────────────┘  └──────────┘  │
│         完全隔离:独立内核 / 独立文件系统 / 独立网络 / 资源限额        │
└─────────────────────────────────────────────────────────────────────┘
```

### 2.2 数据流 / Data Flow

```
用户输入 ──► axon-cli / dashboard
                │
                ▼
        AI Brain.Planner
          │        │
   读记忆 │        │ 写记忆(沉淀用户特点)
          ▼        ▼
        Memory Brain ◄──── 记忆管理(用户可调节)
          │
   规划子任务
          │
          ▼
        Task Dispatcher
          │
   入队 + 调度 + 启动 microVM
          │
          ▼
        Worker (VM 内 agent 执行开发任务)
          │
   自检验收 ──► 不通过 ──► 重试 / 回退
          │
        通过
          │
          ▼
        结果回流 ──► AI Brain 复核 ──► 用户确认 ──► 完成
```

---

## 3. Module Breakdown / 模块拆分

每个模块对应一个 workspace crate,职责单一,通过 trait 解耦。

| 子系统 | Crate | 职责 | 关键依赖 |
|--------|-------|------|---------|
| 共享类型/工具 | `axon-core` | 错误类型、配置、通用 trait | `thiserror`, `serde`, `tracing` |
| LLM Provider 抽象 | `axon-llm` | 多 Provider trait 抽象(OpenAI/DeepSeek) | `reqwest`, `tiktoken-rs`, `async-trait` |
| 记忆大脑 | `axon-memory` | 短期/长期记忆、用户画像、记忆管理 | `qdrant-client`, `redb`/`sled` |
| AI 调度大脑 | `axon-brain` | LLM 编排、任务规划、ReAct/Planner 循环 | `axon-llm`, `axon-memory`, `async-trait` |
| 任务分发中心 | `axon-dispatcher` | 任务队列(DAG)、调度、VM 生命周期管理 | `tokio`, `tonic` |
| 隔离执行环境 | `axon-isolation` | microVM/Firecracker 封装 + `IsolationProvider` trait | `firec-rs` / firecracker REST |
| 任务执行 Worker | `axon-worker` | VM 内运行的 AI agent,执行开发任务 | `axon-llm`, `axon-brain` |
| proto 定义 | `axon-proto` | gRPC/内部消息 schema | `prost`, `tonic` |
| CLI 控制面 | `axon-cli` | clap 命令行,任务下发/状态/记忆管理 | `clap`, `ratatui` |
| Web 仪表盘后端 | `axon-dashboard` | Web API + 实时状态推送 | `axum`, `tokio-tungstenite` |
| Web 前端(可选) | `axon-ui` | 仪表盘前端(独立子项目) | 文档规划,本次不实现 |

### 3.1 Crate 依赖关系 / Dependency Graph

```
                    axon-core  (基础,无下游依赖)
                    ▲   ▲   ▲
                    │   │   │
            axon-llm   axon-proto   axon-isolation
                ▲                        ▲
                │                        │
            axon-memory                  │
                ▲                        │
                │                        │
            axon-brain ◄─────┐           │
                ▲            │           │
                │            │           │
            axon-worker      │           │
                ▲            │           │
                │            │           │
        ┌───────┴────────────┴───────────┴───────┐
        │         axon-dispatcher                 │
        │  (组合 brain + isolation + queue)        │
        └───────────────────┬─────────────────────┘
                            │
                ┌───────────┴───────────┐
                ▼                       ▼
          axon-cli              axon-dashboard
        (二进制入口)              (Web 服务)
```

---

## 4. Key Technology Decisions / 关键技术决策

### 4.1 语言 / Programming Language: Rust

**选型**: Rust (Edition 2021)

**理由**:
- **性能**: 编译为原生代码,无 GC 停顿,适合长时运行的任务调度与服务进程
- **内存安全**: 所有权机制消除一大类内存 bug,框架级代码尤其受益
- **async 生态成熟**: `tokio` 已是事实标准,LLM 并发请求、VM 生命周期管理都受益
- **与 Firecracker 同语言**: Firecracker 本身用 Rust 写,社区有 `firec-rs` 等绑定,集成路径最短
- **类型系统表达力**: trait + 泛型适合构建可扩展的 Provider/Plugin 抽象

**对比**:

| 语言 | 优点 | 缺点 | 结论 |
|------|------|------|------|
| **Rust** ✅ | 性能、安全、Firecracker 同生态 | 学习曲线陡、迭代速度稍慢 | 选用 |
| Go | 并发简单、启动快 | 表达力弱、泛型受限、Firecracker SDK 少 | 备选 |
| Python | AI 生态最丰富、迭代快 | 性能弱、GIL、部署重 | 不适合框架核心 |
| TypeScript | 全栈、生态好 | 性能弱、不适合系统级隔离管理 | 不适合 |

---

### 4.2 异步运行时 / Async Runtime: tokio

**选型**: `tokio` (current-thread + multi-thread runtime)

**理由**: Rust async 事实标准,几乎所有相关 crate(reqwest, axum, tonic, qdrant-client)都基于它,避免运行时割裂。

---

### 4.3 LLM 接入 / LLM Integration: 多 Provider 抽象

**选型**: 自研轻量 `LlmProvider` trait + 评估 `rig-core`

**设计**:

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse>;
    async fn stream(&self, req: CompletionRequest) -> Result<BoxStream<'static, Result<Delta>>>;
    fn capabilities(&self) -> Capabilities; // 函数调用? 视觉? 上下文长度?
}

// 占位实现:OpenAiProvider / DeepSeekProvider
```

**理由**:
- 框架定位需要完全控制 prompt 格式、工具调用协议、错误重试策略
- `rig-core` 提供更高层抽象但可能限制灵活性,评估后决定是否作为底层依赖

**对比**:

| 方案 | 优点 | 缺点 | 结论 |
|------|------|------|------|
| **自研 trait** ✅ | 完全可控、零魔法 | 需自己维护各 Provider 协议 | 选用(核心) |
| `rig-core` | 开箱即用、统一抽象 | 抽象可能泄漏、升级耦合 | 评估作为可选后端 |
| `async-openai` | OpenAI 协议成熟 | 锁定单一协议 | 不符合多 Provider 目标 |
| 纯 OpenAI 兼容协议 | 一个 client 通吃 | 无法用 Anthropic 原生工具调用等特性 | 不符合目标 |

**路由策略**: 按 task type 路由(规划→强模型,执行→便宜模型),支持切换低成本 API key 降本。

---

### 4.4 记忆存储 / Memory Storage: Qdrant + redb

**选型**:
- **向量检索**: Qdrant (本地嵌入式 / 独立服务双模式)
- **本地 KV / 结构化**: redb(纯 Rust、嵌入式、事务支持)

**设计**:

```rust
#[async_trait]
pub trait MemoryStore: Send + Sync {
    async fn store(&self, mem: Memory) -> Result<MemoryId>;
    async fn recall(&self, query: &RecallQuery) -> Result<Vec<Memory>>;
    async fn forget(&self, id: MemoryId) -> Result<()>;
    async fn list(&self, filter: &MemoryFilter) -> Result<Vec<Memory>>; // 记忆管理
    async fn adjust_weight(&self, id: MemoryId, weight: f32) -> Result<()>; // 调节
}
```

**记忆分层**:
- **短期记忆**: 内存中(LRU),单次会话上下文
- **情景记忆 (Episodic)**: Qdrant 向量检索,过往任务/对话片段
- **语义记忆 (Semantic)**: redb 存结构化事实(用户偏好、项目约定)
- **用户画像**: 单独维护,高权重,长期沉淀开发特点

**对比**:

| 方案 | 优点 | 缺点 | 结论 |
|------|------|------|------|
| **Qdrant + redb** ✅ | Rust 原生、本地+分布式、性能好 | 双存储需协调 | 选用 |
| 纯 `sled` | 单一存储 | 无原生向量检索 | 不满足语义召回 |
| PostgreSQL + pgvector | 成熟、SQL 灵活 | 重、嵌入式不友好 | 备选(分布式期) |
| ChromaDB | Python 友好 | Rust client 弱 | 不选 |

---

### 4.5 任务队列与调度 / Task Queue & Scheduling

**选型**:
- **进程内队列(一期)**: `tokio::sync` + 自研 DAG 调度器
- **跨节点消息(二期)**: NATS(轻量、Rust 原生 client `async-nats`)
- **RPC**: `tonic` (gRPC)

**理由**: 一期单机不需要外部 broker,DAG 调度器自研可控;二期分布式时 NATS 比 Kafka 轻量得多,且 Rust client 成熟。

**对比**:

| 方案 | 优点 | 缺点 | 结论 |
|------|------|------|------|
| **自研 + NATS(二期)** ✅ | 渐进、轻量、Rust 原生 | DAG 调度需自研 | 选用 |
| Redis Stream | 通用、简单 | 非原生 Rust 优先 | 备选 |
| Kafka | 高吞吐、成熟 | 过重、运维复杂 | 不选 |

---

### 4.6 隔离执行环境 / Isolation: Firecracker microVM(主)+ Docker(开发期备选)

**选型**:
- **生产**: Firecracker microVM(`firec-rs` 或直接 REST API)
- **开发期**: Docker(通过 `IsolationProvider` trait 抽象,本地无 KVM 时降级)

**设计**:

```rust
#[async_trait]
pub trait IsolationProvider: Send + Sync {
    async fn create_vm(&self, spec: VmSpec) -> Result<VmHandle>;
    async fn exec(&self, vm: &VmHandle, cmd: Command) Result<ExecOutput>;
    async fn snapshot(&self, vm: &VmHandle) -> Result<Snapshot>;     // 快照复用
    async fn destroy(&self, vm: VmHandle) -> Result<()>;
}

pub struct DockerProvider { /* 开发期 */ }
pub struct FirecrackerProvider { /* 生产 */ }
```

**理由**:
- Firecracker:~125ms 启动、KVM 强隔离、AWS Lambda/Fargate 同款,最贴合"完全隔离"
- Docker provider 让 Windows 开发机也能跑通流程(虽隔离弱)
- trait 抽象让两者可切换,渐进式落地

**对比**:

| 方案 | 隔离强度 | 启动延迟 | Windows 可用 | 结论 |
|------|---------|---------|-------------|------|
| **Firecracker** ✅ | 强(独立内核) | ~125ms | 否(需 Linux/KVM) | 生产选用 |
| **Docker** ✅(开发) | 弱(共享内核) | 秒级 | 是 | 开发期选用 |
| gVisor | 中(用户态内核) | 秒级 | 否 | 备选 |
| Kata Containers | 强(轻量 VM) | 秒级 | 否 | 备选 |
| WASM(Wasmtime) | 中 | 毫秒级 | 是 | 未来探索(受限) |

> ⚠️ **关键约束**: Firecracker 需 `/dev/kvm`,即 Linux host + KVM。Windows 开发机的 WSL2 嵌套虚拟化支持有限且实验性。详见 [§5 风险](#5-risk--mitigation--风险与缓解)。

---

### 4.7 控制面 / Control Plane: CLI + Web Dashboard

**选型**:
- **CLI**: `clap` (derive 风格) + `ratatui`(可选 TUI 实时面板)
- **Web 后端**: `axum` + `tokio-tungstenite`(WebSocket 实时推送)
- **Web 前端**: 独立子项目(文档规划,技术栈待定,候选 SolidJS/Svelte/React)

**理由**: CLI 贴合开发者习惯、开发快;Web 仪表盘满足"记忆管理可视化""任务队列实时监控"需求。两者共用 `axon-dispatcher` / `axon-memory` 的 API 层。

---

### 4.8 可观测性 / Observability: tracing + OpenTelemetry

**选型**:
- **结构化日志/追踪**: `tracing` + `tracing-subscriber`
- **分布式追踪(二期)**: `opentelemetry` + OTLP exporter → Jaeger/Tempo
- **指标(二期)**: `metrics` crate + Prometheus exporter

**理由**: `tracing` 是 Rust async 生态标准,与 tokio 深度集成;OTLP 标准化便于分布式阶段跨节点追踪任务流。

---

### 4.9 配置管理 / Configuration: figment

**选型**: `figment`(Toml + 环境变量 + Profile)

**理由**: 比 `config-rs` API 更现代,支持多源合并与环境变量覆盖,适合"开发/测试/生产"多 profile。

---

### 4.10 序列化 / Serialization: serde + prost

**选型**:
- **JSON / 通用**: `serde` + `serde_json`
- **gRPC**: `prost`(protobuf 生成)

---

### 4.11 错误处理 / Error Handling

**选型**:
- **库 crate**: `thiserror`(派生错误枚举,显式分类)
- **应用层(二进制)**: `anyhow`(聚合错误,简化传播)

---

### 4.12 测试 / Testing

**选型**:
- **单元测试**: Rust 内置 `#[test]`
- **集成测试**: `testcontainers-rs`(启 Qdrant / Postgres / mock 服务)
- **Firecracker 测试**: 仅在 Linux CI 跑;Windows 跑 Docker provider 测试

---

### 4.13 依赖版本汇总 / Dependency Summary

| 用途 | Crate | 版本策略 |
|------|-------|---------|
| 异步运行时 | `tokio` | 集中管理于 `[workspace.dependencies]` |
| Web 框架 | `axum` | 同上 |
| gRPC | `tonic`, `prost` | 同上 |
| HTTP 客户端 | `reqwest` | 同上 |
| 序列化 | `serde`, `serde_json` | 同上 |
| 错误 | `thiserror`, `anyhow` | 同上 |
| 日志 | `tracing`, `tracing-subscriber` | 同上 |
| 配置 | `figment` | 同上 |
| CLI | `clap` | 同上 |
| 向量库 | `qdrant-client` | 同上 |
| 本地 KV | `redb` | 同上 |
| 消息 | `async-nats`(二期) | 同上 |
| 隔离 | `firec-rs` / bollard(Docker) | 同上 |
| Tokenizer | `tiktoken-rs` | 同上 |
| trait 异步 | `async-trait` | 同上 |
| 集成测试 | `testcontainers` | dev-dependency |

> 所有版本号集中在根 `Cargo.toml` 的 `[workspace.dependencies]`,子 crate 用 `dep.workspace = true` 引用,便于统一升级。

---

## 5. Risk & Mitigation / 风险与缓解

### ⚠️ R1: Firecracker 与 Windows 开发环境冲突(最高风险)

**风险**: Firecracker 需要 Linux 内核 + KVM(`/dev/kvm`)。当前开发机为 Windows。WSL2 的嵌套虚拟化(KVM inside WSL)支持有限且实验性,不稳定。

**缓解**:
1. **`IsolationProvider` trait 抽象**: 开发期默认用 `DockerProvider`(Windows Docker Desktop 可用),生产用 `FirecrackerProvider`
2. **开发/生产分离**: 本地开发跑 Docker provider,Firecracker 验证放 Linux CI(GitHub Actions ubuntu runner 支持 KVM)或 Linux 服务器
3. **WSL2 可选**: 如需本地体验 Firecracker,在 WSL2 Ubuntu 中尝试嵌套 KVM(不保证稳定),文档标注为实验性
4. **CI 双轨**: Windows runner 跑 Docker provider 测试,Linux runner 跑 Firecracker 测试

### ⚠️ R2: 分布式原生带来的复杂度

**风险**: 一开始就按多节点设计,可能导致一期交付周期长、调试难。

**缓解**:
- 架构上解耦(队列、状态、记忆均外置可分布式),但**一期默认单机进程内运行**
- 通过 feature flag 控制是否启用 NATS / 远程 Qdrant
- 二期再开启多节点

### ⚠️ R3: LLM Provider 成本与速率限制

**风险**: 多 agent 并发 + microVM 多任务,LLM 调用量大,成本与速率受限。

**缓解**:
- `LlmProvider` 路由层支持按任务类型选模型(规划用强模型,执行用便宜模型)
- 选择低成本模型/Provider 作为 fallback,降低成本
- 请求级缓存(相同 prompt 命中)
- 令牌桶限速 + 指数退避重试

### ⚠️ R4: Firecracker Rust SDK 成熟度

**风险**: `firec-rs` 等社区绑定可能滞后于 Firecracker 上游。

**缓解**:
- 评估 `firec-rs` 维护活跃度;若不满意,直接用 `reqwest` 调 Firecracker 的 REST API(协议简单稳定)
- 在 `IsolationProvider` 后封装,SDK 可替换

### ⚠️ R5: 记忆污染与上下文爆炸

**风险**: 长期记忆无限增长,检索召回噪声大,反而降低 agent 质量。

**缓解**:
- 记忆分层 + 权重衰减(长期不访问降权)
- 用户可调节(`MemoryStore::adjust_weight` / `forget`)
- 召回时做相关性 + 时效性 + 权重综合排序,Top-K 截断

### ⚠️ R6: microVM 镜像与启动开销

**风险**: 每个任务启 microVM,若镜像大、启动慢,影响吞吐。

**缓解**:
- Firecracker 支持快照(snapshot)恢复,预热基础镜像后毫秒级恢复
- VM 池化:预启动若干空闲 VM 复用

---

## 6. Development Roadmap / 开发路线

### M0 — 骨架搭建 (Skeleton) ✅ 本次交付
- Cargo workspace 多 crate 结构
- 核心 trait 占位(`LlmProvider` / `MemoryStore` / `IsolationProvider` / `TaskQueue`)
- `.gitignore`、CI 占位、本文档
- **验收**: `cargo build --workspace` 通过

### M1 — 单机 CLI 跑通 (Single-Node MVP) ✅ 已交付
- `axon-llm`: `OpenAiProvider` / `DeepSeekProvider` 实现 + `create_provider_from_env` 路由
- `axon-brain`: `SimplePlanner`(单任务) + `CommandAgent`(LLM 生成 shell 命令)
- `axon-memory`: `InMemoryStore` 占位实现
- `axon-dispatcher`: `InProcessQueue` + `SimpleScheduler`(串行调度,收集执行结果)
- `axon-isolation`: `DockerProvider` 基于系统 `docker` CLI
- `axon-worker`: `run_task` 接入 `Agent`
- `axon-cli`: `axon run --goal "..."` 跑通端到端
- **验收**: `cargo test --workspace` 通过；CLI 下发任务 → 启 Docker 容器 → agent 执行 → 返回结果

### M2 — 记忆系统 (Memory) ○ 进行中
- `axon-core`: ID 统一为 UUID v4
- `axon-llm`: 新增 `EmbeddingProvider` trait + OpenAI 实现
- `axon-memory`:
  - `RedbStore`: 语义/用户画像/短期记忆的本地 KV 实现
  - `QdrantStore`: 情景记忆的向量存储与召回
  - `HybridMemoryStore`: 按 `MemoryKind` 路由的统一 `MemoryStore`
- `axon-brain`: `SimplePlanner` / `CommandAgent` 规划/生成前 `recall` 相关记忆
- `axon-dispatcher`: 任务执行后沉淀 `Episodic` 记忆
- `axon-cli`: `axon memory init/list/forget/adjust` 管理命令 + `run --goal` 接入 HybridMemoryStore
- **验收**: `cargo test --workspace` 通过；记忆分层存储、召回、调节、遗忘均可经 CLI 操作

### M3 — Firecracker 隔离 (Strong Isolation)
- `axon-isolation`: `FirecrackerProvider` 实现
- VM 生命周期、快照复用、资源限额
- Linux CI 跑通 Firecracker 集成测试
- **验收**: 任务在 microVM 内隔离执行,失败可丢弃不影响 host

### M4 — 分布式 + Web Dashboard (Distributed & UI)
- `axon-dispatcher`: NATS 跨节点调度
- `axon-dashboard`: axum Web API + WebSocket
- `axon-ui`: 前端仪表盘(任务队列/VM 状态/记忆管理可视化)
- **验收**: 多节点调度,Web 可视化监控与调节

### M5+ — 高级特性 (Advanced)
- ReAct 多步推理 + 工具调用
- 任务 DAG 编排
- 多 agent 协作
- 成本/性能监控仪表盘

---

## 7. Workspace Layout / 工作区布局

```
axon.ai/
├── Cargo.toml                  # workspace 根,集中管理依赖版本
├── Cargo.lock                  # 锁定(纳入版本控制)
├── .gitignore
├── README.md
├── LICENSE
├── docs/
│   └── tech-stack.md           # 本文档
├── .github/
│   └── workflows/
│       └── ci.yml              # CI: fmt + clippy + test
└── crates/
    ├── axon-core/              # 共享类型、错误、配置
    ├── axon-llm/               # LlmProvider trait + 实现
    ├── axon-memory/            # MemoryStore trait + 实现
    ├── axon-brain/             # Planner / Agent 编排
    ├── axon-dispatcher/        # 任务队列 + 调度 + VM 生命周期
    ├── axon-isolation/         # IsolationProvider trait + Firecracker/Docker
    ├── axon-worker/            # VM 内 agent 执行器
    ├── axon-proto/             # gRPC schema
    ├── axon-cli/               # 二进制:命令行入口
    └── axon-dashboard/         # Web API 服务
```

### 各 crate lib.rs 占位内容原则

每个 crate 的 `src/lib.rs` 包含:
1. **模块文档注释**(`//!`):说明职责与边界
2. **1-2 个核心 trait 定义**:带 `#[async_trait]`,方法签名完整但无实现
3. **占位 struct / enum**:错误类型、配置结构等
4. **无业务逻辑**:仅编译通过的骨架

详见各 crate 的 `src/lib.rs`。

---

## 附录 A:技术选型速查表 / Quick Reference

| 领域 | 选型 | 备注 |
|------|------|------|
| 语言 | Rust 2021 | |
| 运行时 | tokio | |
| Web 框架 | axum | |
| gRPC | tonic + prost | |
| HTTP | reqwest | |
| LLM 抽象 | 自研 trait(+评估 rig-core) | |
| 向量库 | Qdrant | |
| 本地 KV | redb | |
| 消息队列 | NATS(二期) | |
| 隔离 | Firecracker(主)+ Docker(开发) | |
| CLI | clap + ratatui | |
| 日志 | tracing | |
| 配置 | figment | |
| 序列化 | serde + prost | |
| 错误 | thiserror + anyhow | |
| 测试 | 内置 + testcontainers | |

---

## 附录 B:参考资料 / References

- Firecracker: https://firecracker-microvm.github.io/
- Qdrant: https://qdrant.tech/
- NATS: https://nats.io/
- tokio: https://tokio.rs/
- axum: https://github.com/tokio-rs/axum
- tonic: https://github.com/hyperium/tonic
- rig (Rust LLM framework): https://github.com/0xPlaygrounds/rig

---

*本文档为活文档,随项目演进持续更新。*
