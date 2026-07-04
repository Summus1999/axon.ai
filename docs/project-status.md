# axon.ai 项目进度管控 / Project Status & Tracking

> 版本 / Version: 0.1.0
> 最后更新 / Last Updated: 2026-07-04
> 维护者 / Owner: Summus
> 关联文档: [tech-stack.md](./tech-stack.md)(技术选型)、[product-roadmap.md](./product-roadmap.md)(产品需求)、[../AGENTS.md](../AGENTS.md)(开发规范)

---

## 目录 / Table of Contents

1. [进度把控机制 / Tracking Mechanism](#1-进度把控机制--tracking-mechanism)
2. [里程碑总览 / Milestone Overview](#2-里程碑总览--milestone-overview)
3. [任务状态模型 / Task State Model](#3-任务状态模型--task-state-model)
4. [Definition of Done](#4-definition-of-done)
5. [当前进度快照 / Current Snapshot](#5-当前进度快照--current-snapshot)
6. [M1 任务分解 / M1 Breakdown](#6-m1-任务分解--m1-breakdown)
7. [M2 任务分解 / M2 Breakdown](#7-m2-任务分解--m2-breakdown)
8. [风险与阻塞登记 / Risk & Blocker Log](#8-风险与阻塞登记--risk--blocker-log)
9. [文档维护规则 / Maintenance Rules](#9-文档维护规则--maintenance-rules)

---

## 1. 进度把控机制 / Tracking Mechanism

### 1.1 核心机制

进度通过**三层联动**把控,确保"看得见、追得到、可验收":

```
里程碑 (Milestone)  ──►  任务 (Work Item)  ──►  提交 (Commit)
   粗粒度目标              细粒度可追踪单元         实际产出证据
   对应版本发布            状态流转 + DoD           git 历史 + 测试
```

| 层级 | 粒度 | 载体 | 更新时机 |
|------|------|------|---------|
| 里程碑 | 版本级(M0-M5) | 本文档 §2 + product-roadmap §7 | 里程碑完成时 |
| 任务 | Story/子任务 | 本文档 §6(当前里程碑展开) | 状态变更时 |
| 提交 | 代码变更 | git log + commit message | 每次提交 |

### 1.2 进度看板(任务状态汇总)

每个里程碑维护一个任务表,列含义:

| 列 | 说明 |
|----|------|
| ID | 任务标识(对齐 product-roadmap 的 Story ID) |
| 任务 | 简述 |
| 状态 | `×` 未做 / `○` 当前进度点 / `√` 已完成(详见 [AGENTS.md §4.4](../AGENTS.md#44-进度标记规范强制-progress-marking-convention-mandatory)) |
| 负责 | 执行者(AI agent / 人类) |
| 关联 | 关联 commit / PR |

### 1.3 更新频率与责任

- **AI agent 每次任务完成时**:更新对应任务状态 + 关联 commit hash。
- **里程碑完成时**:更新 §5 进度快照 + §2 里程碑状态 + product-roadmap 版本状态。
- **遇阻塞时**:立即登记到 §7,任务保持 `○` 并说明阻塞原因。
- **人工 review 节点**:每个里程碑结束需用户 review,通过方可标记 `√`。

### 1.4 进度健康度指标

| 指标 | 定义 | 健康阈值 |
|------|------|---------|
| 里程碑燃尽 | 当前里程碑 Done 任务数 / 总任务数 | 按计划曲线 |
| 阻塞时长 | 任务处于 Blocked 的累计时长 | < 2 天 |
| 测试门禁通过率 | 提交前 fmt+clippy+test 一次通过率 | 100%(强制) |
| DoD 达成率 | 已标记 Done 的任务中满足 DoD 的比例 | 100% |

---

## 2. 里程碑总览 / Milestone Overview

| 里程碑 | 版本 | 目标 | 状态 | 完成日期 |
|--------|------|------|------|---------|
| **M0** | v0.1 | 骨架:workspace + trait 占位 + 文档 | √ | 2026-07-04 |
| **M1** | v0.2 | 单机 MVP:CLI + OpenAI/DeepSeek + Docker 隔离 + 自检 | √ | 2026-07-04 |
| **M2** | v0.3 | 记忆系统:Qdrant + redb + 用户画像 + 记忆管理 | ○(当前) | — |
| **M3** | v0.4 | Firecracker 强隔离 + VM 生命周期 + 快照 | × | — |
| **M4** | v0.5 | 分布式(NATS)+ Web 仪表盘 + 可观测性 | × | — |
| **M5** | v0.6+ | 高级:多步 DAG + ReAct + 多 agent + 成本监控 | × | — |

> 状态符号:`×` 未做 / `○` 当前进度点 / `√` 已完成。详见 [AGENTS.md §4.4](../AGENTS.md#44-进度标记规范强制-progress-marking-convention-mandatory)。

> 里程碑详细范围见 [product-roadmap.md §7](./product-roadmap.md#7-版本规划--release-planning)。

---

## 3. 任务状态模型 / Task State Model

任务状态用 `×`/`○`/`√` 三符号标记(见 [AGENTS.md §4.4](../AGENTS.md#44-进度标记规范强制-progress-marking-convention-mandatory)),流转如下:

```
        ┌─────────┐
        │    ×    │  未做 / 未开始
        └────┬────┘
             │ 开始执行(标记为 ○,同一时刻唯一)
             ▼
        ┌─────────────┐
        │      ○      │  当前进度点 / 进行中
        └────┬───┬────┘
             │   │ 遇阻塞
             │   ▼
             │  ┌──────────┐
             │  │ Blocked  │  阻塞待解(仍记 ○,在 §7.2 登记)
             │  └────┬─────┘
             │       │ 阻塞解除
             │       ▼
             │  回到 ○
             ▼
        ┌──────────┐
        │    √     │  已完成(满足 §4 DoD + 关联提交)
        └──────────┘
```

**状态定义**:
- **`×` 未做**:已分解但未开始(默认初始)。
- **`○` 当前进度点**:正在执行;**全文档同一时刻有且仅有一个 `○`**。
- **`√` 已完成**:代码完成 + 测试通过(AGENTS.md §3.4 门禁)+ 关联提交。
- **Blocked**:阻塞态仍标记 `○`,但在 §7.2 登记阻塞原因;无法继续需外部输入(用户决策/依赖未就绪/环境问题)。

---

## 4. Definition of Done

### 4.1 任务级 DoD

一个任务标记 `√` 必须满足:

- × 代码实现完成,符合 [AGENTS.md](../AGENTS.md) §1(优雅 + 函数级注释)
- × 代码分块合理,符合 AGENTS.md §2
- × 测试先行:先写测试再实现(AGENTS.md §3.1)
- × 测试覆盖:正常/边界/错误路径(AGENTS.md §3.3)
- × 提交前门禁全过:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`(全部通过,无 `#[ignore]` 逃避)
- × 已提交(commit message 符合 AGENTS.md §4.2)
- × 本文档对应任务状态已更新 + 关联 commit hash

> 上表为 DoD 检查模板,每项随任务推进由 `×` 改 `√`;全部 `√` 后任务方可标 `√`。

### 4.2 里程碑级 DoD

一个里程碑标记 `√` 必须满足:

- √ 里程碑内所有任务为 `√`
- √ 端到端验收场景通过(见 product-roadmap 对应版本"验收"项)
- √ 文档同步更新(tech-stack / product-roadmap / project-status)
- √ 风险登记册回顾,已识别风险有处置结论
- √ **用户 review 通过**

---

## 5. 当前进度快照 / Current Snapshot

### 5.1 总体进度

```
M0  √  100%  已完成
M1  √  100%  已验收
M2  ○    0%  当前进度点  ← 下一个里程碑
M3  ×    0%  未做
M4  ×    0%  未做
M5  ×    0%  未做
```

### 5.2 M0 完成清单(已验收)

| 交付物 | 状态 | Commit |
|--------|------|--------|
| `docs/tech-stack.md` 技术选型文档 | √ | `9d7228d` |
| Cargo workspace 根配置 | √ | `9d7228d` |
| 10 个 crate 骨架 + trait 占位 | √ | `9d7228d` |
| `.gitignore` + CI workflow | √ | `9d7228d` |
| `README.md` 更新 | √ | `9d7228d` |
| `AGENTS.md` 开发规范 | √ | `d9c4dad` |
| 编译验证(build/clippy/test) | √ | `9d7228d` |

### 5.3 已知遗留(非阻塞)

| 项 | 说明 | 处置 |
|----|------|------|
| 本地代理问题 | 开发机 `127.0.0.1:7897` 代理未运行,需 `CARGO_HTTP_PROXY=` 绕过 | 待用户决定是否加项目级 `.cargo/config.toml` |
| 无真实测试 | 骨架阶段 0 测试,M1 起按测试先行补充 | √ 已由 M1-T14 真实 DeepSeek API 端到端测试解决(`29ffc62`) |

---

## 6. M1 任务分解 / M1 Breakdown

> M1 目标:端到端跑通 `axon run --goal "..."` → Docker 隔离执行 → 自检 → 回报。
> 任务 ID 对齐 [product-roadmap.md §4](./product-roadmap.md#4-需求拆解--requirements-breakdown)。

### 6.1 任务看板

| ID | 任务 | 状态 | 关联 |
|----|------|------|------|
| **M1-T1** | LLM Provider 抽象实现:OpenAI/DeepSeek provider(S1.3.1/S1.3.2) | √ | `ed637c7` |
| ~~M1-T2~~ | ~~LLM Provider 抽象实现:Ollama provider~~ | ~~×~~ | ~~`ce8a93b` 已取消~~ |
| **M1-T3** | LlmRouter:按配置路由 provider(S1.3.1) | √ | `4294242` |
| **M1-T4** | Brain:单步 Planner(Goal → 单 Task)(S1.1.1/S1.2.1) | √ | `4294242` |
| **M1-T5** | Dispatcher:进程内优先级队列(S3.1.1) | √ | `4294242` |
| **M1-T6** | Dispatcher:并发调度循环 + 重试/超时(S3.2.1/S3.2.2) | √ | `4294242` |
| **M1-T7** | Isolation:DockerProvider 启停容器(S3.3.1) | √ | `4294242` |
| **M1-T8** | Isolation:容器内执行命令 + 工作目录挂载(S3.3.2) | √ | `4294242` |
| **M1-T9** | Worker:接收任务 + 调 LLM 生成改动(S4.1.1/S4.1.2/S4.1.3) | √ | `4294242` |
| **M1-T10** | Worker:自检验收(跑测试/构建)(S4.2.1/S4.2.2) | √ | `4294242` |
| **M1-T11** | Worker:结果回报(S4.3.1) | √ | `4294242` |
| **M1-T12** | CLI:`axon run` / `axon tasks` 接入真实流程(S5.1.1/S5.1.2) | √ | `4294242` |
| **M1-T13** | 配置加载:figment 接入 toml+env | √ | `8b468d5` |
| **M1-T14** | 端到端集成测试(真实 DeepSeek API + Docker 隔离) | √ | `29ffc62` |
| **M1-T15** | M1 验收 + 文档同步 + 用户 review | √ | `283b42e`/`1fca0db` |

> 状态符号:`×` 未做 / `○` 当前进度点(全文档唯一)/ `√` 已完成。M1 结束时 `○` 落在 **M1-T15**。

### 6.2 建议执行顺序(依赖关系)

```
M1-T13(配置)──► M1-T1/T2(provider)──► M1-T3(router)
                                          │
                   M1-T4(planner)◄────────┘
                       │
                       ▼
M1-T5(队列)──► M1-T6(调度)──► M1-T7/T8(Docker)──► M1-T9/T10/T11(worker)
                                                          │
                                                          ▼
                                          M1-T12(CLI)──► M1-T14(集成测试)──► M1-T15(验收)
```

### 6.3 M1 验收标准(端到端)

- √ `axon run --goal "写一个返回 hello world 的函数"` 使用真实 DeepSeek API 端到端成功(`29ffc62`)
- √ 任务在 Docker 容器内执行,主机无残留(`29ffc62`)
- √ worker 自检(构建/测试)通过后才回报(`29ffc62`)
- √ `axon tasks` 能查看任务状态流转(`283b42e`)
- √ M1 全部门禁(fmt/clippy/test)通过(`29ffc62`/`283b42e`)
- √ 用户 review 通过

---

## 7. M2 任务分解 / M2 Breakdown

> M2 目标:实现跨会话记忆沉淀与管理,让系统记住用户偏好与项目知识。
> 任务 ID 对齐 [product-roadmap.md §4](./product-roadmap.md#4-需求拆解--requirements-breakdown)。

### 7.1 任务看板

| ID | 任务 | 状态 | 关联 |
|----|------|------|------|
| **M2-T1** | 文档对齐与基线建立 | √ | `c2fd415` |
| **M2-T2** | 配置与后端选择统一(`axon.toml`/`.env`/backend) | √ | `b279f46` |
| **M2-T3** | GLM embedding provider | √ | `35d59a1` |
| **M2-T4** | 自动用户画像与语义记忆写入 | √ | `<hash>` |
| **M2-T5** | 权重衰减与 LRU 短期记忆 | ○ | — |
| **M2-T6** | 记忆管理 CLI 完善与跨会话集成测试 | × | — |
| **M2-T7** | 文档同步、验收与用户 review | × | — |

> 状态符号:`×` 未做 / `○` 当前进度点(全文档唯一)/ `√` 已完成。当前 `○` 落在 **M2-T5**。

### 7.2 建议执行顺序(依赖关系)

```
M2-T1(文档对齐)──► M2-T2(配置统一)──► M2-T3(GLM embedding)
                                           │
                                           ▼
              M2-T4(画像提取)◄───────── M2-T5(权重/LRU)
                  │
                  ▼
              M2-T6(CLI 集成测试)──► M2-T7(验收)
```

### 7.3 M2 验收标准(端到端)

- × 跨会话保留偏好(库选择、命名风格、测试约定)
- × 新任务自动应用记忆
- × `axon memory list/forget/adjust` 命令可用
- × 偏好错误时可修正
- × M2 全部门禁(fmt/clippy/test)通过
- × 用户 review 通过

---

## 8. 风险与阻塞登记 / Risk & Blocker Log

### 8.1 风险登记册(长期)

| ID | 风险 | 影响 | 概率 | 缓解 | 状态 |
|----|------|------|------|------|------|
| R1 | Firecracker 需 Linux/KVM,Windows 开发机不支持 | M3 阻塞 | 高 | 开发期用 Docker provider;Firecracker 在 Linux CI 验证 | 缓解中 |
| R2 | LLM 成本/速率限制 | 多 agent 并发受限 | 中 | 路由层选模型 + Ollama fallback + 缓存 + 限速 | 监控 |
| R3 | Firecracker Rust SDK(`firec-rs`)成熟度 | M3 集成风险 | 中 | 评估,必要时直调 REST API | 待评估 |
| R4 | 记忆污染/上下文爆炸 | agent 质量下降 | 中 | 分层 + 权重衰减 + 用户调节 + Top-K | M2 处理 |
| R5 | 本地代理(7897)未运行 | 开发构建失败 | 高 | `CARGO_HTTP_PROXY=` 绕过;待定项目级配置 | 待用户决策 |

> 技术风险详情见 [tech-stack.md §5](./tech-stack.md#5-risk--mitigation--风险与缓解)。

### 8.2 阻塞事项(当前)

| 日期 | 任务 | 阻塞原因 | 需要的输入 | 状态 |
|------|------|---------|-----------|------|
| — | — | 当前无阻塞 | — | — |

---

## 9. 文档维护规则 / Maintenance Rules

### 9.1 谁更新 / When to Update

| 事件 | 更新内容 | 责任 |
|------|---------|------|
| 任务状态变更 | §6/§7 看板状态 + 关联 commit | AI agent(执行后立即) |
| 遇阻塞 | §8.2 登记 + 任务标 Blocked | AI agent / 人类 |
| 提交代码 | 关联 commit hash 到对应任务 | AI agent |
| 里程碑完成 | §2 状态 + §5 快照 + §4.2 DoD 勾选 | AI agent(草拟)+ 人类(review) |
| 新增/调整需求 | product-roadmap 对应表 + 本文档任务 | 评审后 |

### 9.2 真实性要求

- 状态必须反映**实际**情况:未开始就是 Todo,不许虚标 In Progress 或 Done。
- Done 必须有 commit + 测试通过证据,不许"我觉得写完了"就标 Done。
- 阻塞必须如实登记,不许隐瞒。
- 进度快照的百分比基于任务数,不含水分。

### 9.3 与其他文档的关系

```
product-roadmap.md  ──定义做什么──►  project-status.md(本文)
                                          │
tech-stack.md       ──定义怎么做──►      │
                                          ▼
AGENTS.md           ──定义怎么写──►  实际代码 + git 历史
```

- 需求变更 → 改 product-roadmap → 同步本文任务。
- 技术调整 → 改 tech-stack → 同步本文风险/任务。
- 开发规范 → AGENTS.md(已是强制规范)。

---

*本文档为活文档,每次任务执行后更新;禁止虚报进度。*
