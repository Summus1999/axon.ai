# AGENTS.md — axon.ai 开发规范

> 本文件是所有 AI agent(及人类协作者)在本仓库工作的**强制规范**。
> 每次会话开始必须读取并遵守。与系统默认规则冲突时,**以本文件为准**。
> 版本:1.0 · 生效日期:2026-07-04

---

## 0. 总则 / General Principles

1. **本文件优先级最高**:与任何默认行为冲突时,按 `AGENTS.md` 执行。
2. **三条铁律不可破坏**(详见 §1–§3):
   - 代码优雅 + 每个函数必有函数级注释
   - 代码分块必须合理
   - 需求先写测试,提交前测试必须全过
3. **诚实汇报**:测试失败就说失败、贴输出;跳过步骤就说跳过;只有验证通过才说"完成"。
4. **不可逆动作(提交/推送/删除/覆盖)先确认**,除非用户已明确授权;授权不跨场景。

---

## 1. 代码质量 / Code Quality

### 1.1 优雅优先 / Elegance First

代码必须**可读、简洁、意图清晰**,不堆砌技巧,不写"能跑就行"的代码。

**要求**:
- 命名达意:函数/变量名自解释,避免 `tmp`、`data2`、`handle_stuff` 这类模糊命名。
- 单一职责:一个函数只做一件事;出现"并且"描述职责时考虑拆分。
- 及早返回 / 减少嵌套:用 guard clause 消除深层 `if` 嵌套。
- 无死代码、无冗余注释、无被注释掉的代码块。
- 错误显式处理:不裸 `unwrap()`/`expect()` 进入生产代码(测试代码除外);用 `?` 或匹配传播。
- 避免过度抽象:不为假想的未来需求过早泛化(YAGNI)。

### 1.2 函数级注释(强制)/ Function-Level Comments (Mandatory)

**每一个函数(包括私有函数、方法、测试函数)必须有函数级文档注释。**

- 使用 `///` 文档注释(doc comment),位于函数签名上方。
- 注释语言:中英混排(中文叙述 + 英文术语),与现有 crate 风格一致。
- 至少包含:**职责一句话说明**;参数复杂或返回值非显然时补充说明。
- `pub` 项的 doc comment 会被 `cargo doc` 收录,更要写好。
- 复杂算法/非显然逻辑**额外加行内 `//` 注释**解释 why,而非 what。

**示例 ✅**:
```rust
/// 计算任务的就绪状态:当且仅当其所有依赖任务都已完成。
///
/// `deps` 为该任务依赖的 task id 列表;`completed` 为当前已完成集合。
/// 返回 `true` 表示可调度执行。
fn is_ready(deps: &[TaskId], completed: &HashSet<TaskId>) -> bool {
    deps.iter().all(|d| completed.contains(d))
}
```

**反例 ❌**(无注释或注释无效):
```rust
fn is_ready(d: &[TaskId], c: &HashSet<TaskId>) -> bool {
    d.iter().all(|x| c.contains(x))  // 无函数级注释
}
```

### 1.3 模块级注释 / Module-Level Comments

每个 `lib.rs` / `main.rs` / 子模块文件**顶部必须有 `//!` 模块注释**,说明该模块的职责与边界(与现有 crate 风格一致)。

---

## 2. 代码分块 / Code Organization

### 2.1 分块原则 / Batching Principles

**代码必须按职责合理分块,文件不过长,模块不臃肿。**

- **单文件理想行数 ≤ 400 行**(含注释);超过则考虑拆模块。硬上限 600 行,超出必须拆分。
- **一个文件一个主职责**;出现两个以上不相关概念时拆成独立模块。
- **模块划分按领域概念**,不按类型(types/funcs 这种分法禁止)。

### 2.2 文件内分块顺序 / File-Internal Layout

Rust 源文件内部按以下顺序组织,段间空行分隔:

1. 模块文档注释 `//!`
2. `#![allow(...)]` / feature gate 等属性
3. `use` 导入(标准库 → 外部 crate → 本仓库 crate,各组内字母序)
4. 类型/结构体/枚举定义
5. trait 定义
6. 函数实现(关联函数 → 方法 → 自由函数)
7. 测试模块 `#[cfg(test)] mod tests`(若与实现同文件)

### 2.3 模块拆分 / Module Splitting

- 当一个 `lib.rs` 承载多职责时,拆为 `mod xxx;` 子文件,`lib.rs` 仅做 `pub use` 重导出。
- 例:`axon-core` 已拆为 `error.rs` / `config.rs`,`lib.rs` 聚合。
- 公共 API 在 `lib.rs` 用 `pub use` 暴露,内部模块细节默认私有。

### 2.4 依赖方向 / Dependency Direction

- 依赖必须单向、无环;遵循 `tech-stack.md §3.1` 的 crate 依赖图。
- 禁止下层 crate 依赖上层(`axon-core` 不得依赖任何 `axon-*`)。
- 跨 crate 共享类型放 `axon-core` 或 `axon-proto`,不在多处重复定义。

---

## 3. 测试规范 / Testing

### 3.1 测试先行(强制)/ Test-First (Mandatory)

**写实现代码之前,先写好测试用例。** 这是本项目的硬约束。

工作流:
1. **理解需求** → 明确输入/输出/边界条件。
2. **先写测试**:定义函数/trait 的签名(可 `todo!()` 占位实现),然后写测试用例覆盖:
   - 正常路径(happy path)
   - 边界条件(空输入、极值、单元素)
   - 错误路径(无效输入、失败场景)
3. **跑测试确认失败**(红):测试能编译但失败,证明测试有效。
4. **写实现**(绿):最小实现让测试通过。
5. **重构**:在测试保护下优化代码。

> 即 **Red → Green → Refactor** 的 TDD 循环。不允许"先写实现再补测试"。

### 3.2 测试分层 / Test Layers

| 层级 | 位置 | 内容 | 依赖 |
|------|------|------|------|
| 单元测试 | `#[cfg(test)] mod tests`(同文件或 `tests/` 子模块) | 纯逻辑、无外部依赖 | 无 |
| 集成测试 | `crates/<name>/tests/*.rs` | 跨模块/crate 协作 | 可用 `testcontainers` |
| crate 内联 | 同文件 `mod tests` | 私有项测试 | 无 |

### 3.3 测试覆盖要求 / Coverage Requirements

每个 `pub` 函数/trait 方法至少有:
- 1 个正常路径测试
- 1 个边界条件测试(适用时)
- 1 个错误路径测试(返回 `Result` 的必须有)

测试函数同样遵守 §1.2(函数级注释),例如:
```rust
/// 验证 `is_ready` 在所有依赖完成时返回 true。
#[test]
fn ready_when_all_deps_completed() { ... }
```

### 3.4 提交前测试门禁(强制)/ Pre-Commit Test Gate (Mandatory)

**代码提交前,所有测试必须全部通过,无例外。**

提交前必须依次执行并全部通过:
```bash
cargo fmt --all -- --check          # 格式检查
cargo clippy --workspace --all-targets -- -D warnings   # lint,零警告
cargo test --workspace              # 全部测试通过
```

- **任何一项未通过,禁止提交**。即使只改了一行注释,也要跑完整门禁。
- 测试失败时:如实报告失败 + 贴出失败输出,**不允许跳过/忽略失败测试来强行提交**。
- 禁止用 `#[ignore]` 逃避失败测试;`#[ignore]` 仅用于已知且经用户确认可暂缓的集成测试。
- 禁止删除/弱化测试来让构建通过(删测试必须说明理由并获用户同意)。

### 3.5 测试可执行性 / Test Executability

- 测试必须能在干净环境跑通,不依赖未声明的本地文件/服务。
- 需要外部服务(Qdrant/Docker/Firecracker)的集成测试,用 `testcontainers` 或 `#[ignore]` 标注(并在 CI 单独 job 跑)。
- 单元测试不得有网络/磁盘 IO,保持快速与确定性。

---

## 4. 工作流程 / Workflow

### 4.1 任务执行步骤

1. **理解**:读代码、搜现有实现,优先复用;不确定先问。
2. **规划**:非简单任务进 plan mode,写计划待审批。
3. **测试先行**:按 §3.1 写测试(Red)。
4. **实现**:最小实现让测试通过(Green),遵守 §1–§2。
5. **重构**:优化代码,测试仍绿。
6. **验证**:跑 §3.4 全套门禁。
7. **汇报**:如实说明做了什么、验证结果、需注意点。

### 4.2 提交规则 / Commit Rules

- 默认**先开分支**再提交;直推 `main` 需用户每次明确授权(授权不跨场景)。
- 提交信息用约定式前缀:`feat:` / `fix:` / `docs:` / `refactor:` / `test:` / `chore:`。
- 提交前必须 §3.4 门禁全过。
- 一次提交一个逻辑变更;不把无关改动混在一起。
- `Cargo.lock` 纳入版本控制(应用项目)。

### 4.3 依赖管理 / Dependency Management

- 新增依赖先评估必要性,优先用现有依赖解决问题。
- 版本统一声明在根 `Cargo.toml` 的 `[workspace.dependencies]`,子 crate 用 `workspace = true` 引用。
- 优先纯 Rust 实现的 crate;引入 `unsafe` 或 C 绑定需说明理由。

---

## 5. 风格细节 / Style Details

### 5.1 Rust 风格

- Edition 2021;`rust-version = 1.75`。
- 用 `rustfmt` 默认格式化,不自定义格式;提交前 `cargo fmt --all`。
- 公共 API 用 `serde` 派生 `Serialize/Deserialize`,字段 `snake_case`。
- 错误:库用 `thiserror`,应用二进制用 `anyhow`;不混用。
- 异步:`tokio` + `async-trait`;不在库中强制 `tokio` runtime(用 `trait` 抽象)。

### 5.2 注释语言

- 中英混排:中文叙述 + 英文术语(如"调度器 (Scheduler)")。
- doc comment(`///` / `//!`)必写;行内 `//` 解释 why 不解释 what。

### 5.3 占位与 TODO

- 骨架/未实现用 `todo!()` 或返回明确 `Err`,**不裸 `unimplemented!()` 进生产路径**。
- `TODO(Mn)` 标注后续里程碑(如 `// TODO(M1): 接入真实 provider`),与 `tech-stack.md` 路线图对应。

---

## 6. 汇报要求 / Reporting

每次任务结束汇报必须包含:
1. **做了什么**:实际改动概要。
2. **验证结果**:门禁各项 pass/fail(贴关键输出)。
3. **需注意**:风险、未决问题、下一步建议。
4. **诚实分级**:明确区分"已完成且验证" / "已完成未验证" / "跳过/未做"。

禁止:粉饰失败、跳过测试不报告、用"完美/彻底"等夸大词。

---

## 7. 例外与豁免 / Exceptions

任何对本规范的偏离(如临时跳过测试、放宽行数限制)必须:
1. 当场说明理由;
2. 获得用户明确同意;
3. 在汇报中记录该偏离。

不允许"静默偏离"。

---

*本规范随项目演进持续更新;重大修改需用户确认。*
